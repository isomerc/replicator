//! Portable zip export/import: the offline complement to the git
//! remote. One archive holds every profile's settings files in the
//! same layout as the mirror, plus a checksummed manifest, so a setup
//! can be handed to a corpmate or carried to another machine without a
//! PAT or repository. A second archive kind carries a single template.
//!
//! Import verifies every checksum before writing anything: a corrupt
//! archive is a hard error with zero files touched, never a partial
//! write.

use crate::error::{AppError, AppResult};
use crate::eve::characters::ProfileFile;
use crate::eve::ops::CopyReport;
use crate::git_mirror::{profile_dir_label, sanitize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Bumped only when an older app could misread the archive. Newer
/// formats are refused with an "update Replicator" error rather than
/// guessed at.
pub const FORMAT: u32 = 1;
const APP: &str = "replicator";
const MANIFEST_NAME: &str = "manifest.json";
const TEMPLATE_CHAR: &str = "template/char.dat";
const TEMPLATE_USER: &str = "template/user.dat";

pub const KIND_SETUP: &str = "setup";
pub const KIND_TEMPLATE: &str = "template";

/// A settings file runs to a few hundred KB; an archive entry claiming
/// more than this is not one of ours. Checked against the manifest
/// before a byte is read, so a hostile zip cannot inflate into memory.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub format: u32,
    pub app: String,
    pub app_version: String,
    /// "setup" or "template".
    pub kind: String,
    pub created_at: i64,
    pub characters: Vec<CharacterRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateMeta>,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CharacterRef {
    pub id: i64,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TemplateMeta {
    pub name: String,
    pub source_character_id: Option<i64>,
    pub source_name: Option<String>,
    pub has_user_data: bool,
    /// Added after format 1 shipped; absent in older archives, which
    /// deserialize to None and apply anywhere, as they always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_server: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// What an import would do, shown to the user before anything is
/// written. For template archives `would_write` is empty: importing a
/// template only adds to the shelf.
#[derive(Debug, Serialize)]
pub struct ImportPreview {
    pub kind: String,
    pub app_version: String,
    pub created_at: i64,
    pub characters: Vec<CharacterRef>,
    pub file_count: usize,
    pub template: Option<TemplateMeta>,
    pub would_write: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ExportSummary {
    pub files: usize,
    pub characters: usize,
}

fn zerr(e: zip::result::ZipError) -> AppError {
    AppError::Other(format!("zip: {e}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Export every live settings file into `zip_path`, in the mirror's
/// `characters/<profile>/<label>_<id>.dat` / `users/<profile>/
/// user_<id>.dat` layout. Two settings dirs sharing a profile label
/// collapse to one entry (last wins), matching the mirror.
pub fn export_setup(
    zip_path: &Path,
    profiles: &[ProfileFile],
    names: &HashMap<i64, String>,
    now: i64,
) -> AppResult<ExportSummary> {
    let mut order: Vec<String> = Vec::new();
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();
    let mut characters: Vec<CharacterRef> = Vec::new();

    for p in profiles {
        let profile = profile_dir_label(p.settings_dir());
        let bytes = std::fs::read(p.path())?;
        let entry_path = match p {
            ProfileFile::Character { id, .. } => {
                if !characters.iter().any(|c| c.id == *id) {
                    characters.push(CharacterRef {
                        id: *id,
                        name: names.get(id).cloned(),
                    });
                }
                let label = names
                    .get(id)
                    .map(|n| sanitize(n))
                    .unwrap_or_else(|| id.to_string());
                format!("characters/{profile}/{label}_{id}.dat")
            }
            ProfileFile::User { id, .. } => format!("users/{profile}/user_{id}.dat"),
        };
        if !blobs.contains_key(&entry_path) {
            order.push(entry_path.clone());
        }
        blobs.insert(entry_path, bytes);
    }

    if order.is_empty() {
        return Err(AppError::Other("no settings files found to export".into()));
    }
    characters.sort_by_key(|c| c.id);

    let files: Vec<FileEntry> = order
        .iter()
        .map(|p| FileEntry {
            path: p.clone(),
            sha256: sha256_hex(&blobs[p]),
            size: blobs[p].len() as u64,
        })
        .collect();

    let file_count = files.len();
    let character_count = characters.len();
    let manifest = Manifest {
        format: FORMAT,
        app: APP.into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        kind: KIND_SETUP.into(),
        created_at: now,
        characters,
        template: None,
        files,
    };

    write_archive(zip_path, &manifest, &order, &blobs)?;
    Ok(ExportSummary {
        files: file_count,
        characters: character_count,
    })
}

/// Export one template as a standalone shareable archive.
pub fn export_template(
    zip_path: &Path,
    meta: &TemplateMeta,
    char_bytes: &[u8],
    user_bytes: Option<&[u8]>,
    now: i64,
) -> AppResult<()> {
    let mut order = vec![TEMPLATE_CHAR.to_string()];
    let mut blobs = HashMap::from([(TEMPLATE_CHAR.to_string(), char_bytes.to_vec())]);
    if let Some(u) = user_bytes {
        order.push(TEMPLATE_USER.to_string());
        blobs.insert(TEMPLATE_USER.to_string(), u.to_vec());
    }

    let files = order
        .iter()
        .map(|p| FileEntry {
            path: p.clone(),
            sha256: sha256_hex(&blobs[p]),
            size: blobs[p].len() as u64,
        })
        .collect();

    let characters = meta
        .source_character_id
        .map(|id| {
            vec![CharacterRef {
                id,
                name: meta.source_name.clone(),
            }]
        })
        .unwrap_or_default();

    let manifest = Manifest {
        format: FORMAT,
        app: APP.into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        kind: KIND_TEMPLATE.into(),
        created_at: now,
        characters,
        template: Some(TemplateMeta {
            has_user_data: user_bytes.is_some(),
            ..meta.clone()
        }),
        files,
    };

    write_archive(zip_path, &manifest, &order, &blobs)
}

fn write_archive(
    zip_path: &Path,
    manifest: &Manifest,
    order: &[String],
    blobs: &HashMap<String, Vec<u8>>,
) -> AppResult<()> {
    let file = std::fs::File::create(zip_path)?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(MANIFEST_NAME, opts).map_err(zerr)?;
    zip.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    for path in order {
        zip.start_file(path.as_str(), opts).map_err(zerr)?;
        zip.write_all(&blobs[path])?;
    }
    zip.finish().map_err(zerr)?;
    Ok(())
}

/// Open an archive, parse its manifest and verify every listed file's
/// checksum. Nothing downstream touches a byte that wasn't verified
/// here, so a truncated download or bit-rotted file dies before any
/// write.
fn open_verified(zip_path: &Path) -> AppResult<(Manifest, HashMap<String, Vec<u8>>)> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(zerr)?;

    let manifest: Manifest = {
        let entry = archive.by_name(MANIFEST_NAME).map_err(|_| {
            AppError::Other("not a Replicator archive (no manifest.json inside)".into())
        })?;
        serde_json::from_reader(entry.take(MAX_MANIFEST_BYTES))?
    };
    if manifest.app != APP {
        return Err(AppError::Other(
            "not a Replicator archive (manifest belongs to another app)".into(),
        ));
    }
    if manifest.format > FORMAT {
        return Err(AppError::Other(format!(
            "archive format {} is newer than this Replicator understands - update the app",
            manifest.format
        )));
    }

    let mut blobs = HashMap::new();
    for f in &manifest.files {
        if f.size > MAX_ENTRY_BYTES {
            return Err(AppError::Other(format!(
                "{}: {} bytes is larger than any settings file; refusing the archive",
                f.path, f.size
            )));
        }
        let entry = archive.by_name(&f.path).map_err(|_| {
            AppError::Other(format!(
                "{}: listed in the manifest but missing from the archive",
                f.path
            ))
        })?;
        // The manifest's size is the read budget; one byte more than it
        // is enough to notice an entry that outgrew its own listing.
        let mut bytes = Vec::with_capacity(f.size as usize);
        entry.take(f.size + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != f.size {
            return Err(AppError::Other(format!(
                "{}: size mismatch - the archive is corrupt, nothing was written",
                f.path
            )));
        }
        if sha256_hex(&bytes) != f.sha256 {
            return Err(AppError::Other(format!(
                "{}: checksum mismatch - the archive is corrupt, nothing was written",
                f.path
            )));
        }
        blobs.insert(f.path.clone(), bytes);
    }
    Ok((manifest, blobs))
}

/// The archive entry a live profile file should be filled from, using
/// the same semantics as the mirror restore: exact-or-suffix match in
/// `root/<profile>/`, falling back to the flat `root/` layout.
fn entry_for<'a>(
    paths: &'a [String],
    root: &str,
    profile: &str,
    suffix_or_eq: &str,
) -> Option<&'a str> {
    let in_profile = format!("{root}/{profile}/");
    let matches = |prefix: &str, path: &str| {
        path.strip_prefix(prefix)
            .filter(|rest| !rest.contains('/'))
            .is_some_and(|rest| rest == suffix_or_eq || rest.ends_with(suffix_or_eq))
    };
    paths
        .iter()
        .find(|p| matches(&in_profile, p))
        .or_else(|| {
            let flat = format!("{root}/");
            paths.iter().find(|p| matches(&flat, p))
        })
        .map(|s| s.as_str())
}

/// Pair every live profile file with its archive entry. Returns the
/// matches plus skip notes for both directions: live files the archive
/// doesn't cover, and archive entries nothing on this machine claims.
fn match_profiles<'p, 'm>(
    manifest: &'m Manifest,
    profiles: &'p [ProfileFile],
) -> (Vec<(&'p ProfileFile, &'m str)>, Vec<String>) {
    let paths: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    let mut matches: Vec<(&ProfileFile, &str)> = Vec::new();
    let mut skipped = Vec::new();
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();

    for p in profiles {
        let profile = profile_dir_label(p.settings_dir());
        let found = match p {
            ProfileFile::Character { id, .. } => {
                entry_for(&paths, "characters", &profile, &format!("_{id}.dat"))
            }
            ProfileFile::User { id, .. } => {
                entry_for(&paths, "users", &profile, &format!("user_{id}.dat"))
            }
        };
        match found {
            Some(zip_entry) => {
                // Borrow the manifest's own string so the zip path
                // outlives the local `paths` scratch list.
                let owned = manifest
                    .files
                    .iter()
                    .find(|f| f.path == zip_entry)
                    .expect("matched path comes from the manifest");
                claimed.insert(owned.path.clone());
                matches.push((p, owned.path.as_str()));
            }
            None => skipped.push(match p {
                ProfileFile::Character { id, .. } => format!("character {id} not in archive"),
                ProfileFile::User { id, .. } => format!("user {id} not in archive"),
            }),
        }
    }

    for f in &manifest.files {
        if !claimed.contains(&f.path) {
            skipped.push(format!("{}: no matching profile on this machine", f.path));
        }
    }
    (matches, skipped)
}

/// Inspect an archive without writing anything.
pub fn preview(zip_path: &Path, profiles: &[ProfileFile]) -> AppResult<ImportPreview> {
    let (manifest, _blobs) = open_verified(zip_path)?;
    let (would_write, skipped) = if manifest.kind == KIND_SETUP {
        let (matches, skipped) = match_profiles(&manifest, profiles);
        (
            matches
                .iter()
                .map(|(p, _)| p.path().display().to_string())
                .collect(),
            skipped,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(ImportPreview {
        kind: manifest.kind,
        app_version: manifest.app_version,
        created_at: manifest.created_at,
        characters: manifest.characters,
        file_count: manifest.files.len(),
        template: manifest.template,
        would_write,
        skipped,
    })
}

/// Write a verified setup archive over the live settings files it
/// matches. The caller is responsible for the EVE-running gate and the
/// snapshot-before/after bracket, same as a restore.
pub fn import_setup(zip_path: &Path, profiles: &[ProfileFile]) -> AppResult<CopyReport> {
    let (manifest, blobs) = open_verified(zip_path)?;
    if manifest.kind != KIND_SETUP {
        return Err(AppError::Other(
            "this archive holds a template - import it from the Templates shelf".into(),
        ));
    }
    let (matches, skipped) = match_profiles(&manifest, profiles);
    let mut report = CopyReport {
        skipped,
        ..Default::default()
    };
    // Per-file failures land in the report, not in an error: a
    // read-only file on target four must not hide that one through
    // three were written.
    for (p, zip_entry) in matches {
        match crate::eve::ops::write_profile_bytes(p.path(), &blobs[zip_entry]) {
            Ok(()) => report.written.push(p.path().to_path_buf()),
            Err(e) => report
                .skipped
                .push(format!("{}: write failed ({e})", p.path().display())),
        }
    }
    Ok(report)
}

/// Read a verified template archive back into its meta and blobs.
pub fn read_template(zip_path: &Path) -> AppResult<(TemplateMeta, Vec<u8>, Option<Vec<u8>>)> {
    let (manifest, mut blobs) = open_verified(zip_path)?;
    if manifest.kind != KIND_TEMPLATE {
        return Err(AppError::Other(
            "this archive holds a full setup - import it from Version Control".into(),
        ));
    }
    let meta = manifest
        .template
        .ok_or_else(|| AppError::Other("template archive carries no template metadata".into()))?;
    let char_bytes = blobs
        .remove(TEMPLATE_CHAR)
        .ok_or_else(|| AppError::Other("template archive holds no character file".into()))?;
    let user_bytes = blobs.remove(TEMPLATE_USER);
    // The file is the truth; a flag that disagrees with the archive's
    // contents was wrong at export time.
    let meta = TemplateMeta {
        has_user_data: user_bytes.is_some(),
        ..meta
    };
    Ok((meta, char_bytes, user_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn names(pairs: &[(i64, &str)]) -> HashMap<i64, String> {
        pairs.iter().map(|(i, n)| (*i, n.to_string())).collect()
    }

    /// A machine: a temp root with one `settings_Default` profile
    /// holding char 1001 and user 9001.
    fn machine(char_v: &str, user_v: &str) -> (TempDir, PathBuf, Vec<ProfileFile>) {
        let tmp = TempDir::new().unwrap();
        let live = settings_dir(tmp.path(), "settings_Default");
        write_char(&live, 1001, char_v);
        write_user(&live, 9001, user_v);
        let profiles = vec![char_profile(&live, 1001), user_profile(&live, 9001)];
        (tmp, live, profiles)
    }

    fn tamper(zip_path: &Path, victim: &str) {
        let mut archive = ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
        let mut entries = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let name = f.name().to_string();
            let mut b = Vec::new();
            f.read_to_end(&mut b).unwrap();
            entries.push((name, b));
        }
        drop(archive);
        let mut w = ZipWriter::new(std::fs::File::create(zip_path).unwrap());
        for (name, mut b) in entries {
            if name == victim {
                b[0] ^= 0xff;
            }
            w.start_file(name.as_str(), SimpleFileOptions::default())
                .unwrap();
            w.write_all(&b).unwrap();
        }
        w.finish().unwrap();
    }

    // ---------------------------- export ------------------------------

    #[test]
    fn export_writes_a_manifest_with_correct_checksums() {
        let (tmp, _live, profiles) = machine("char bytes", "user bytes");
        let zip = tmp.path().join("setup.zip");
        let summary = export_setup(&zip, &profiles, &names(&[(1001, "Alice")]), 42).unwrap();
        assert_eq!(summary.files, 2);
        assert_eq!(summary.characters, 1);

        let (manifest, blobs) = open_verified(&zip).unwrap();
        assert_eq!(manifest.format, FORMAT);
        assert_eq!(manifest.kind, KIND_SETUP);
        assert_eq!(manifest.created_at, 42);
        assert_eq!(manifest.characters.len(), 1);
        assert_eq!(manifest.characters[0].name.as_deref(), Some("Alice"));
        assert_eq!(
            blobs["characters/settings_Default/Alice_1001.dat"],
            b"char bytes"
        );
        assert_eq!(blobs["users/settings_Default/user_9001.dat"], b"user bytes");
    }

    #[test]
    fn export_with_no_profiles_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("setup.zip");
        assert!(export_setup(&zip, &[], &names(&[]), 0).is_err());
        assert!(!zip.exists(), "a failed export must not leave a file");
    }

    // ---------------------------- import ------------------------------

    #[test]
    fn export_then_import_restores_bytes_on_another_machine() {
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        // Names cached on machine A only: import must match by id
        // suffix, not by the label A happened to use.
        export_setup(&zip, &profiles_a, &names(&[(1001, "Alice")]), 0).unwrap();

        let (_b, live_b, profiles_b) = machine("WRECKED", "WRECKED");
        let report = import_setup(&zip, &profiles_b).unwrap();

        assert_eq!(report.written.len(), 2);
        assert!(report.skipped.is_empty());
        assert_eq!(read(&live_b.join("core_char_1001.dat")), "v1");
        assert_eq!(read(&live_b.join("core_user_9001.dat")), "u1");
    }

    #[test]
    fn import_skips_both_directions_and_says_why() {
        // Archive holds 1001+9001; the target machine has 1001 and a
        // stranger 3003. One file lands, one live file has no source,
        // one archive entry has no target.
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[]), 0).unwrap();

        let b = TempDir::new().unwrap();
        let live_b = settings_dir(b.path(), "settings_Default");
        write_char(&live_b, 1001, "old");
        write_char(&live_b, 3003, "stranger");
        let profiles_b = vec![char_profile(&live_b, 1001), char_profile(&live_b, 3003)];

        let report = import_setup(&zip, &profiles_b).unwrap();

        assert_eq!(report.written.len(), 1);
        assert_eq!(read(&live_b.join("core_char_1001.dat")), "v1");
        assert_eq!(read(&live_b.join("core_char_3003.dat")), "stranger");
        assert!(report
            .skipped
            .iter()
            .any(|s| s.contains("character 3003 not in archive")));
        assert!(report
            .skipped
            .iter()
            .any(|s| s.contains("user_9001.dat") && s.contains("no matching profile")));
    }

    #[test]
    fn a_tampered_file_fails_the_whole_import_before_any_write() {
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[]), 0).unwrap();
        tamper(&zip, "characters/settings_Default/1001_1001.dat");

        let (_b, live_b, profiles_b) = machine("KEEP", "KEEP");
        let err = import_setup(&zip, &profiles_b).unwrap_err();

        assert!(err.to_string().contains("checksum mismatch"), "{err}");
        assert_eq!(read(&live_b.join("core_char_1001.dat")), "KEEP");
        assert_eq!(read(&live_b.join("core_user_9001.dat")), "KEEP");
    }

    #[test]
    fn a_manifest_that_lies_about_a_size_is_rejected_before_any_write() {
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[]), 0).unwrap();
        let (mut manifest, blobs) = open_verified(&zip).unwrap();
        let order: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();

        manifest.files[0].size += 1;
        write_archive(&zip, &manifest, &order, &blobs).unwrap();
        let (_b, live_b, profiles_b) = machine("KEEP", "KEEP");
        let err = import_setup(&zip, &profiles_b).unwrap_err();
        assert!(err.to_string().contains("size mismatch"), "{err}");
        assert_eq!(read(&live_b.join("core_char_1001.dat")), "KEEP");

        // A size beyond anything EVE writes is refused before any read.
        manifest.files[0].size = MAX_ENTRY_BYTES + 1;
        write_archive(&zip, &manifest, &order, &blobs).unwrap();
        let err = import_setup(&zip, &profiles_b).unwrap_err();
        assert!(
            err.to_string().contains("larger than any settings file"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_import_write_is_reported_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[]), 0).unwrap();

        let (_b, live_b, profiles_b) = machine("WRECKED", "WRECKED");
        let locked = live_b.join("core_char_1001.dat");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();

        let report = import_setup(&zip, &profiles_b).unwrap();

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(report.written.len(), 1, "{report:?}");
        assert_eq!(read(&live_b.join("core_user_9001.dat")), "u1");
        assert!(
            report.skipped.iter().any(|s| s.contains("write failed")),
            "the failed write must be in the report: {:?}",
            report.skipped
        );
    }

    #[test]
    fn a_zip_without_a_manifest_is_rejected_with_a_plain_answer() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("random.zip");
        let mut w = ZipWriter::new(std::fs::File::create(&zip).unwrap());
        w.start_file("hello.txt", SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"hi").unwrap();
        w.finish().unwrap();

        let err = preview(&zip, &[]).unwrap_err();
        assert!(
            err.to_string().contains("not a Replicator archive"),
            "{err}"
        );
    }

    #[test]
    fn a_newer_format_is_refused_with_an_update_hint() {
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[]), 0).unwrap();

        // Rewrite the manifest claiming a future format.
        let (mut manifest, blobs) = open_verified(&zip).unwrap();
        manifest.format = FORMAT + 1;
        let order: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
        write_archive(&zip, &manifest, &order, &blobs).unwrap();

        let err = preview(&zip, &profiles_a).unwrap_err();
        assert!(err.to_string().contains("update the app"), "{err}");
    }

    #[test]
    fn cross_server_profiles_keep_their_prefix_and_do_not_cross() {
        // A Singularity profile exports under its prefixed label and
        // must not land on a Tranquility profile of the same basename.
        let a = TempDir::new().unwrap();
        let sisi = settings_dir(
            &a.path().join("c_ccp_eve_sisi_singularity"),
            "settings_Default",
        );
        write_char(&sisi, 1001, "sisi v1");
        let profiles_a = vec![char_profile(&sisi, 1001)];
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[]), 0).unwrap();

        let (manifest, _) = open_verified(&zip).unwrap();
        assert_eq!(
            manifest.files[0].path,
            "characters/singularity_settings_Default/1001_1001.dat"
        );

        // Tranquility-shaped machine: label mismatch, nothing written.
        let (_b, live_b, profiles_b) = machine("KEEP", "KEEP");
        let report = import_setup(&zip, &profiles_b).unwrap();
        assert!(report.written.is_empty());
        assert_eq!(read(&live_b.join("core_char_1001.dat")), "KEEP");

        // Same Singularity layout elsewhere: it lands.
        let c = TempDir::new().unwrap();
        let sisi_c = settings_dir(
            &c.path().join("c_ccp_eve_sisi_singularity"),
            "settings_Default",
        );
        write_char(&sisi_c, 1001, "WRECKED");
        let report = import_setup(&zip, &[char_profile(&sisi_c, 1001)]).unwrap();
        assert_eq!(report.written.len(), 1);
        assert_eq!(read(&sisi_c.join("core_char_1001.dat")), "sisi v1");
    }

    // ---------------------------- preview -----------------------------

    #[test]
    fn preview_reports_without_writing() {
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let zip = a.path().join("setup.zip");
        export_setup(&zip, &profiles_a, &names(&[(1001, "Alice")]), 7).unwrap();

        let (_b, live_b, profiles_b) = machine("KEEP", "KEEP");
        let p = preview(&zip, &profiles_b).unwrap();

        assert_eq!(p.kind, KIND_SETUP);
        assert_eq!(p.created_at, 7);
        assert_eq!(p.file_count, 2);
        assert_eq!(p.would_write.len(), 2);
        assert!(p.skipped.is_empty());
        assert_eq!(read(&live_b.join("core_char_1001.dat")), "KEEP");
    }

    // --------------------------- templates ----------------------------

    fn meta() -> TemplateMeta {
        TemplateMeta {
            name: "Doctrine".into(),
            source_character_id: Some(1001),
            source_name: Some("Alice".into()),
            has_user_data: true,
            source_server: Some("tranquility".into()),
        }
    }

    #[test]
    fn template_round_trips_meta_and_blobs() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("doctrine.zip");
        export_template(&zip, &meta(), b"char", Some(b"user"), 0).unwrap();

        let (m, c, u) = read_template(&zip).unwrap();
        assert_eq!(m.name, "Doctrine");
        assert_eq!(m.source_name.as_deref(), Some("Alice"));
        assert!(m.has_user_data);
        assert_eq!(m.source_server.as_deref(), Some("tranquility"));
        assert_eq!(c, b"char");
        assert_eq!(u.as_deref(), Some(&b"user"[..]));
    }

    #[test]
    fn a_char_only_template_round_trips_without_user_data() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("doctrine.zip");
        // The exporter corrects a stale has_user_data flag.
        export_template(&zip, &meta(), b"char", None, 0).unwrap();

        let (m, c, u) = read_template(&zip).unwrap();
        assert!(!m.has_user_data);
        assert_eq!(c, b"char");
        assert!(u.is_none());
    }

    #[test]
    fn archive_kinds_refuse_to_cross_wires() {
        let (a, _live_a, profiles_a) = machine("v1", "u1");
        let setup_zip = a.path().join("setup.zip");
        export_setup(&setup_zip, &profiles_a, &names(&[]), 0).unwrap();
        let tpl_zip = a.path().join("tpl.zip");
        export_template(&tpl_zip, &meta(), b"char", None, 0).unwrap();

        let err = read_template(&setup_zip).unwrap_err();
        assert!(err.to_string().contains("full setup"), "{err}");
        let err = import_setup(&tpl_zip, &profiles_a).unwrap_err();
        assert!(err.to_string().contains("Templates shelf"), "{err}");
    }

    #[test]
    fn a_tampered_template_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("doctrine.zip");
        export_template(&zip, &meta(), b"char", Some(b"user"), 0).unwrap();
        tamper(&zip, TEMPLATE_USER);

        let err = read_template(&zip).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
    }
}
