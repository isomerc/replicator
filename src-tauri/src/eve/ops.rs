use crate::error::{AppError, AppResult};
use crate::eve::characters::ProfileFile;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use sysinfo::System;

/// Outcome of a replication run: every file actually written, plus a
/// human-readable reason for each thing we declined to do.
#[derive(Debug, Serialize, Clone, Default, PartialEq, Eq)]
pub struct CopyReport {
    pub written: Vec<PathBuf>,
    pub skipped: Vec<String>,
}

pub fn is_eve_running() -> bool {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
    sys.processes()
        .values()
        .any(|p| looks_like_eve_process(&p.name().to_string_lossy()))
}

/// The game client is exefile.exe everywhere (natively on Windows, under
/// Proton/Wine elsewhere). The modern launcher spells itself differently
/// per platform: "eve-online" on Linux, "EVE Online.exe" (with a space)
/// on Windows. The old launcher was evelauncher.
fn looks_like_eve_process(name: &str) -> bool {
    let n = name.to_lowercase();
    n == "exefile.exe"
        || n.starts_with("evelauncher")
        || n.starts_with("eve-online")
        || n.starts_with("eve online")
}

pub fn ensure_not_running() -> AppResult<()> {
    if is_eve_running() {
        Err(AppError::EveRunning)
    } else {
        Ok(())
    }
}

/// Build the path to the `core_user_<user_id>.dat` for a given user
/// id inside a settings dir. Returns Some only if the file actually
/// exists (so the caller doesn't try to copy onto a nonexistent path).
pub fn user_file_for_user_id(settings_dir: &Path, user_id: i64) -> Option<PathBuf> {
    let p = settings_dir.join(format!("core_user_{user_id}.dat"));
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Enumerate every `core_user_*.dat` in `settings_dir`, excluding the
/// `core_user__.dat` placeholder. Used to do the bash-style blanket
/// "copy master user file to every other user file" pass.
pub fn all_user_files(settings_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(settings_dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !fname.starts_with("core_user_") || !fname.ends_with(".dat") {
            continue;
        }
        if fname == "core_user__.dat" {
            continue;
        }
        out.push(path);
    }
    out
}

/// Write settings bytes to `target`, matching `cp src dst` exactly:
/// the existing file is truncated in place (preserving its inode),
/// with no backup, no temp file and no atomic rename.
///
/// This mirrors the bash predecessor at
/// `~/dev/isomerc/eve_online/replicator/replicator.sh`. An earlier
/// version wrote via `.tmp` + rename (which changes the inode) plus a
/// `.replicator-bak` sidecar; both deviate from `cp` and may matter
/// under Wine/Proton, so don't reintroduce them.
pub fn write_profile_bytes(target: &Path, bytes: &[u8]) -> AppResult<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, bytes)?;
    Ok(())
}

/// Every (settings_dir, core_char_<id>.dat) pair where `target_id`
/// already has a profile. A character can appear in several settings
/// dirs (multiple EVE profiles); each one is a distinct write target.
/// BTreeMap keeps the result ordered so callers - and the report they
/// build - are deterministic.
pub fn target_paths_for_character(
    profiles: &[ProfileFile],
    target_id: i64,
) -> Vec<(PathBuf, PathBuf)> {
    let mut by_dir: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for p in profiles {
        if let ProfileFile::Character {
            id, settings_dir, ..
        } = p
        {
            if *id == target_id {
                let dest = settings_dir.join(format!("core_char_{id}.dat"));
                by_dir.insert(settings_dir.clone(), dest);
            }
        }
    }
    by_dir.into_iter().collect()
}

/// The character's file - the most recently modified one when the
/// character exists in several settings dirs. Enumeration order is
/// filesystem order (arbitrary), so first-match would pick a source at
/// random; newest-wins matches what the UI's "modified" column shows
/// and what the user means by "this character's settings".
pub fn find_character_file(profiles: &[ProfileFile], id: i64) -> Option<&ProfileFile> {
    profiles
        .iter()
        .filter(|p| matches!(p, ProfileFile::Character { id: i, .. } if *i == id))
        .max_by_key(|p| match p {
            ProfileFile::Character { modified, .. } => modified.unwrap_or(i64::MIN),
            _ => i64::MIN,
        })
}

/// Test convenience: the copy-everything path. Production always goes
/// through `replicate_with`, whose `None` selection is this exact
/// behavior.
#[cfg(test)]
pub fn replicate(
    source_id: i64,
    source_char_path: &Path,
    source_user_file: Option<&Path>,
    target_ids: &[i64],
    profiles: &[ProfileFile],
) -> AppResult<CopyReport> {
    replicate_with(
        source_id,
        source_char_path,
        source_user_file,
        target_ids,
        profiles,
        None,
        false,
    )
}

/// Which settings groups a copy should carry. `None` at the call sites
/// means "everything" and takes the byte-perfect whole-file path;
/// a selection routes every write through a decode-merge-encode of
/// EVE's marshal format instead, so unselected groups keep each
/// target's own values.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CopySelection {
    pub char_groups: Vec<String>,
    pub user_groups: Vec<String>,
}

/// Selectively merge `source_bytes` into the existing file at `dest`.
/// A merge failure (unparseable file, round-trip mismatch) skips the
/// write with a reason - the target is never half-written.
/// One target file of a selective copy. Every per-file failure - an
/// unreadable target, a parse failure, a failed write - lands in the
/// report instead of aborting the run: a disk problem on target four
/// must not leave targets one through three overwritten with no record
/// of it.
fn merge_into_file(
    source_bytes: &[u8],
    dest: &Path,
    groups: &[String],
    written: &mut Vec<PathBuf>,
    skipped: &mut Vec<String>,
) {
    let target_bytes = match std::fs::read(dest) {
        Ok(b) => b,
        Err(e) => {
            skipped.push(format!("{}: could not read target ({e})", dest.display()));
            return;
        }
    };
    match crate::eve::settings::selective_merge(source_bytes, &target_bytes, groups) {
        Ok(merged) => match std::fs::write(dest, merged) {
            Ok(()) => written.push(dest.to_path_buf()),
            Err(e) => skipped.push(format!("{}: write failed ({e})", dest.display())),
        },
        Err(e) => skipped.push(format!(
            "{}: selective copy failed ({e}); file left untouched",
            dest.display()
        )),
    }
}

/// The core replication algorithm, factored out of the
/// `copy_character` Tauri command so it can be tested without a
/// running app. Everything stateful - locating the install, refreshing
/// the launcher-log char->user map, resolving `source_user_file` from
/// the DB - happens in the caller; this function only touches the
/// filesystem.
///
/// Two passes, matching the bash predecessor:
///
/// 1. Write the source's `core_char` bytes to each target's
///    `core_char_<tid>.dat`, in every settings dir where that target
///    already has a profile.
/// 2. If we know the source's user file, write its bytes over *every*
///    `core_user_*.dat` in every settings dir touched by pass 1. This
///    is a deliberately wide blast radius - the user file is what
///    carries window positions, overview and chat - and it mirrors
///    what the shell script did.
///
/// With a `selection`, both passes route each write through a
/// decode-merge-encode of the marshal format instead of raw bytes, so
/// only the chosen groups move and every other group keeps the
/// target's own value.
///
/// Safety properties (each has a test):
/// - Source bytes are read ONCE upfront, so the source can never be
///   truncated by a write that happens later in the loop, even if
///   path equality fails under Wine/Proton path quirks.
/// - The source character is removed from the target set, and the
///   source's own user file is never overwritten.
/// - The `core_user__.dat` placeholder is never written.
/// - In selective mode, an unparseable or round-trip-unsafe target is
///   skipped with a reason and left byte-for-byte untouched.
/// - A copy stays on the source's server: settings dirs on a different
///   known server (Singularity vs Tranquility, say) are skipped with a
///   reason unless `cross_server` is set. Dirs with no recognizable
///   server are never restricted.
#[allow(clippy::too_many_arguments)]
pub fn replicate_with(
    source_id: i64,
    source_char_path: &Path,
    source_user_file: Option<&Path>,
    target_ids: &[i64],
    profiles: &[ProfileFile],
    selection: Option<&CopySelection>,
    cross_server: bool,
) -> AppResult<CopyReport> {
    let source_server =
        crate::eve::paths::server_of(source_char_path.parent().unwrap_or(source_char_path));
    // READ source bytes ONCE upfront so we can never accidentally
    // truncate the source by writing to it during the loop. Even if
    // path-equality skips fail for some reason (canonicalization,
    // symlinks, Wine path quirks), the worst case is we re-write the
    // source with its own bytes - no data loss.
    let source_char_bytes = std::fs::read(source_char_path)?;
    let source_user_bytes = match source_user_file {
        Some(p) => Some(std::fs::read(p)?),
        None => None,
    };

    // BTreeSet rather than HashSet: dedups and drops the source the
    // same way, but iterates in a stable order so the resulting
    // report is deterministic.
    let target_set: BTreeSet<i64> = target_ids
        .iter()
        .copied()
        .filter(|i| *i != source_id)
        .collect();

    let mut written = Vec::new();
    let mut skipped = Vec::new();
    let mut settings_dirs: BTreeSet<PathBuf> = BTreeSet::new();

    // -- 1) Char file copy: source char bytes -> each target's core_char_<tid>.dat --
    for tid in &target_set {
        let targets = target_paths_for_character(profiles, *tid);
        if targets.is_empty() {
            skipped.push(format!("target {tid} has no existing profile file"));
            continue;
        }
        for (target_dir, char_dest) in targets {
            if !cross_server {
                if let (Some(s), Some(t)) =
                    (source_server, crate::eve::paths::server_of(&target_dir))
                {
                    if s != t {
                        skipped.push(format!(
                            "{}: on {t}, source is on {s} (cross-server copy is off)",
                            char_dest.display()
                        ));
                        continue;
                    }
                }
            }
            // Recorded even when the char write is skipped: a
            // user-groups-only selection still needs to know which
            // dirs hold its targets.
            settings_dirs.insert(target_dir);
            if char_dest == source_char_path {
                continue;
            }
            match selection {
                None => match std::fs::write(&char_dest, &source_char_bytes) {
                    Ok(()) => written.push(char_dest),
                    Err(e) => skipped.push(format!("{}: write failed ({e})", char_dest.display())),
                },
                Some(sel) if sel.char_groups.is_empty() => {}
                Some(sel) => merge_into_file(
                    &source_char_bytes,
                    &char_dest,
                    &sel.char_groups,
                    &mut written,
                    &mut skipped,
                ),
            }
        }
    }

    // -- 2) User file copy: bash-style. For every settings dir we
    //    touched, write the source user bytes onto every
    //    core_user_*.dat in that dir (skip the source itself and the
    //    `core_user__.dat` placeholder, which all_user_files omits).
    let wants_user = selection.map_or(true, |s| !s.user_groups.is_empty());
    match (source_user_file, &source_user_bytes) {
        (Some(src_user), Some(user_bytes)) if wants_user => match selection {
            None => {
                blanket_user_files(
                    &settings_dirs,
                    user_bytes,
                    Some(src_user),
                    &mut written,
                    &mut skipped,
                );
            }
            Some(sel) => {
                for dir in &settings_dirs {
                    for tgt_user_file in all_user_files(dir) {
                        if tgt_user_file.as_path() == src_user {
                            continue;
                        }
                        merge_into_file(
                            user_bytes,
                            &tgt_user_file,
                            &sel.user_groups,
                            &mut written,
                            &mut skipped,
                        );
                    }
                }
            }
        },
        (Some(_), Some(_)) => {}
        _ if wants_user => skipped.push(format!(
            "no account-level file for source character {source_id} (no launcher-log mapping, or no core_user file in its settings dir) - only character files were copied. Logging this character in via the EVE launcher usually fixes both."
        )),
        _ => {}
    }

    Ok(CopyReport { written, skipped })
}

/// Pass 2 of any replication: write `user_bytes` over every
/// `core_user_*.dat` in each of `dirs` (the placeholder is excluded by
/// `all_user_files`), skipping `skip` - the source's own user file,
/// when the source is a live character. Per-file failures land in the
/// report; one bad file must not hide what was already written.
fn blanket_user_files(
    dirs: &BTreeSet<PathBuf>,
    user_bytes: &[u8],
    skip: Option<&Path>,
    written: &mut Vec<PathBuf>,
    skipped: &mut Vec<String>,
) {
    for dir in dirs {
        for tgt_user_file in all_user_files(dir) {
            if Some(tgt_user_file.as_path()) == skip {
                continue;
            }
            match std::fs::write(&tgt_user_file, user_bytes) {
                Ok(()) => written.push(tgt_user_file),
                Err(e) => skipped.push(format!("{}: write failed ({e})", tgt_user_file.display())),
            }
        }
    }
}

/// The template-apply core: `replicate`'s two passes with a stored
/// template as the source instead of a live character. Write the
/// stored char bytes to every target's `core_char_<tid>.dat`, then
/// blanket the stored user bytes - when the template has them - over
/// every user file in the touched dirs. Templates saved before the
/// user file was captured (or with no launcher-log mapping at save
/// time) apply only character files, and the report says so.
/// Test convenience: the apply-everything path (see `replicate`).
#[cfg(test)]
pub fn apply_template_bytes(
    char_bytes: &[u8],
    user_bytes: Option<&[u8]>,
    target_ids: &[i64],
    profiles: &[ProfileFile],
) -> AppResult<CopyReport> {
    apply_template_bytes_with(
        char_bytes, user_bytes, target_ids, profiles, None, None, false,
    )
}

/// A template lands only on settings dirs of the server it was saved
/// from (`source_server`), the same rule a copy follows, unless
/// `cross_server` is set. Templates with no recorded server - older
/// ones, or sources with no recognizable server - go anywhere.
#[allow(clippy::too_many_arguments)]
pub fn apply_template_bytes_with(
    char_bytes: &[u8],
    user_bytes: Option<&[u8]>,
    target_ids: &[i64],
    profiles: &[ProfileFile],
    selection: Option<&CopySelection>,
    source_server: Option<&str>,
    cross_server: bool,
) -> AppResult<CopyReport> {
    let target_set: BTreeSet<i64> = target_ids.iter().copied().collect();

    let mut written = Vec::new();
    let mut skipped = Vec::new();
    let mut settings_dirs: BTreeSet<PathBuf> = BTreeSet::new();

    for tid in &target_set {
        let targets = target_paths_for_character(profiles, *tid);
        if targets.is_empty() {
            skipped.push(format!("target {tid} has no existing profile file"));
            continue;
        }
        for (target_dir, char_dest) in targets {
            if !cross_server {
                if let (Some(s), Some(t)) =
                    (source_server, crate::eve::paths::server_of(&target_dir))
                {
                    if s != t {
                        skipped.push(format!(
                            "{}: on {t}, template is from {s} (cross-server apply is off)",
                            char_dest.display()
                        ));
                        continue;
                    }
                }
            }
            settings_dirs.insert(target_dir);
            match selection {
                None => match write_profile_bytes(&char_dest, char_bytes) {
                    Ok(()) => written.push(char_dest),
                    Err(e) => skipped.push(format!("{}: write failed ({e})", char_dest.display())),
                },
                Some(sel) if sel.char_groups.is_empty() => {}
                Some(sel) => merge_into_file(
                    char_bytes,
                    &char_dest,
                    &sel.char_groups,
                    &mut written,
                    &mut skipped,
                ),
            }
        }
    }

    let wants_user = selection.map_or(true, |s| !s.user_groups.is_empty());
    match user_bytes {
        Some(user_bytes) if wants_user => match selection {
            None => {
                blanket_user_files(&settings_dirs, user_bytes, None, &mut written, &mut skipped);
            }
            Some(sel) => {
                for dir in &settings_dirs {
                    for tgt_user_file in all_user_files(dir) {
                        merge_into_file(
                            user_bytes,
                            &tgt_user_file,
                            &sel.user_groups,
                            &mut written,
                            &mut skipped,
                        );
                    }
                }
            }
        },
        Some(_) => {}
        None if wants_user => skipped.push(
            "template has no account-level settings - only character files were applied. \
             Re-save the template after its source character has logged in via the launcher once."
                .to_string(),
        ),
        None => {}
    }

    Ok(CopyReport { written, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use tempfile::TempDir;

    // ------------------------ eve-running guard -----------------------

    #[test]
    fn ensure_not_running_mirrors_is_eve_running() {
        // Every destructive command is gated on this pair, so they must
        // never disagree - whichever way the host happens to answer.
        assert_eq!(ensure_not_running().is_err(), is_eve_running());
    }

    #[test]
    fn eve_process_names_match_on_every_platform() {
        // The Windows launcher has a SPACE in its process name; missing
        // it made the guard launcher-blind on native Windows.
        assert!(looks_like_eve_process("exefile.exe"));
        assert!(looks_like_eve_process("ExeFile.exe"));
        assert!(looks_like_eve_process("EVE Online.exe"));
        assert!(looks_like_eve_process("eve-online"));
        assert!(looks_like_eve_process("evelauncher.exe"));
        assert!(!looks_like_eve_process("explorer.exe"));
        assert!(!looks_like_eve_process("steam"));
        assert!(!looks_like_eve_process("eventviewer.exe"));
    }

    #[test]
    fn the_eve_running_error_is_the_dedicated_variant() {
        if let Err(e) = ensure_not_running() {
            assert!(
                matches!(e, AppError::EveRunning),
                "the UI keys its warning pill off this variant"
            );
        }
    }

    // ------------------------- all_user_files -------------------------

    #[test]
    fn all_user_files_excludes_the_placeholder() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_user(&dir, 9001, "a");
        write_user(&dir, 9002, "b");
        write_user_placeholder(&dir);

        let mut found = all_user_files(&dir);
        found.sort();

        assert_eq!(
            found,
            vec![
                dir.join("core_user_9001.dat"),
                dir.join("core_user_9002.dat")
            ],
            "core_user__.dat is EVE's pre-selection placeholder and must never be a write target"
        );
    }

    #[test]
    fn all_user_files_ignores_unrelated_files() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_user(&dir, 9001, "a");
        write_char(&dir, 1001, "char");
        std::fs::write(dir.join("core_user_9002.txt"), "wrong ext").unwrap();
        std::fs::write(dir.join("notes.dat"), "wrong prefix").unwrap();

        assert_eq!(all_user_files(&dir), vec![dir.join("core_user_9001.dat")]);
    }

    #[test]
    fn all_user_files_on_missing_dir_is_empty_not_an_error() {
        let tmp = TempDir::new().unwrap();
        assert!(all_user_files(&tmp.path().join("nope")).is_empty());
    }

    // ---------------------- user_file_for_user_id ---------------------

    #[test]
    fn find_character_file_prefers_the_newest_profile() {
        // Enumeration order is filesystem order; the source must be
        // the file the user most recently played on, not a stale 2019
        // copy that happens to sort first.
        let tmp = TempDir::new().unwrap();
        let old_dir = settings_dir(tmp.path(), "settings_Old");
        let new_dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&old_dir, 1001, "stale");
        write_char(&new_dir, 1001, "current");
        let mut old_p = char_profile(&old_dir, 1001);
        let mut new_p = char_profile(&new_dir, 1001);
        if let ProfileFile::Character { modified, .. } = &mut old_p {
            *modified = Some(100);
        }
        if let ProfileFile::Character { modified, .. } = &mut new_p {
            *modified = Some(900);
        }

        // Stale-first and stale-last must both resolve to the newest.
        for profiles in [
            vec![old_p.clone(), new_p.clone()],
            vec![new_p.clone(), old_p.clone()],
        ] {
            let found = find_character_file(&profiles, 1001).unwrap();
            assert_eq!(found.settings_dir(), new_dir.as_path());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_target_write_is_reported_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        // Three targets; the middle one's dir is read-only. The other
        // two must still be written and the failure must appear in the
        // report - not vanish into a bare io error.
        let tmp = TempDir::new().unwrap();
        let src_dir = settings_dir(tmp.path(), "settings_Src");
        let src = write_char(&src_dir, 1, "SRC");
        let a = settings_dir(tmp.path(), "settings_A");
        let b = settings_dir(tmp.path(), "settings_B");
        write_char(&a, 2, "old");
        write_char(&b, 3, "old");
        let profiles = vec![
            char_profile(&src_dir, 1),
            char_profile(&a, 2),
            char_profile(&b, 3),
        ];
        let locked = a.join("core_char_2.dat");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();

        let report = replicate(1, &src, None, &[2, 3], &profiles).unwrap();

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(report.written.len(), 1, "{report:?}");
        assert_eq!(read(&b.join("core_char_3.dat")), "SRC");
        assert!(
            report.skipped.iter().any(|s| s.contains("write failed")),
            "the failed write must be in the report: {:?}",
            report.skipped
        );
    }

    #[test]
    fn user_file_for_user_id_requires_the_file_to_exist() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_user(&dir, 9001, "a");

        assert_eq!(
            user_file_for_user_id(&dir, 9001),
            Some(dir.join("core_user_9001.dat"))
        );
        assert_eq!(user_file_for_user_id(&dir, 9999), None);
    }

    #[test]
    fn user_file_for_user_id_rejects_a_directory_with_the_right_name() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        std::fs::create_dir(dir.join("core_user_9001.dat")).unwrap();

        assert_eq!(
            user_file_for_user_id(&dir, 9001),
            None,
            "a directory is not a copyable user file"
        );
    }

    // ------------------- target_paths_for_character -------------------

    #[test]
    fn target_paths_spans_every_profile_the_character_appears_in() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let d2 = settings_dir(tmp.path(), "settings_Alt");
        let profiles = vec![char_profile(&d1, 2002), char_profile(&d2, 2002)];

        let targets = target_paths_for_character(&profiles, 2002);

        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&(d1.clone(), d1.join("core_char_2002.dat"))));
        assert!(targets.contains(&(d2.clone(), d2.join("core_char_2002.dat"))));
    }

    #[test]
    fn target_paths_dedups_repeated_entries_for_one_dir() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![char_profile(&d1, 2002), char_profile(&d1, 2002)];

        assert_eq!(target_paths_for_character(&profiles, 2002).len(), 1);
    }

    #[test]
    fn target_paths_ignores_user_files_sharing_the_id() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![user_profile(&d1, 2002)];

        assert!(
            target_paths_for_character(&profiles, 2002).is_empty(),
            "a User profile must never be treated as a character write target"
        );
    }

    #[test]
    fn target_paths_is_empty_for_an_unknown_character() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![char_profile(&d1, 2002)];

        assert!(target_paths_for_character(&profiles, 4004).is_empty());
    }

    // ------------------------- find_character_file --------------------

    #[test]
    fn find_character_file_prefers_character_over_same_id_user() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![user_profile(&d1, 7007), char_profile(&d1, 7007)];

        let found = find_character_file(&profiles, 7007).expect("should find the character");
        assert!(matches!(found, ProfileFile::Character { .. }));
    }

    #[test]
    fn find_character_file_returns_none_when_absent() {
        assert!(find_character_file(&[], 1).is_none());
    }

    // ------------------------- write_profile_bytes --------------------

    #[test]
    fn write_profile_bytes_creates_missing_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("a/b/c/core_char_1.dat");

        write_profile_bytes(&dest, b"payload").unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
    }

    #[test]
    fn write_profile_bytes_truncates_a_longer_existing_file() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("core_char_1.dat");
        std::fs::write(&dest, "a very long previous value").unwrap();

        write_profile_bytes(&dest, b"short").unwrap();

        assert_eq!(
            read(&dest),
            "short",
            "stale trailing bytes would corrupt the file"
        );
    }

    // ---------------------------- replicate ---------------------------

    /// The standard fixture: one profile dir holding a source
    /// character (1001, whose account is user 9001), two target
    /// characters, a second account's user file, and the placeholder.
    struct Fixture {
        _tmp: TempDir,
        dir: PathBuf,
        profiles: Vec<ProfileFile>,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "SOURCE_CHAR");
        write_char(&dir, 2002, "target A original");
        write_char(&dir, 3003, "target B original");
        write_user(&dir, 9001, "SOURCE_USER");
        write_user(&dir, 9002, "other account original");
        write_user_placeholder(&dir);

        let profiles = vec![
            char_profile(&dir, 1001),
            char_profile(&dir, 2002),
            char_profile(&dir, 3003),
            user_profile(&dir, 9001),
            user_profile(&dir, 9002),
        ];
        Fixture {
            _tmp: tmp,
            dir,
            profiles,
        }
    }

    #[test]
    fn replicate_writes_source_char_bytes_to_every_target() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let report =
            replicate(1001, &src_char, Some(&src_user), &[2002, 3003], &f.profiles).unwrap();

        assert_eq!(read(&f.dir.join("core_char_2002.dat")), "SOURCE_CHAR");
        assert_eq!(read(&f.dir.join("core_char_3003.dat")), "SOURCE_CHAR");
        assert!(
            report.skipped.is_empty(),
            "unexpected skips: {:?}",
            report.skipped
        );
    }

    #[test]
    fn replicate_never_modifies_the_source_character_file() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        replicate(1001, &src_char, Some(&src_user), &[2002, 3003], &f.profiles).unwrap();

        assert_eq!(
            read(&src_char),
            "SOURCE_CHAR",
            "the source is the one file a copy must never touch"
        );
    }

    #[test]
    fn replicate_never_modifies_the_source_user_file() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        replicate(1001, &src_char, Some(&src_user), &[2002], &f.profiles).unwrap();

        assert_eq!(read(&src_user), "SOURCE_USER");
    }

    #[test]
    fn replicate_blankets_the_source_user_bytes_over_other_accounts() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        replicate(1001, &src_char, Some(&src_user), &[2002], &f.profiles).unwrap();

        // This wide blast radius is deliberate - the user file carries
        // window positions / overview / chat, and EVE keys it by
        // account, not character.
        assert_eq!(read(&f.dir.join("core_user_9002.dat")), "SOURCE_USER");
    }

    #[test]
    fn replicate_leaves_the_user_placeholder_alone() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        replicate(1001, &src_char, Some(&src_user), &[2002], &f.profiles).unwrap();

        assert_eq!(read(&f.dir.join("core_user__.dat")), "placeholder");
    }

    #[test]
    fn replicate_drops_the_source_from_its_own_target_list() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let report = replicate(1001, &src_char, Some(&src_user), &[1001], &f.profiles).unwrap();

        assert!(
            report.written.is_empty(),
            "copying a character onto itself must be a no-op, got {:?}",
            report.written
        );
        assert_eq!(read(&src_char), "SOURCE_CHAR");
        // No dir was touched in pass 1, so the blanket user pass must
        // not run either.
        assert_eq!(
            read(&f.dir.join("core_user_9002.dat")),
            "other account original"
        );
    }

    #[test]
    fn replicate_reports_targets_with_no_profile_and_writes_nothing_for_them() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let report =
            replicate(1001, &src_char, Some(&src_user), &[2002, 8888], &f.profiles).unwrap();

        assert_eq!(
            report.skipped,
            vec!["target 8888 has no existing profile file".to_string()]
        );
        assert_eq!(read(&f.dir.join("core_char_2002.dat")), "SOURCE_CHAR");
        assert!(!f.dir.join("core_char_8888.dat").exists());
    }

    #[test]
    fn replicate_without_a_user_mapping_still_copies_char_files_and_explains_itself() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");

        let report = replicate(1001, &src_char, None, &[2002], &f.profiles).unwrap();

        assert_eq!(read(&f.dir.join("core_char_2002.dat")), "SOURCE_CHAR");
        assert_eq!(
            read(&f.dir.join("core_user_9002.dat")),
            "other account original",
            "with no known source user file, user files must be left untouched"
        );
        assert_eq!(report.skipped.len(), 1);
        assert!(
            report.skipped[0].contains("no launcher-log mapping"),
            "the skip message should tell the user how to fix it, got: {}",
            report.skipped[0]
        );
    }

    #[test]
    fn replicate_spans_every_profile_dir_a_target_lives_in() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let d2 = settings_dir(tmp.path(), "settings_Alt");
        write_char(&d1, 1001, "SOURCE_CHAR");
        write_user(&d1, 9001, "SOURCE_USER");
        write_char(&d1, 2002, "old");
        write_char(&d2, 2002, "old");
        write_user(&d2, 9003, "alt account original");

        let profiles = vec![
            char_profile(&d1, 1001),
            char_profile(&d1, 2002),
            char_profile(&d2, 2002),
        ];

        replicate(
            1001,
            &d1.join("core_char_1001.dat"),
            Some(&d1.join("core_user_9001.dat")),
            &[2002],
            &profiles,
        )
        .unwrap();

        assert_eq!(read(&d1.join("core_char_2002.dat")), "SOURCE_CHAR");
        assert_eq!(read(&d2.join("core_char_2002.dat")), "SOURCE_CHAR");
        // d2 was touched by pass 1, so its user files get the blanket too.
        assert_eq!(read(&d2.join("core_user_9003.dat")), "SOURCE_USER");
    }

    #[test]
    fn replicate_leaves_untouched_profile_dirs_completely_alone() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let bystander = settings_dir(tmp.path(), "settings_Bystander");
        write_char(&d1, 1001, "SOURCE_CHAR");
        write_user(&d1, 9001, "SOURCE_USER");
        write_char(&d1, 2002, "old");
        write_user(&bystander, 9004, "bystander original");
        write_char(&bystander, 5005, "bystander char");

        let profiles = vec![
            char_profile(&d1, 1001),
            char_profile(&d1, 2002),
            char_profile(&bystander, 5005),
        ];

        replicate(
            1001,
            &d1.join("core_char_1001.dat"),
            Some(&d1.join("core_user_9001.dat")),
            &[2002],
            &profiles,
        )
        .unwrap();

        assert_eq!(
            read(&bystander.join("core_user_9004.dat")),
            "bystander original",
            "a profile dir with no targeted character must not be rewritten"
        );
        assert_eq!(
            read(&bystander.join("core_char_5005.dat")),
            "bystander char"
        );
    }

    #[test]
    fn replicate_report_lists_exactly_the_files_it_wrote() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let report =
            replicate(1001, &src_char, Some(&src_user), &[2002, 3003], &f.profiles).unwrap();

        let mut got = report.written.clone();
        got.sort();
        assert_eq!(
            got,
            vec![
                f.dir.join("core_char_2002.dat"),
                f.dir.join("core_char_3003.dat"),
                f.dir.join("core_user_9002.dat"),
            ]
        );
    }

    #[test]
    fn replicate_is_deterministic_across_runs() {
        // HashSet iteration order used to make `written` vary run to
        // run, which made the UI's "copied N files" list unstable.
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let a = replicate(1001, &src_char, Some(&src_user), &[3003, 2002], &f.profiles).unwrap();
        let b = replicate(1001, &src_char, Some(&src_user), &[2002, 3003], &f.profiles).unwrap();

        assert_eq!(a, b, "report order must not depend on hash iteration order");
    }

    #[test]
    fn replicate_dedups_a_repeated_target_id() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let report = replicate(
            1001,
            &src_char,
            Some(&src_user),
            &[2002, 2002, 2002],
            &f.profiles,
        )
        .unwrap();

        let char_writes = report
            .written
            .iter()
            .filter(|p| p.ends_with("core_char_2002.dat"))
            .count();
        assert_eq!(char_writes, 1);
    }

    #[test]
    fn replicate_fails_loudly_when_the_source_char_file_is_missing() {
        let f = fixture();
        let missing = f.dir.join("core_char_404.dat");

        let err = replicate(1001, &missing, None, &[2002], &f.profiles).unwrap_err();

        assert!(
            matches!(err, AppError::Io(_)),
            "a missing source must abort before any target is written, got {err:?}"
        );
        assert_eq!(
            read(&f.dir.join("core_char_2002.dat")),
            "target A original",
            "no target may be modified when the source read fails"
        );
    }

    #[test]
    fn replicate_with_no_targets_does_nothing() {
        let f = fixture();
        let src_char = f.dir.join("core_char_1001.dat");
        let src_user = f.dir.join("core_user_9001.dat");

        let report = replicate(1001, &src_char, Some(&src_user), &[], &f.profiles).unwrap();

        assert!(report.written.is_empty());
        assert_eq!(
            read(&f.dir.join("core_user_9002.dat")),
            "other account original"
        );
    }

    // ---------------------- apply_template_bytes ----------------------

    #[test]
    fn apply_template_writes_char_bytes_and_blankets_user_files() {
        let f = fixture();

        let report =
            apply_template_bytes(b"TPL_CHAR", Some(b"TPL_USER"), &[2002, 3003], &f.profiles)
                .unwrap();

        assert_eq!(read(&f.dir.join("core_char_2002.dat")), "TPL_CHAR");
        assert_eq!(read(&f.dir.join("core_char_3003.dat")), "TPL_CHAR");
        // Templates have no live source to protect, so every account's
        // user file in the touched dirs gets the bytes.
        assert_eq!(read(&f.dir.join("core_user_9001.dat")), "TPL_USER");
        assert_eq!(read(&f.dir.join("core_user_9002.dat")), "TPL_USER");
        assert_eq!(read(&f.dir.join("core_user__.dat")), "placeholder");
        assert!(report.skipped.is_empty(), "got {:?}", report.skipped);
    }

    #[test]
    fn apply_template_without_user_bytes_leaves_user_files_alone_and_says_so() {
        let f = fixture();

        let report = apply_template_bytes(b"TPL_CHAR", None, &[2002], &f.profiles).unwrap();

        assert_eq!(read(&f.dir.join("core_char_2002.dat")), "TPL_CHAR");
        assert_eq!(read(&f.dir.join("core_user_9001.dat")), "SOURCE_USER");
        assert_eq!(
            read(&f.dir.join("core_user_9002.dat")),
            "other account original"
        );
        assert_eq!(report.skipped.len(), 1);
        assert!(
            report.skipped[0].contains("no account-level settings"),
            "the report must say why the layout did not move, got: {}",
            report.skipped[0]
        );
    }

    #[test]
    fn apply_template_reports_unknown_targets_and_dedups_repeats() {
        let f = fixture();

        let report = apply_template_bytes(
            b"TPL_CHAR",
            Some(b"TPL_USER"),
            &[2002, 2002, 8888],
            &f.profiles,
        )
        .unwrap();

        assert_eq!(
            report.skipped,
            vec!["target 8888 has no existing profile file".to_string()]
        );
        let char_writes = report
            .written
            .iter()
            .filter(|p| p.ends_with("core_char_2002.dat"))
            .count();
        assert_eq!(char_writes, 1);
        assert!(!f.dir.join("core_char_8888.dat").exists());
    }

    #[test]
    fn apply_template_leaves_untouched_profile_dirs_alone() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let bystander = settings_dir(tmp.path(), "settings_Bystander");
        write_char(&d1, 2002, "old");
        write_user(&d1, 9001, "old user");
        write_char(&bystander, 5005, "bystander char");
        write_user(&bystander, 9004, "bystander user");
        let profiles = vec![char_profile(&d1, 2002), char_profile(&bystander, 5005)];

        apply_template_bytes(b"TPL_CHAR", Some(b"TPL_USER"), &[2002], &profiles).unwrap();

        assert_eq!(
            read(&bystander.join("core_char_5005.dat")),
            "bystander char"
        );
        assert_eq!(
            read(&bystander.join("core_user_9004.dat")),
            "bystander user"
        );
        assert_eq!(read(&d1.join("core_user_9001.dat")), "TPL_USER");
    }

    // ----------------------- selective replication --------------------

    /// Marshal-encoded settings fixture: {group -> Int marker}, so each
    /// group's provenance is checkable after a merge.
    fn marshal_file(entries: &[(&str, i64)]) -> Vec<u8> {
        use blue_marshal::{encode, EncodeOptions, Value};
        let dict = Value::Dict(
            entries
                .iter()
                .map(|(k, v)| (Value::Str((*k).to_string()), Value::Int(*v)))
                .collect(),
        );
        encode(&dict, &EncodeOptions::default()).unwrap()
    }

    fn group_val(bytes: &[u8], group: &str) -> Option<i64> {
        use blue_marshal::{decode, Value};
        let Value::Dict(items) = decode(bytes).unwrap().value else {
            panic!("not a dict");
        };
        items.iter().find_map(|(k, v)| match (k, v) {
            (Value::Str(s), Value::Int(i)) if s == group => Some(*i),
            _ => None,
        })
    }

    fn sel(chars: &[&str], users: &[&str]) -> CopySelection {
        CopySelection {
            char_groups: chars.iter().map(|s| s.to_string()).collect(),
            user_groups: users.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn selective_copy_merges_chosen_groups_and_keeps_the_rest() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        std::fs::write(
            dir.join("core_char_1001.dat"),
            marshal_file(&[("windows", 1), ("ui", 1)]),
        )
        .unwrap();
        std::fs::write(
            dir.join("core_char_2002.dat"),
            marshal_file(&[("windows", 2), ("ui", 2), ("notepad", 2)]),
        )
        .unwrap();
        let profiles = vec![char_profile(&dir, 1001), char_profile(&dir, 2002)];

        let report = replicate_with(
            1001,
            &dir.join("core_char_1001.dat"),
            None,
            &[2002],
            &profiles,
            Some(&sel(&["windows"], &[])),
            false,
        )
        .unwrap();

        let out = std::fs::read(dir.join("core_char_2002.dat")).unwrap();
        assert_eq!(
            group_val(&out, "windows"),
            Some(1),
            "selected group takes the source value"
        );
        assert_eq!(
            group_val(&out, "ui"),
            Some(2),
            "unselected group keeps the target value"
        );
        assert_eq!(
            group_val(&out, "notepad"),
            Some(2),
            "target-only groups survive"
        );
        assert_eq!(report.written.len(), 1);
        assert!(
            report.skipped.is_empty(),
            "empty user selection must not complain about the launcher map, got {:?}",
            report.skipped
        );
    }

    #[test]
    fn selective_copy_with_user_groups_merges_each_user_file_individually() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        std::fs::write(
            dir.join("core_char_1001.dat"),
            marshal_file(&[("windows", 1)]),
        )
        .unwrap();
        std::fs::write(
            dir.join("core_char_2002.dat"),
            marshal_file(&[("windows", 2)]),
        )
        .unwrap();
        std::fs::write(
            dir.join("core_user_9001.dat"),
            marshal_file(&[("overview", 1), ("audio", 1)]),
        )
        .unwrap();
        std::fs::write(
            dir.join("core_user_9002.dat"),
            marshal_file(&[("overview", 2), ("audio", 2), ("cmd", 2)]),
        )
        .unwrap();
        let profiles = vec![char_profile(&dir, 1001), char_profile(&dir, 2002)];

        replicate_with(
            1001,
            &dir.join("core_char_1001.dat"),
            Some(&dir.join("core_user_9001.dat")),
            &[2002],
            &profiles,
            Some(&sel(&[], &["overview"])),
            false,
        )
        .unwrap();

        let ch = std::fs::read(dir.join("core_char_2002.dat")).unwrap();
        assert_eq!(
            group_val(&ch, "windows"),
            Some(2),
            "no char groups selected: char file untouched"
        );
        let u = std::fs::read(dir.join("core_user_9002.dat")).unwrap();
        assert_eq!(group_val(&u, "overview"), Some(1), "selected: synced");
        assert_eq!(group_val(&u, "audio"), Some(2), "unselected: kept");
        assert_eq!(group_val(&u, "cmd"), Some(2), "target-only: kept");
        let su = std::fs::read(dir.join("core_user_9001.dat")).unwrap();
        assert_eq!(
            group_val(&su, "audio"),
            Some(1),
            "the source's own user file is never touched"
        );
    }

    #[test]
    fn selective_copy_skips_an_unparseable_target_and_leaves_it_alone() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        std::fs::write(
            dir.join("core_char_1001.dat"),
            marshal_file(&[("windows", 1)]),
        )
        .unwrap();
        write_char(&dir, 2002, "not marshal at all");
        let profiles = vec![char_profile(&dir, 1001), char_profile(&dir, 2002)];

        let report = replicate_with(
            1001,
            &dir.join("core_char_1001.dat"),
            None,
            &[2002],
            &profiles,
            Some(&sel(&["windows"], &[])),
            false,
        )
        .unwrap();

        assert_eq!(
            read(&dir.join("core_char_2002.dat")),
            "not marshal at all",
            "an unparseable target must never be overwritten in selective mode"
        );
        assert!(report.written.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].contains("selective copy failed"));
    }

    #[test]
    fn copies_stay_on_the_sources_server_unless_told_otherwise() {
        // A character mirrored onto Singularity shares its id with the
        // Tranquility original; a copy must not silently rewrite the
        // test-server profile (or vice versa).
        let tmp = TempDir::new().unwrap();
        let tq = tmp.path().join("EVE/c_ccp_eve_tq_tranquility");
        let sisi = tmp.path().join("EVE/c_ccp_eve_sisi_singularity");
        let tq_dir = settings_dir(&tq, "settings_Default");
        let sisi_dir = settings_dir(&sisi, "settings_Default");
        write_char(&tq_dir, 1001, "TQ SOURCE");
        write_char(&tq_dir, 2002, "tq target");
        write_char(&sisi_dir, 2002, "sisi target");
        let profiles = vec![
            char_profile(&tq_dir, 1001),
            char_profile(&tq_dir, 2002),
            char_profile(&sisi_dir, 2002),
        ];

        let report = replicate(
            1001,
            &tq_dir.join("core_char_1001.dat"),
            None,
            &[2002],
            &profiles,
        )
        .unwrap();

        assert_eq!(read(&tq_dir.join("core_char_2002.dat")), "TQ SOURCE");
        assert_eq!(
            read(&sisi_dir.join("core_char_2002.dat")),
            "sisi target",
            "the Singularity profile must survive a Tranquility copy"
        );
        assert!(report.skipped.iter().any(|s| s.contains("cross-server")));

        // Explicit opt-in crosses.
        replicate_with(
            1001,
            &tq_dir.join("core_char_1001.dat"),
            None,
            &[2002],
            &profiles,
            None,
            true,
        )
        .unwrap();
        assert_eq!(read(&sisi_dir.join("core_char_2002.dat")), "TQ SOURCE");
    }

    #[test]
    fn selective_template_apply_keeps_unselected_groups() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        std::fs::write(
            dir.join("core_char_2002.dat"),
            marshal_file(&[("shiptheme", 2), ("ui", 2)]),
        )
        .unwrap();
        std::fs::write(
            dir.join("core_user_9002.dat"),
            marshal_file(&[("overview", 2), ("audio", 2)]),
        )
        .unwrap();
        let profiles = vec![char_profile(&dir, 2002)];

        let tpl_char = marshal_file(&[("shiptheme", 7), ("ui", 7)]);
        let tpl_user = marshal_file(&[("overview", 7), ("audio", 7)]);

        apply_template_bytes_with(
            &tpl_char,
            Some(&tpl_user),
            &[2002],
            &profiles,
            Some(&sel(&["shiptheme"], &["overview"])),
            None,
            false,
        )
        .unwrap();

        let ch = std::fs::read(dir.join("core_char_2002.dat")).unwrap();
        assert_eq!(group_val(&ch, "shiptheme"), Some(7));
        assert_eq!(group_val(&ch, "ui"), Some(2));
        let u = std::fs::read(dir.join("core_user_9002.dat")).unwrap();
        assert_eq!(group_val(&u, "overview"), Some(7));
        assert_eq!(group_val(&u, "audio"), Some(2));
    }

    #[test]
    fn template_apply_stays_on_its_server_unless_told_otherwise() {
        // Copies had this guard; templates applied to every server a
        // target lived on, so a Tranquility doctrine silently rewrote
        // the Singularity profile too.
        let tmp = TempDir::new().unwrap();
        let tq_dir = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_tq_tranquility"),
            "settings_Default",
        );
        let sisi_dir = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_sisi_singularity"),
            "settings_Default",
        );
        write_char(&tq_dir, 2002, "tq target");
        write_char(&sisi_dir, 2002, "sisi target");
        let profiles = vec![char_profile(&tq_dir, 2002), char_profile(&sisi_dir, 2002)];

        let report = apply_template_bytes_with(
            b"TPL",
            None,
            &[2002],
            &profiles,
            None,
            Some("tranquility"),
            false,
        )
        .unwrap();

        assert_eq!(read(&tq_dir.join("core_char_2002.dat")), "TPL");
        assert_eq!(read(&sisi_dir.join("core_char_2002.dat")), "sisi target");
        assert!(report.skipped.iter().any(|s| s.contains("cross-server")));

        // No recorded server: goes everywhere, as older templates did.
        apply_template_bytes_with(b"TPL2", None, &[2002], &profiles, None, None, false).unwrap();
        assert_eq!(read(&sisi_dir.join("core_char_2002.dat")), "TPL2");

        // Explicit opt-in crosses.
        apply_template_bytes_with(
            b"TPL3",
            None,
            &[2002],
            &profiles,
            None,
            Some("tranquility"),
            true,
        )
        .unwrap();
        assert_eq!(read(&sisi_dir.join("core_char_2002.dat")), "TPL3");
    }
}
