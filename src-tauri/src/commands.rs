use crate::db::{Group, Template};
use crate::error::{AppError, AppResult};
use crate::eve::characters::{enumerate, ProfileFile};
use crate::eve::esi;
use crate::eve::launcher;
use crate::eve::ops;
use crate::eve::ops::CopyReport;
use crate::eve::paths::{looks_like_settings_dir, EveInstall};
use crate::git_mirror;
use crate::keychain;
use crate::state::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

type S<'a> = State<'a, Arc<Mutex<AppState>>>;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Run a command body on a blocking worker. Sync Tauri commands execute
/// on the main thread, so anything that hits the network, parses
/// launcher logs or copies every profile file would freeze the window
/// for its duration. The cheap commands go through here too: they take
/// the state lock, and a sync command waiting on that lock while a push
/// or a first-run log parse holds it would freeze the window just the
/// same. Nothing in this module runs on the main thread.
async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Other(format!("background task failed: {e}")))?
}

/// Cached names for every character in `profiles`, for labeling mirror
/// files.
fn char_names_for(g: &AppState, profiles: &[ProfileFile]) -> AppResult<HashMap<i64, String>> {
    let ids: Vec<i64> = profiles
        .iter()
        .filter_map(|p| match p {
            ProfileFile::Character { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    g.db.lookup_names(&ids)
}

/// Refresh the char->user mapping from launcher logs. (The launcher
/// emits explicit `userId: X, characterId: Y` blocks; there is no
/// on-disk mapping inside the settings files themselves.)
fn refresh_launcher_map(g: &AppState) -> AppResult<()> {
    refresh_launcher_map_from(&g.db, &launcher::discover_log_dirs())
}

/// The incremental orchestration behind `refresh_launcher_map`, with
/// the log dirs injected so it can be tested against fixtures. The
/// watermark keeps re-parses incremental - logs reach multi-GB - and
/// must never move backwards, or a stale dir would force full
/// re-reads forever after.
fn refresh_launcher_map_from(db: &crate::db::Db, log_dirs: &[PathBuf]) -> AppResult<()> {
    let since: i64 = db
        .get_setting("launcher_logs_parsed_through")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Byte cursors for the active logs, so the one file the mtime
    // watermark can never bound is read incrementally instead of
    // wholesale on every refresh. Unreadable stored state degrades to
    // a full parse, never an error.
    let mut cursors: HashMap<String, launcher::LogCursor> = db
        .get_setting("launcher_log_cursors")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Merge across log dirs by evidence age, not iteration order: on a
    // machine with several installs the newest log wins, whichever dir
    // it lives in.
    let mut timed: std::collections::HashMap<i64, launcher::TimedPair> =
        std::collections::HashMap::new();
    let mut newest = since;
    for dir in log_dirs {
        let (got, dir_newest) = launcher::extract_char_user_map(dir, since, &mut cursors);
        for (char_id, (user_id, mtime)) in got {
            launcher::merge_pair(&mut timed, char_id, user_id, mtime);
        }
        if dir_newest > newest {
            newest = dir_newest;
        }
    }
    let pairs: std::collections::HashMap<i64, i64> =
        timed.into_iter().map(|(c, (u, _))| (c, u)).collect();
    if !pairs.is_empty() {
        db.upsert_char_user_pairs(&pairs, now())?;
    }
    if newest > since {
        db.set_setting("launcher_logs_parsed_through", &newest.to_string())?;
    }
    db.set_setting(
        "launcher_log_cursors",
        &serde_json::to_string(&cursors).unwrap_or_else(|_| "{}".into()),
    )?;
    Ok(())
}

/// The on-disk `core_user_*.dat` belonging to `source_id`'s account,
/// if the launcher-log mapping knows the account and the file exists
/// in the source's settings dir.
fn source_user_file_for(
    g: &AppState,
    source_id: i64,
    source_settings_dir: &std::path::Path,
) -> AppResult<Option<PathBuf>> {
    let user_id = g.db.lookup_user_ids(&[source_id])?.get(&source_id).copied();
    Ok(user_id.and_then(|uid| ops::user_file_for_user_id(source_settings_dir, uid)))
}

/// Take a snapshot only when the user has already initialized the
/// mirror. Mutations auto-snapshot through this so "every change is a
/// commit" holds, but they must never implicitly create the repo -
/// turning version control on stays an explicit choice.
fn snapshot_if_initialized(
    mirror: &std::path::Path,
    profiles: &[ProfileFile],
    names: &HashMap<i64, String>,
    message: &str,
) -> AppResult<Option<String>> {
    if !git_mirror::status(mirror)?.initialized {
        return Ok(None);
    }
    git_mirror::snapshot(mirror, profiles, names, message)
}

#[derive(Serialize, Clone)]
pub struct InstallSummary {
    pub installs: Vec<EveInstall>,
    pub override_path: Option<PathBuf>,
}

#[tauri::command]
pub async fn detect_installs(state: S<'_>) -> AppResult<InstallSummary> {
    let state = state.inner().clone();
    run_blocking(move || {
        let mut g = state.lock().unwrap();
        g.rescan();
        Ok(InstallSummary {
            installs: g.installs.clone(),
            override_path: g.manual_override.clone(),
        })
    })
    .await
}

#[tauri::command]
pub async fn set_install_override(state: S<'_>, path: Option<String>) -> AppResult<InstallSummary> {
    let state = state.inner().clone();
    run_blocking(move || {
        let mut g = state.lock().unwrap();
        match path {
            Some(p) => {
                let buf = PathBuf::from(&p);
                if !buf.exists() {
                    return Err(AppError::NotFound(p));
                }
                if !looks_like_settings_dir(&buf) && !has_settings_subdir(&buf) {
                    return Err(AppError::Config(
                        "selected folder doesn't look like an EVE settings folder (no core_*.dat files found)".into(),
                    ));
                }
                g.db.set_setting("install_override", &p)?;
                g.manual_override = Some(buf);
            }
            None => {
                g.db.delete_setting("install_override")?;
                g.manual_override = None;
            }
        }
        g.rescan();
        Ok(InstallSummary {
            installs: g.installs.clone(),
            override_path: g.manual_override.clone(),
        })
    })
    .await
}

fn has_settings_subdir(p: &std::path::Path) -> bool {
    std::fs::read_dir(p)
        .map(|rd| {
            rd.flatten().any(|e| {
                e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && e.file_name().to_string_lossy().starts_with("settings_")
                    // Same rule build_install applies: a settings_* dir
                    // with no .dat files would pass validation here and
                    // then be silently dropped by discover, leaving the
                    // user with "Override set" and zero installs.
                    && looks_like_settings_dir(&e.path())
            })
        })
        .unwrap_or(false)
}

#[derive(Serialize, Clone)]
pub struct CharacterEntry {
    pub id: i64,
    pub name: Option<String>,
    pub install_root: PathBuf,
    pub settings_dirs: Vec<PathBuf>,
    /// Distinct servers this character's settings dirs belong to
    /// (empty when no dir carries a recognizable server component).
    pub servers: Vec<String>,
    /// The owning account per the launcher-log mapping, when known.
    pub user_id: Option<i64>,
    /// The user's alias for that account, when they set one.
    pub account_alias: Option<String>,
    pub modified: Option<i64>,
    pub size: u64,
}

#[derive(Serialize, Clone)]
pub struct CharactersResponse {
    pub characters: Vec<CharacterEntry>,
    pub users: Vec<UserEntry>,
}

#[derive(Serialize, Clone)]
pub struct UserEntry {
    pub id: i64,
    pub install_root: PathBuf,
    pub modified: Option<i64>,
}

#[tauri::command]
pub async fn list_characters(state: S<'_>) -> AppResult<CharactersResponse> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let ids: Vec<i64> = profiles
            .iter()
            .filter_map(|p| match p {
                ProfileFile::Character { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        let cached = g.db.lookup_names(&ids)?;
        let mut resp = build_characters_response(&profiles, &cached);

        // Cheap DB lookups only - no launcher-log parse here, this is
        // the roster the first paint waits on. The map itself is
        // refreshed by the copy/template/accounts paths.
        let pairs = g.db.lookup_user_ids(&ids)?;
        let aliases: HashMap<i64, String> =
            g.db.list_account_meta()?
                .into_iter()
                .filter_map(|m| m.alias.map(|a| (m.user_id, a)))
                .collect();
        attach_accounts(&mut resp, &pairs, &aliases);
        Ok(resp)
    })
    .await
}

/// Stamp each character with its owning account and that account's
/// alias, where the launcher-log mapping knows them.
fn attach_accounts(
    resp: &mut CharactersResponse,
    pairs: &HashMap<i64, i64>,
    aliases: &HashMap<i64, String>,
) {
    for c in &mut resp.characters {
        c.user_id = pairs.get(&c.id).copied();
        c.account_alias = c.user_id.and_then(|u| aliases.get(&u).cloned());
    }
}

/// Fold the flat profile list into one entry per character (and per
/// user), attaching cached names. A character can own a profile file in
/// several settings dirs, so entries are merged by id: the newest mtime
/// wins and every settings dir is recorded.
///
/// Split out of `list_characters` so it can be tested without a running
/// app - the command itself is now just the state + DB lookup around it.
fn build_characters_response(
    profiles: &[ProfileFile],
    names: &HashMap<i64, String>,
) -> CharactersResponse {
    let mut chars_by_id: HashMap<i64, CharacterEntry> = HashMap::new();
    let mut users_by_id: HashMap<i64, UserEntry> = HashMap::new();

    for p in profiles {
        match p {
            ProfileFile::Character {
                id,
                install_root,
                settings_dir,
                modified,
                size,
                ..
            } => {
                let entry = chars_by_id.entry(*id).or_insert(CharacterEntry {
                    id: *id,
                    name: None,
                    install_root: install_root.clone(),
                    settings_dirs: Vec::new(),
                    servers: Vec::new(),
                    user_id: None,
                    account_alias: None,
                    modified: *modified,
                    size: *size,
                });
                if !entry.settings_dirs.contains(settings_dir) {
                    entry.settings_dirs.push(settings_dir.clone());
                }
                if entry.modified < *modified {
                    entry.modified = *modified;
                }
            }
            ProfileFile::User {
                id,
                install_root,
                modified,
                ..
            } => {
                users_by_id.entry(*id).or_insert(UserEntry {
                    id: *id,
                    install_root: install_root.clone(),
                    modified: *modified,
                });
            }
        }
    }

    for (id, entry) in chars_by_id.iter_mut() {
        entry.name = names.get(id).cloned();
        let mut servers: Vec<String> = entry
            .settings_dirs
            .iter()
            .filter_map(|d| crate::eve::paths::server_of(d).map(str::to_string))
            .collect();
        servers.sort();
        servers.dedup();
        entry.servers = servers;
    }

    let mut chars: Vec<CharacterEntry> = chars_by_id.into_values().collect();
    chars.sort_by(|a, b| {
        a.name
            .as_deref()
            .unwrap_or("")
            .cmp(b.name.as_deref().unwrap_or(""))
            .then(a.id.cmp(&b.id))
    });
    let mut users: Vec<UserEntry> = users_by_id.into_values().collect();
    users.sort_by_key(|u| u.id);
    CharactersResponse {
        characters: chars,
        users,
    }
}

/// How long a cached name (or tombstone) is trusted before it gets
/// re-resolved. Long enough that steady-state refreshes never touch
/// the network, short enough that a character rename shows up within
/// the week.
const NAME_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[tauri::command]
pub async fn resolve_names(state: S<'_>, ids: Vec<i64>) -> AppResult<HashMap<i64, String>> {
    // Diff against everything freshly cached - including tombstones -
    // so ids ESI has already refused are not re-sent on every refresh,
    // while entries past the TTL fall out and get re-resolved.
    let unknown: Vec<i64> = {
        let g = state.lock().unwrap();
        let cached = g.db.cached_ids(&ids, now() - NAME_TTL_SECS)?;
        ids.iter()
            .copied()
            .filter(|i| !cached.contains(i))
            .collect()
    };

    let resolution = if unknown.is_empty() {
        esi::EsiResolution::default()
    } else {
        esi::resolve_ids(&unknown).await?
    };

    let g = state.lock().unwrap();
    g.db.upsert_names(&resolution.names, now())?;
    if !resolution.unresolvable.is_empty() {
        let tombstones: Vec<esi::EsiName> = resolution
            .unresolvable
            .iter()
            .map(|id| esi::EsiName {
                id: *id,
                name: String::new(),
                category: "unresolvable".to_string(),
            })
            .collect();
        g.db.upsert_names(&tombstones, now())?;
    }
    g.db.lookup_names(&ids)
}

#[tauri::command]
pub async fn copy_character(
    state: S<'_>,
    source_id: i64,
    target_ids: Vec<i64>,
    selection: Option<ops::CopySelection>,
    cross_server: Option<bool>,
) -> AppResult<CopyReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        do_copy_character(
            &state,
            source_id,
            &target_ids,
            selection.as_ref(),
            cross_server.unwrap_or(false),
        )
    })
    .await
}

/// The body of `copy_character`, shared with `copy_to_group`.
fn do_copy_character(
    state: &Mutex<AppState>,
    source_id: i64,
    target_ids: &[i64],
    selection: Option<&ops::CopySelection>,
    cross_server: bool,
) -> AppResult<CopyReport> {
    let g = state.lock().unwrap();
    ops::ensure_not_running()?;
    let profiles = enumerate(&g.installs)?;
    let source = ops::find_character_file(&profiles, source_id)
        .ok_or_else(|| AppError::NotFound(format!("source character {source_id}")))?;
    let source_char_path = source.path().to_path_buf();
    let source_settings_dir = source.settings_dir().to_path_buf();

    refresh_launcher_map(&g)?;
    let source_user_file = source_user_file_for(&g, source_id, &source_settings_dir)?;

    // The "before" commit is what makes a wrong-direction copy
    // undoable, so a failure here blocks the copy. The "after" commit
    // is only the record of the new state; failing to write it must
    // not make a copy that already happened look failed.
    let names = char_names_for(&g, &profiles)?;
    let src_label = names
        .get(&source_id)
        .cloned()
        .unwrap_or_else(|| source_id.to_string());
    snapshot_if_initialized(
        &g.mirror_dir,
        &profiles,
        &names,
        &format!("before copy from {src_label}"),
    )?;

    // Re-checked after the slow pre-steps (log parse, before-snapshot):
    // EVE launched in that window must still block the writes.
    ops::ensure_not_running()?;
    let mut report = ops::replicate_with(
        source_id,
        &source_char_path,
        source_user_file.as_deref(),
        target_ids,
        &profiles,
        selection,
        cross_server,
    )?;

    if let Err(e) = snapshot_if_initialized(
        &g.mirror_dir,
        &profiles,
        &names,
        &format!("after copy from {src_label}"),
    ) {
        report
            .skipped
            .push(format!("warning: post-copy snapshot failed: {e}"));
    }
    Ok(report)
}

#[tauri::command]
pub async fn copy_to_group(
    state: S<'_>,
    source_id: i64,
    group_id: i64,
    selection: Option<ops::CopySelection>,
    cross_server: Option<bool>,
) -> AppResult<CopyReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let member_ids = {
            let g = state.lock().unwrap();
            g.db.group_member_ids(group_id)?
        };
        do_copy_character(
            &state,
            source_id,
            &member_ids,
            selection.as_ref(),
            cross_server.unwrap_or(false),
        )
    })
    .await
}

/// The settings groups available for a selective copy from `source_id`:
/// the top-level groups of its char file, and of its account's user
/// file when the launcher-log mapping knows it. Empty lists mean
/// selective mode is unavailable for that file (unparseable or absent)
/// and the UI falls back to copy-everything.
#[derive(Serialize)]
pub struct CopyGroups {
    pub char_groups: Vec<String>,
    pub user_groups: Vec<String>,
}

#[tauri::command]
pub async fn copy_groups(state: S<'_>, source_id: i64) -> AppResult<CopyGroups> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let source = ops::find_character_file(&profiles, source_id)
            .ok_or_else(|| AppError::NotFound(format!("character {source_id}")))?;
        let char_groups = std::fs::read(source.path())
            .ok()
            .and_then(|b| crate::eve::settings::list_groups(&b).ok())
            .unwrap_or_default();

        refresh_launcher_map(&g)?;
        let user_groups = source_user_file_for(&g, source_id, source.settings_dir())?
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| crate::eve::settings::list_groups(&b).ok())
            .unwrap_or_default();

        Ok(CopyGroups {
            char_groups,
            user_groups,
        })
    })
    .await
}

#[tauri::command]
pub async fn template_groups(state: S<'_>, template_id: i64) -> AppResult<CopyGroups> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let (char_bytes, user_bytes) =
            g.db.template_blobs(template_id)?
                .ok_or_else(|| AppError::NotFound(format!("template {template_id}")))?;
        Ok(CopyGroups {
            char_groups: crate::eve::settings::list_groups(&char_bytes).unwrap_or_default(),
            user_groups: user_bytes
                .as_deref()
                .and_then(|b| crate::eve::settings::list_groups(b).ok())
                .unwrap_or_default(),
        })
    })
    .await
}

#[tauri::command]
pub async fn save_template(state: S<'_>, source_id: i64, name: String) -> AppResult<Template> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let source = ops::find_character_file(&profiles, source_id)
            .ok_or_else(|| AppError::NotFound(format!("character {source_id}")))?;
        let bytes = std::fs::read(source.path())?;

        // Window positions, overview and chat live in the account-level
        // user file, so a template without it is not the profile the user
        // thinks they froze. Capture it when the launcher-log mapping
        // knows the account; the Template's has_user_data flag tells the
        // UI to warn when it doesn't.
        refresh_launcher_map(&g)?;
        let user_bytes = source_user_file_for(&g, source_id, source.settings_dir())?
            .map(std::fs::read)
            .transpose()?;

        let server = crate::eve::paths::server_of(source.settings_dir());
        let id = g.db.create_template(
            &name,
            Some(source_id),
            &bytes,
            user_bytes.as_deref(),
            now(),
            server,
        )?;
        let templates = g.db.list_templates()?;
        templates
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| AppError::Other("created template missing".into()))
    })
    .await
}

#[tauri::command]
pub async fn list_templates(state: S<'_>) -> AppResult<Vec<Template>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        g.db.list_templates()
    })
    .await
}

#[tauri::command]
pub async fn apply_template(
    state: S<'_>,
    template_id: i64,
    target_ids: Vec<i64>,
    selection: Option<ops::CopySelection>,
    cross_server: Option<bool>,
) -> AppResult<CopyReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        ops::ensure_not_running()?;
        let template =
            g.db.list_templates()?
                .into_iter()
                .find(|t| t.id == template_id)
                .ok_or_else(|| AppError::NotFound(format!("template {template_id}")))?;
        let (char_bytes, user_bytes) =
            g.db.template_blobs(template_id)?
                .ok_or_else(|| AppError::NotFound(format!("template {template_id}")))?;
        let profiles = enumerate(&g.installs)?;

        let names = char_names_for(&g, &profiles)?;
        snapshot_if_initialized(
            &g.mirror_dir,
            &profiles,
            &names,
            &format!("before applying template {}", template.name),
        )?;

        ops::ensure_not_running()?;
        let mut report = ops::apply_template_bytes_with(
            &char_bytes,
            user_bytes.as_deref(),
            &target_ids,
            &profiles,
            selection.as_ref(),
            template.source_server.as_deref(),
            cross_server.unwrap_or(false),
        )?;

        if let Err(e) = snapshot_if_initialized(
            &g.mirror_dir,
            &profiles,
            &names,
            &format!("after applying template {}", template.name),
        ) {
            report
                .skipped
                .push(format!("warning: post-apply snapshot failed: {e}"));
        }
        Ok(report)
    })
    .await
}

#[tauri::command]
pub async fn delete_template(state: S<'_>, template_id: i64) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        g.db.delete_template(template_id)
    })
    .await
}

#[tauri::command]
pub async fn list_groups(state: S<'_>) -> AppResult<Vec<Group>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        g.db.list_groups()
    })
    .await
}

#[tauri::command]
pub async fn create_group(state: S<'_>, name: String) -> AppResult<Group> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let id = g.db.create_group(&name)?;
        Ok(Group {
            id,
            name,
            member_ids: Vec::new(),
        })
    })
    .await
}

#[tauri::command]
pub async fn delete_group(state: S<'_>, group_id: i64) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        g.db.delete_group(group_id)
    })
    .await
}

#[tauri::command]
pub async fn set_group_members(
    state: S<'_>,
    group_id: i64,
    member_ids: Vec<i64>,
) -> AppResult<Group> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        g.db.set_group_members(group_id, &member_ids)?;
        let groups = g.db.list_groups()?;
        groups
            .into_iter()
            .find(|gr| gr.id == group_id)
            .ok_or_else(|| AppError::NotFound(format!("group {group_id}")))
    })
    .await
}

/// Polled every few seconds by the UI: the process-table walk belongs
/// on a worker, not the main thread.
#[tauri::command]
pub async fn is_eve_running() -> bool {
    tauri::async_runtime::spawn_blocking(ops::is_eve_running)
        .await
        .unwrap_or(false)
}

/// Portraits from the public EVE image server, disk-cached, returned as
/// data URIs (the webview's CSP allows no remote images on purpose).
/// Best-effort: absent ids mean the UI shows its monogram fallback.
#[tauri::command]
pub async fn character_portraits(state: S<'_>, ids: Vec<i64>) -> AppResult<HashMap<i64, String>> {
    let dir = {
        let g = state.lock().unwrap();
        g.portrait_dir.clone()
    };
    Ok(crate::eve::portraits::fetch_portraits(&dir, &ids).await)
}

#[tauri::command]
pub async fn git_status(state: S<'_>) -> AppResult<git_mirror::MirrorStatus> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        git_mirror::status(&g.mirror_dir)
    })
    .await
}

#[tauri::command]
pub async fn git_set_remote(state: S<'_>, url: String) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        git_mirror::set_remote(&g.mirror_dir, &url)
    })
    .await
}

#[tauri::command]
pub async fn git_snapshot(state: S<'_>, message: Option<String>) -> AppResult<Option<String>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let names = char_names_for(&g, &profiles)?;
        let msg = message.unwrap_or_else(|| {
            format!(
                "snapshot {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            )
        });
        git_mirror::snapshot(&g.mirror_dir, &profiles, &names, &msg)
    })
    .await
}

#[tauri::command]
pub async fn git_history(
    state: S<'_>,
    limit: Option<usize>,
) -> AppResult<Vec<git_mirror::CommitEntry>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        git_mirror::history(&g.mirror_dir, limit.unwrap_or(100))
    })
    .await
}

#[tauri::command]
pub async fn git_restore(
    state: S<'_>,
    oid: String,
    selection: Option<ops::CopySelection>,
) -> AppResult<git_mirror::RestoreReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        ops::ensure_not_running()?;
        let profiles = enumerate(&g.installs)?;

        // Committing the live state first is what makes the docs' claim -
        // "the state you were in before the restore is still in the
        // history" - actually true.
        let names = char_names_for(&g, &profiles)?;
        // Char-safe: a malformed oid must fail in Oid::from_str, not
        // panic here while the state mutex is held (a poison would
        // brick every later command).
        let short: String = oid.chars().take(7).collect();
        let short = short.as_str();
        snapshot_if_initialized(
            &g.mirror_dir,
            &profiles,
            &names,
            &format!("before restore of {short}"),
        )?;

        let mut report = {
            ops::ensure_not_running()?;
            git_mirror::restore_commit_with(&g.mirror_dir, &oid, &profiles, selection.as_ref())?
        };

        if let Err(e) = snapshot_if_initialized(
            &g.mirror_dir,
            &profiles,
            &names,
            &format!("after restore of {short}"),
        ) {
            report
                .skipped
                .push(format!("warning: post-restore snapshot failed: {e}"));
        }
        Ok(report)
    })
    .await
}

#[tauri::command]
pub async fn git_push(state: S<'_>) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let pat = keychain::get_pat()?
            .ok_or_else(|| AppError::Config("no GitHub PAT stored; set it in Settings".into()))?;
        let g = state.lock().unwrap();
        let status = git_mirror::status(&g.mirror_dir)?;
        if !status.initialized {
            return Err(AppError::Config(
                "mirror not initialized; take a snapshot first".into(),
            ));
        }
        git_mirror::push(&g.mirror_dir, &status.branch, &pat)
    })
    .await
}

/// The stored PAT if the keyring can produce one; anonymous otherwise.
/// Public remotes clone and pull fine without credentials, so a broken
/// keyring must not block the attempt - the transport will surface an
/// auth error if the remote actually needs one.
fn pat_if_available() -> Option<String> {
    match keychain::get_pat() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("keyring unavailable, proceeding without PAT: {e}");
            None
        }
    }
}

#[tauri::command]
pub async fn git_clone(state: S<'_>, url: String) -> AppResult<git_mirror::MirrorStatus> {
    let state = state.inner().clone();
    run_blocking(move || {
        let pat = pat_if_available();
        let g = state.lock().unwrap();
        git_mirror::clone(&g.mirror_dir, &url, pat)
    })
    .await
}

#[tauri::command]
pub async fn git_pull(state: S<'_>) -> AppResult<git_mirror::PullReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let pat = pat_if_available();
        let g = state.lock().unwrap();
        git_mirror::pull(&g.mirror_dir, pat)
    })
    .await
}

/// The keyring is a D-Bus round trip on Linux and a prompt-capable API
/// elsewhere; none of it belongs on the main thread.
#[tauri::command]
pub async fn pat_set(token: String) -> AppResult<()> {
    run_blocking(move || keychain::set_pat(&token)).await
}

#[tauri::command]
pub async fn pat_clear() -> AppResult<()> {
    run_blocking(keychain::clear_pat).await
}

/// Degrades to `false` instead of erroring when the keyring is
/// unavailable (no Secret Service provider on Linux, say): this is
/// called on every refresh alongside everything else the UI loads, and
/// an optional PAT check must never take the whole refresh down with
/// it. `pat_set`/`git_push` stay loud - there the user asked for the
/// keyring specifically and needs the real error.
#[tauri::command]
pub async fn pat_present() -> bool {
    tauri::async_runtime::spawn_blocking(|| match keychain::get_pat() {
        Ok(t) => t.is_some(),
        Err(e) => {
            tracing::warn!("keyring unavailable, treating PAT as absent: {e}");
            false
        }
    })
    .await
    .unwrap_or(false)
}

#[tauri::command]
pub async fn export_setup(state: S<'_>, path: String) -> AppResult<crate::portable::ExportSummary> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let names = char_names_for(&g, &profiles)?;
        crate::portable::export_setup(std::path::Path::new(&path), &profiles, &names, now())
    })
    .await
}

#[tauri::command]
pub async fn preview_import(
    state: S<'_>,
    path: String,
) -> AppResult<crate::portable::ImportPreview> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        crate::portable::preview(std::path::Path::new(&path), &profiles)
    })
    .await
}

#[tauri::command]
pub async fn import_setup(state: S<'_>, path: String) -> AppResult<CopyReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        ops::ensure_not_running()?;
        let profiles = enumerate(&g.installs)?;

        let names = char_names_for(&g, &profiles)?;
        snapshot_if_initialized(&g.mirror_dir, &profiles, &names, "before zip import")?;

        ops::ensure_not_running()?;
        let mut report = crate::portable::import_setup(std::path::Path::new(&path), &profiles)?;

        if let Err(e) =
            snapshot_if_initialized(&g.mirror_dir, &profiles, &names, "after zip import")
        {
            report
                .skipped
                .push(format!("warning: post-import snapshot failed: {e}"));
        }
        Ok(report)
    })
    .await
}

#[tauri::command]
pub async fn export_template(state: S<'_>, template_id: i64, path: String) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let template =
            g.db.list_templates()?
                .into_iter()
                .find(|t| t.id == template_id)
                .ok_or_else(|| AppError::NotFound(format!("template {template_id}")))?;
        let (char_bytes, user_bytes) =
            g.db.template_blobs(template_id)?
                .ok_or_else(|| AppError::NotFound(format!("template {template_id}")))?;
        let meta = crate::portable::TemplateMeta {
            name: template.name,
            source_character_id: template.source_character_id,
            source_name: template.source_name,
            has_user_data: template.has_user_data,
            source_server: template.source_server,
        };
        crate::portable::export_template(
            std::path::Path::new(&path),
            &meta,
            &char_bytes,
            user_bytes.as_deref(),
            now(),
        )
    })
    .await
}

#[tauri::command]
pub async fn import_template(state: S<'_>, path: String) -> AppResult<Template> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let (meta, char_bytes, user_bytes) =
            crate::portable::read_template(std::path::Path::new(&path))?;

        // A doctrine passed around a corp will be imported more than
        // once; suffix instead of tripping the unique-name constraint.
        let existing: std::collections::HashSet<String> =
            g.db.list_templates()?.into_iter().map(|t| t.name).collect();
        let name = free_template_name(&meta.name, &existing);

        // Seed the name cache so the shelf can say "from Alice" even
        // when this machine has never met the source character.
        if let (Some(id), Some(source_name)) = (meta.source_character_id, &meta.source_name) {
            g.db.upsert_names(
                &[crate::eve::esi::EsiName {
                    id,
                    name: source_name.clone(),
                    category: "character".into(),
                }],
                now(),
            )?;
        }

        let id = g.db.create_template(
            &name,
            meta.source_character_id,
            &char_bytes,
            user_bytes.as_deref(),
            now(),
            meta.source_server.as_deref(),
        )?;
        let templates = g.db.list_templates()?;
        templates
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| AppError::Other("imported template missing".into()))
    })
    .await
}

/// What a commit changed, semantically - loaded lazily when a ledger
/// entry is expanded.
#[tauri::command]
pub async fn git_diff(state: S<'_>, oid: String) -> AppResult<Vec<git_mirror::FileDiff>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        git_mirror::commit_semantic_diff(&g.mirror_dir, &oid)
    })
    .await
}

/// The settings groups a selective restore from this commit can offer.
#[tauri::command]
pub async fn commit_groups(state: S<'_>, oid: String) -> AppResult<CopyGroups> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let (char_groups, user_groups) = git_mirror::commit_groups(&g.mirror_dir, &oid)?;
        Ok(CopyGroups {
            char_groups,
            user_groups,
        })
    })
    .await
}

/// How two characters' settings differ, group by group. Notes are
/// machine codes the frontend translates: "undecodable" (bytes
/// compared instead), "same_account", "unavailable".
#[derive(Serialize)]
pub struct CompareReport {
    pub char_groups: Vec<crate::eve::settings::GroupDiff>,
    pub char_note: Option<String>,
    /// Only meaningful when char_note is "undecodable": whether the
    /// raw bytes differ.
    pub char_bytes_differ: bool,
    pub user_groups: Vec<crate::eve::settings::GroupDiff>,
    pub user_note: Option<String>,
}

#[tauri::command]
pub async fn diff_characters(state: S<'_>, a_id: i64, b_id: i64) -> AppResult<CompareReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let a = ops::find_character_file(&profiles, a_id)
            .ok_or_else(|| AppError::NotFound(format!("character {a_id}")))?;
        let b = ops::find_character_file(&profiles, b_id)
            .ok_or_else(|| AppError::NotFound(format!("character {b_id}")))?;
        let a_bytes = std::fs::read(a.path())?;
        let b_bytes = std::fs::read(b.path())?;

        let (char_groups, char_note, char_bytes_differ) =
            match crate::eve::settings::diff_settings(&a_bytes, &b_bytes) {
                Ok(groups) => (groups, None, false),
                Err(_) => (
                    Vec::new(),
                    Some("undecodable".to_string()),
                    a_bytes != b_bytes,
                ),
            };

        // Account side: both characters need a known account, distinct
        // accounts, and a user file on disk for each.
        refresh_launcher_map(&g)?;
        let uids = g.db.lookup_user_ids(&[a_id, b_id])?;
        let (user_groups, user_note) = match (uids.get(&a_id), uids.get(&b_id)) {
            (Some(ua), Some(ub)) if ua == ub => (Vec::new(), Some("same_account".to_string())),
            (Some(ua), Some(ub)) => {
                let fa = ops::user_file_for_user_id(a.settings_dir(), *ua);
                let fb = ops::user_file_for_user_id(b.settings_dir(), *ub);
                match (fa, fb) {
                    (Some(fa), Some(fb)) => {
                        let ba = std::fs::read(fa)?;
                        let bb = std::fs::read(fb)?;
                        match crate::eve::settings::diff_settings(&ba, &bb) {
                            Ok(groups) => (groups, None),
                            Err(_) => (Vec::new(), Some("undecodable".to_string())),
                        }
                    }
                    _ => (Vec::new(), Some("unavailable".to_string())),
                }
            }
            _ => (Vec::new(), Some("unavailable".to_string())),
        };

        Ok(CompareReport {
            char_groups,
            char_note,
            char_bytes_differ,
            user_groups,
            user_note,
        })
    })
    .await
}

/// The window layout a character's settings record, if any: the char
/// file's own, else the account file's (the schema has lived in both
/// over EVE's history). None means nothing renderable - the UI simply
/// shows no sketch.
#[tauri::command]
pub async fn character_layout(
    state: S<'_>,
    character_id: i64,
) -> AppResult<Option<crate::eve::settings::WindowLayout>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let source = ops::find_character_file(&profiles, character_id)
            .ok_or_else(|| AppError::NotFound(format!("character {character_id}")))?;
        let bytes = std::fs::read(source.path())?;
        if let Ok(Some(layout)) = crate::eve::settings::window_layout(&bytes) {
            return Ok(Some(layout));
        }
        if let Some(user_file) = source_user_file_for(&g, character_id, source.settings_dir())? {
            let bytes = std::fs::read(user_file)?;
            if let Ok(Some(layout)) = crate::eve::settings::window_layout(&bytes) {
                return Ok(Some(layout));
            }
        }
        Ok(None)
    })
    .await
}

/// The window layout frozen inside a template, if any.
#[tauri::command]
pub async fn template_layout(
    state: S<'_>,
    template_id: i64,
) -> AppResult<Option<crate::eve::settings::WindowLayout>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let (char_bytes, user_bytes) =
            g.db.template_blobs(template_id)?
                .ok_or_else(|| AppError::NotFound(format!("template {template_id}")))?;
        if let Ok(Some(layout)) = crate::eve::settings::window_layout(&char_bytes) {
            return Ok(Some(layout));
        }
        if let Some(u) = user_bytes {
            if let Ok(Some(layout)) = crate::eve::settings::window_layout(&u) {
                return Ok(Some(layout));
            }
        }
        Ok(None)
    })
    .await
}

/// Per-window size floors, learned from every settings file on this
/// machine: the smallest width and height EVE has ever saved for each
/// window name. The client never records a size it wouldn't allow, so
/// anything at or above the floor is proven achievable - the designer
/// clamps there instead of drawing layouts the game would correct.
#[tauri::command]
pub async fn layout_floors(state: S<'_>) -> AppResult<HashMap<String, (i64, i64)>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        let mut floors: HashMap<String, (i64, i64)> = HashMap::new();
        for p in &profiles {
            let Ok(bytes) = std::fs::read(p.path()) else {
                continue;
            };
            let Ok(sizes) = crate::eve::settings::geometry_sizes(&bytes) else {
                continue;
            };
            for (name, w, h) in sizes {
                floors
                    .entry(name)
                    .and_modify(|(fw, fh)| {
                        *fw = (*fw).min(w);
                        *fh = (*fh).min(h);
                    })
                    .or_insert((w, h));
            }
        }
        Ok(merge_floors(floors, &g.db.list_window_floors()?))
    })
    .await
}

/// The designer's final floor table. Observed minimums are raised to
/// at least the shipped defaults - safe whichever way the user's UI
/// scale differs from the calibration machine's - and a local
/// calibration run (the game's own answer on THIS machine) replaces
/// both.
fn merge_floors(
    mut observed: HashMap<String, (i64, i64)>,
    calibrated: &[(String, i64, i64)],
) -> HashMap<String, (i64, i64)> {
    for (name, (w, h)) in observed.iter_mut() {
        if let Some((sw, sh)) = crate::eve::settings::shipped_floor(name) {
            *w = (*w).max(sw);
            *h = (*h).max(sh);
        }
    }
    for (name, w, h) in calibrated {
        observed.insert(name.clone(), (*w, *h));
    }
    observed
}

/// The file a character's designable layout lives in:
/// (is_user_file, path, bytes). Char file first, account file second -
/// the schema has lived in both across EVE's history.
fn locate_layout_file(
    g: &AppState,
    character_id: i64,
    profiles: &[ProfileFile],
) -> AppResult<(bool, PathBuf, Vec<u8>)> {
    let source = ops::find_character_file(profiles, character_id)
        .ok_or_else(|| AppError::NotFound(format!("character {character_id}")))?;
    let char_bytes = std::fs::read(source.path())?;
    if matches!(
        crate::eve::settings::window_layout(&char_bytes),
        Ok(Some(_))
    ) {
        return Ok((false, source.path().to_path_buf(), char_bytes));
    }
    if let Some(user_path) = source_user_file_for(g, character_id, source.settings_dir())? {
        let user_bytes = std::fs::read(&user_path)?;
        if matches!(
            crate::eve::settings::window_layout(&user_bytes),
            Ok(Some(_))
        ) {
            return Ok((true, user_path, user_bytes));
        }
    }
    Err(AppError::Other(
        "this character records no window geometry".into(),
    ))
}

/// Freeze a designed layout as a new template: the source character's
/// files with the geometry rewritten. Nothing on disk changes.
#[tauri::command]
pub async fn design_save_template(
    state: S<'_>,
    character_id: i64,
    changes: Vec<crate::eve::settings::LayoutChange>,
    name: String,
) -> AppResult<Template> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        let profiles = enumerate(&g.installs)?;
        refresh_launcher_map(&g)?;
        let (is_user, _path, bytes) = locate_layout_file(&g, character_id, &profiles)?;
        let (designed, _applied, _skipped) = crate::eve::settings::apply_layout(&bytes, &changes)?;

        let source = ops::find_character_file(&profiles, character_id)
            .ok_or_else(|| AppError::NotFound(format!("character {character_id}")))?;
        let (char_blob, user_blob) = if is_user {
            (std::fs::read(source.path())?, Some(designed))
        } else {
            let user_bytes = source_user_file_for(&g, character_id, source.settings_dir())?
                .map(std::fs::read)
                .transpose()?;
            (designed, user_bytes)
        };

        let id = g.db.create_template(
            &name,
            Some(character_id),
            &char_blob,
            user_blob.as_deref(),
            now(),
            crate::eve::paths::server_of(source.settings_dir()),
        )?;
        let templates = g.db.list_templates()?;
        templates
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| AppError::Other("created template missing".into()))
    })
    .await
}

/// Write a designed layout straight into the character's live file,
/// with the same gates as every mutation: EVE closed, snapshot
/// bracket when version control is on.
#[tauri::command]
pub async fn design_apply(
    state: S<'_>,
    character_id: i64,
    changes: Vec<crate::eve::settings::LayoutChange>,
) -> AppResult<CopyReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        ops::ensure_not_running()?;
        let profiles = enumerate(&g.installs)?;
        refresh_launcher_map(&g)?;
        let (_is_user, path, bytes) = locate_layout_file(&g, character_id, &profiles)?;
        let (designed, _applied, skipped) = crate::eve::settings::apply_layout(&bytes, &changes)?;

        let names = char_names_for(&g, &profiles)?;
        let label = names
            .get(&character_id)
            .cloned()
            .unwrap_or_else(|| character_id.to_string());
        snapshot_if_initialized(
            &g.mirror_dir,
            &profiles,
            &names,
            &format!("before layout design for {label}"),
        )?;

        ops::ensure_not_running()?;
        ops::write_profile_bytes(&path, &designed)?;

        let mut report = CopyReport {
            written: vec![path],
            skipped: skipped
                .into_iter()
                .map(|n| format!("window {n} not found in the file"))
                .collect(),
        };
        if let Err(e) = snapshot_if_initialized(
            &g.mirror_dir,
            &profiles,
            &names,
            &format!("after layout design for {label}"),
        ) {
            report
                .skipped
                .push(format!("warning: post-design snapshot failed: {e}"));
        }
        Ok(report)
    })
    .await
}

/// The shelf name an imported template gets: the archive's own name,
/// or the first "{name} (n)" variant not already taken.
fn free_template_name(base: &str, existing: &std::collections::HashSet<String>) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while existing.contains(&name) {
        name = format!("{base} ({n})");
        n += 1;
    }
    name
}

#[derive(Serialize, Clone)]
pub struct AccountCharacter {
    pub id: i64,
    pub name: Option<String>,
    /// True when the pairing was set by hand rather than learned from
    /// the launcher logs - the UI marks those as editable.
    pub manual: bool,
    /// True when the pairing was set by hand AND the launcher logs
    /// have since said something different - the UI flags these.
    pub disputed: bool,
    /// What the logs last said, for the dispute message.
    pub log_user_id: Option<i64>,
}

#[derive(Serialize, Clone)]
pub struct AccountEntry {
    pub user_id: i64,
    pub alias: Option<String>,
    pub note: Option<String>,
    pub characters: Vec<AccountCharacter>,
    pub modified: Option<i64>,
}

#[tauri::command]
pub async fn list_accounts(state: S<'_>) -> AppResult<Vec<AccountEntry>> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        // The full refresh lives here (not in list_characters): parsing
        // launcher logs is the expensive part and the accounts view is
        // where a fresh mapping actually matters.
        refresh_launcher_map(&g)?;
        let profiles = enumerate(&g.installs)?;
        let char_ids: Vec<i64> = profiles
            .iter()
            .filter_map(|p| match p {
                ProfileFile::Character { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        let pairs = g.db.lookup_user_pairs(&char_ids)?;
        let names = g.db.lookup_names(&char_ids)?;
        let meta = g.db.list_account_meta()?;
        Ok(build_accounts_response(&profiles, &pairs, &names, &meta))
    })
    .await
}

/// Fold profile files and the char->user mapping into one entry per
/// account: every account with a live user file, plus any account the
/// mapping points at even when its user file is gone (its characters
/// still deserve a row). Pure so it tests without a running app.
fn build_accounts_response(
    profiles: &[ProfileFile],
    pairs: &HashMap<i64, crate::db::CharPairing>,
    names: &HashMap<i64, String>,
    meta: &[crate::db::AccountMeta],
) -> Vec<AccountEntry> {
    let mut accounts: std::collections::BTreeMap<i64, AccountEntry> =
        std::collections::BTreeMap::new();
    let blank = |user_id: i64| AccountEntry {
        user_id,
        alias: None,
        note: None,
        characters: Vec::new(),
        modified: None,
    };

    for p in profiles {
        if let ProfileFile::User { id, modified, .. } = p {
            let e = accounts.entry(*id).or_insert_with(|| blank(*id));
            if e.modified < *modified {
                e.modified = *modified;
            }
        }
    }

    for p in profiles {
        if let ProfileFile::Character { id, .. } = p {
            if let Some(pairing) = pairs.get(id) {
                let e = accounts
                    .entry(pairing.user_id)
                    .or_insert_with(|| blank(pairing.user_id));
                if !e.characters.iter().any(|c| c.id == *id) {
                    e.characters.push(AccountCharacter {
                        id: *id,
                        name: names.get(id).cloned(),
                        manual: pairing.manual,
                        disputed: pairing.manual
                            && pairing.log_user_id.is_some_and(|l| l != pairing.user_id),
                        log_user_id: pairing.log_user_id,
                    });
                }
            }
        }
    }

    for m in meta {
        if let Some(e) = accounts.get_mut(&m.user_id) {
            e.alias = m.alias.clone();
            e.note = m.note.clone();
        }
    }

    let mut out: Vec<AccountEntry> = accounts.into_values().collect();
    for e in &mut out {
        e.characters.sort_by(|a, b| {
            a.name
                .as_deref()
                .unwrap_or("")
                .cmp(b.name.as_deref().unwrap_or(""))
                .then(a.id.cmp(&b.id))
        });
    }
    out
}

#[tauri::command]
pub async fn set_account_meta(
    state: S<'_>,
    user_id: i64,
    alias: Option<String>,
    note: Option<String>,
) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        g.db.set_account_meta(user_id, alias.as_deref(), note.as_deref())
    })
    .await
}

/// Pair a character with an account by hand, or (with `user_id` None)
/// clear a hand-set pairing. Hand-set pairings feed account-level
/// copies exactly like logged ones, and the log parser never
/// overwrites them.
#[tauri::command]
pub async fn set_char_account(
    state: S<'_>,
    character_id: i64,
    user_id: Option<i64>,
) -> AppResult<()> {
    let state = state.inner().clone();
    run_blocking(move || {
        let g = state.lock().unwrap();
        match user_id {
            Some(uid) => g.db.set_char_user_manual(character_id, uid, now()),
            None => g.db.clear_char_user_manual(character_id, now()),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use tempfile::TempDir;

    fn ch(dir: &std::path::Path, id: i64, modified: i64, size: u64) -> ProfileFile {
        ProfileFile::Character {
            id,
            path: dir.join(format!("core_char_{id}.dat")),
            install_root: dir.to_path_buf(),
            settings_dir: dir.to_path_buf(),
            modified: Some(modified),
            size,
        }
    }

    fn usr(dir: &std::path::Path, id: i64) -> ProfileFile {
        ProfileFile::User {
            id,
            path: dir.join(format!("core_user_{id}.dat")),
            install_root: dir.to_path_buf(),
            settings_dir: dir.to_path_buf(),
            modified: Some(0),
            size: 0,
        }
    }

    fn names(pairs: &[(i64, &str)]) -> HashMap<i64, String> {
        pairs.iter().map(|(i, n)| (*i, n.to_string())).collect()
    }

    // ------------------- build_characters_response --------------------

    #[test]
    fn an_empty_install_yields_an_empty_response() {
        let r = build_characters_response(&[], &names(&[]));
        assert!(r.characters.is_empty());
        assert!(r.users.is_empty());
    }

    #[test]
    fn a_character_in_two_profiles_collapses_to_one_entry() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");
        let b = settings_dir(tmp.path(), "settings_Alt");

        let r = build_characters_response(&[ch(&a, 1001, 10, 5), ch(&b, 1001, 20, 5)], &names(&[]));

        assert_eq!(r.characters.len(), 1);
        assert_eq!(r.characters[0].settings_dirs.len(), 2);
        assert!(r.characters[0].settings_dirs.contains(&a));
        assert!(r.characters[0].settings_dirs.contains(&b));
    }

    #[test]
    fn the_newest_mtime_across_profiles_wins() {
        // The UI shows this as "modified"; showing the older of two
        // profiles would make a fresh copy look stale.
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");
        let b = settings_dir(tmp.path(), "settings_Alt");

        let r =
            build_characters_response(&[ch(&a, 1001, 500, 5), ch(&b, 1001, 100, 5)], &names(&[]));
        assert_eq!(r.characters[0].modified, Some(500));

        // Same, but encountered in the opposite order.
        let r =
            build_characters_response(&[ch(&a, 1001, 100, 5), ch(&b, 1001, 500, 5)], &names(&[]));
        assert_eq!(r.characters[0].modified, Some(500));
    }

    #[test]
    fn a_repeated_settings_dir_is_not_listed_twice() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");

        let r = build_characters_response(&[ch(&a, 1001, 10, 5), ch(&a, 1001, 10, 5)], &names(&[]));

        assert_eq!(r.characters[0].settings_dirs, vec![a]);
    }

    #[test]
    fn cached_names_are_attached() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");

        let r = build_characters_response(
            &[ch(&a, 1001, 10, 5), ch(&a, 2002, 10, 5)],
            &names(&[(1001, "Alice")]),
        );

        let alice = r.characters.iter().find(|c| c.id == 1001).unwrap();
        let unknown = r.characters.iter().find(|c| c.id == 2002).unwrap();
        assert_eq!(alice.name, Some("Alice".to_string()));
        assert_eq!(unknown.name, None);
    }

    #[test]
    fn characters_sort_by_name_then_id() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");

        let r = build_characters_response(
            &[
                ch(&a, 3003, 10, 5),
                ch(&a, 1001, 10, 5),
                ch(&a, 2002, 10, 5),
            ],
            &names(&[(3003, "Alice"), (1001, "Zed"), (2002, "Mallory")]),
        );

        let order: Vec<i64> = r.characters.iter().map(|c| c.id).collect();
        assert_eq!(order, vec![3003, 2002, 1001]);
    }

    #[test]
    fn unnamed_characters_sort_ahead_of_named_ones_by_id() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");

        let r = build_characters_response(
            &[
                ch(&a, 3003, 10, 5),
                ch(&a, 1001, 10, 5),
                ch(&a, 2002, 10, 5),
            ],
            &names(&[(2002, "Alice")]),
        );

        let order: Vec<i64> = r.characters.iter().map(|c| c.id).collect();
        assert_eq!(
            order,
            vec![1001, 3003, 2002],
            "unresolved names collapse to \"\" and must still order deterministically by id"
        );
    }

    #[test]
    fn users_are_deduped_and_sorted_by_id() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");
        let b = settings_dir(tmp.path(), "settings_Alt");

        let r =
            build_characters_response(&[usr(&b, 9002), usr(&a, 9001), usr(&b, 9001)], &names(&[]));

        let ids: Vec<i64> = r.users.iter().map(|u| u.id).collect();
        assert_eq!(ids, vec![9001, 9002]);
    }

    #[test]
    fn characters_and_users_are_kept_in_separate_lists() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");

        let r = build_characters_response(&[ch(&a, 1001, 10, 5), usr(&a, 9001)], &names(&[]));

        assert_eq!(r.characters.len(), 1);
        assert_eq!(r.users.len(), 1);
        assert_eq!(r.characters[0].id, 1001);
        assert_eq!(r.users[0].id, 9001);
    }

    #[test]
    fn a_character_and_user_sharing_an_id_do_not_collide() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");

        let r = build_characters_response(&[ch(&a, 7007, 10, 5), usr(&a, 7007)], &names(&[]));

        assert_eq!(r.characters.len(), 1);
        assert_eq!(r.users.len(), 1);
    }

    #[test]
    fn servers_are_collected_and_deduped_from_settings_dirs() {
        let tmp = TempDir::new().unwrap();
        let tq = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_tq_tranquility"),
            "settings_Default",
        );
        let tq2 = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_tq_tranquility"),
            "settings_Alt",
        );
        let sisi = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_sisi_singularity"),
            "settings_Default",
        );

        let r = build_characters_response(
            &[
                ch(&tq, 1001, 10, 5),
                ch(&tq2, 1001, 10, 5),
                ch(&sisi, 1001, 10, 5),
            ],
            &names(&[]),
        );

        assert_eq!(r.characters[0].servers, vec!["singularity", "tranquility"]);
    }

    // ------------------------- has_settings_subdir --------------------

    #[test]
    fn has_settings_subdir_finds_a_profile_folder() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");
        assert!(has_settings_subdir(tmp.path()));
    }

    #[test]
    fn has_settings_subdir_rejects_a_profile_folder_with_no_dat_files() {
        // Accepting it would show "Override set" and then zero installs
        // once discover applies the stricter build_install rule.
        let tmp = TempDir::new().unwrap();
        settings_dir(tmp.path(), "settings_Default");
        assert!(!has_settings_subdir(tmp.path()));
    }

    #[test]
    fn has_settings_subdir_is_false_without_one() {
        let tmp = TempDir::new().unwrap();
        settings_dir(tmp.path(), "cache");
        assert!(!has_settings_subdir(tmp.path()));
    }

    #[test]
    fn has_settings_subdir_ignores_a_file_named_like_a_profile_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("settings_Default"), "not a dir").unwrap();
        assert!(!has_settings_subdir(tmp.path()));
    }

    #[test]
    fn has_settings_subdir_on_a_missing_path_is_false() {
        let tmp = TempDir::new().unwrap();
        assert!(!has_settings_subdir(&tmp.path().join("nope")));
    }

    // --------------------- snapshot_if_initialized --------------------

    #[test]
    fn snapshot_if_initialized_never_creates_the_repo() {
        let tmp = TempDir::new().unwrap();
        let mirror = tmp.path().join("mirror");

        let r = snapshot_if_initialized(&mirror, &[], &names(&[]), "auto").unwrap();

        assert_eq!(r, None);
        assert!(
            !mirror.join(".git").exists(),
            "a mutation must not implicitly turn version control on"
        );
    }

    #[test]
    fn snapshot_if_initialized_commits_once_the_mirror_exists() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "v1");
        let mirror = tmp.path().join("mirror");
        let profiles = vec![char_profile(&dir, 1001)];
        // The user turns version control on with the first snapshot.
        crate::git_mirror::snapshot(&mirror, &profiles, &names(&[]), "init").unwrap();

        write_char(&dir, 1001, "v2");
        let r = snapshot_if_initialized(&mirror, &profiles, &names(&[]), "auto").unwrap();

        assert!(r.is_some(), "an initialized mirror should get the commit");
    }

    // ------------------- refresh_launcher_map_from --------------------

    fn test_db() -> (TempDir, crate::db::Db) {
        let tmp = TempDir::new().unwrap();
        let db = crate::db::Db::open(&tmp.path().join("t.sqlite")).unwrap();
        (tmp, db)
    }

    fn write_log(dir: &std::path::Path, name: &str, user_id: i64, char_id: i64, mtime: u64) {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!("userId: {user_id},\n  characterId: {char_id},\n"),
        )
        .unwrap();
        set_mtime(&p, mtime);
    }

    fn watermark(db: &crate::db::Db) -> Option<String> {
        db.get_setting("launcher_logs_parsed_through").unwrap()
    }

    #[test]
    fn launcher_refresh_persists_pairs_and_advances_the_watermark() {
        let (tmp, db) = test_db();
        let logs = tmp.path().join("logs");
        write_log(&logs, "eve-online-launcher.log", 9001, 2035047876, 1000);

        refresh_launcher_map_from(&db, &[logs]).unwrap();

        assert_eq!(
            db.lookup_user_ids(&[2035047876]).unwrap().get(&2035047876),
            Some(&9001)
        );
        assert_eq!(watermark(&db).as_deref(), Some("1000"));
    }

    #[test]
    fn launcher_refresh_merges_dirs_and_keeps_the_newest_watermark() {
        let (tmp, db) = test_db();
        let a = tmp.path().join("logs_a");
        let b = tmp.path().join("logs_b");
        write_log(&a, "eve-online-launcher.log", 9001, 2035047876, 1000);
        write_log(&b, "eve-online-launcher.log", 9002, 2035047877, 2000);

        refresh_launcher_map_from(&db, &[a, b]).unwrap();

        let got = db.lookup_user_ids(&[2035047876, 2035047877]).unwrap();
        assert_eq!(got.get(&2035047876), Some(&9001));
        assert_eq!(got.get(&2035047877), Some(&9002));
        assert_eq!(
            watermark(&db).as_deref(),
            Some("2000"),
            "the watermark must advance to the newest mtime across every dir"
        );
    }

    #[test]
    fn launcher_refresh_never_moves_the_watermark_backwards() {
        // The always-parse-the-newest-log rule still reads the file,
        // so the pair lands - but a stale mtime must not rewind the
        // watermark and cause GB-scale re-reads on the next copy.
        let (tmp, db) = test_db();
        db.set_setting("launcher_logs_parsed_through", "5000")
            .unwrap();
        let logs = tmp.path().join("logs");
        write_log(&logs, "eve-online-launcher.log", 9001, 2035047876, 1000);

        refresh_launcher_map_from(&db, &[logs]).unwrap();

        assert_eq!(
            db.lookup_user_ids(&[2035047876]).unwrap().get(&2035047876),
            Some(&9001),
            "the newest log is always parsed regardless of the watermark"
        );
        assert_eq!(watermark(&db).as_deref(), Some("5000"));
    }

    #[test]
    fn launcher_refresh_persists_and_reuses_log_cursors() {
        let (_tmp, db) = test_db();
        let logs = TempDir::new().unwrap();
        write_log(
            logs.path(),
            "eve-online-launcher.log",
            9001,
            2035047876,
            1000,
        );

        refresh_launcher_map_from(&db, &[logs.path().to_path_buf()]).unwrap();

        let stored = db.get_setting("launcher_log_cursors").unwrap().unwrap();
        assert!(
            stored.contains("eve-online-launcher.log"),
            "the active log's cursor must be persisted: {stored}"
        );

        // A second refresh with nothing appended must keep working
        // (and, per the cursor, read no content at all).
        refresh_launcher_map_from(&db, &[logs.path().to_path_buf()]).unwrap();
        assert_eq!(
            db.lookup_user_ids(&[2035047876]).unwrap().get(&2035047876),
            Some(&9001)
        );
    }

    #[test]
    fn launcher_refresh_with_no_dirs_writes_nothing() {
        let (_tmp, db) = test_db();

        refresh_launcher_map_from(&db, &[]).unwrap();

        assert_eq!(watermark(&db), None);
    }

    // ---------------------------- floors ------------------------------

    #[test]
    fn floors_are_raised_to_shipped_defaults_and_calibration_wins() {
        let observed = HashMap::from([
            // Observed smaller than the shipped market floor (a user
            // at a smaller UI scale): the shipped prior raises it.
            ("market".to_string(), (500, 300)),
            // Observed larger (bigger UI scale): observation stands.
            ("mail".to_string(), (700, 500)),
            // No shipped prior: observation passes through.
            ("overview".to_string(), (200, 150)),
            // Family rule: any chat channel gets the channel prior.
            ("chatchannel_player_abc".to_string(), (100, 100)),
            // A stack id has no prior even though it is all digits.
            ("153".to_string(), (90, 80)),
        ]);
        let calibrated = vec![("market".to_string(), 640, 420)];

        let f = merge_floors(observed, &calibrated);

        assert_eq!(
            f["market"],
            (640, 420),
            "local calibration replaces even the shipped prior"
        );
        assert_eq!(f["mail"], (700, 500));
        assert_eq!(f["overview"], (200, 150));
        assert_eq!(f["chatchannel_player_abc"], (188, 120));
        assert_eq!(f["153"], (90, 80));
    }

    // ---------------------- min-size calibration ----------------------

    /// Measure EVE's true per-window minimum sizes by letting the game
    /// clamp deliberately-tiny windows.
    ///
    /// Stage (writes every recorded window of one character to 1x1,
    /// with a byte-exact .bak):
    ///   REPLICATOR_MIN_PROBE="stage:<character name>" cargo test --lib min_size_probe -- --ignored --nocapture
    /// Then log the character in, touch nothing, log out. Then:
    ///   REPLICATOR_MIN_PROBE=harvest cargo test --lib min_size_probe -- --ignored --nocapture
    /// harvests every clamped size into the window_floors table (the
    /// designer picks them up automatically) and restores the .bak.
    #[test]
    #[ignore = "rewrites one real settings file (with backup); run explicitly"]
    fn min_size_probe() {
        use crate::eve::{paths, settings};
        use blue_marshal::{decode, encode, EncodeOptions, Value};

        let Ok(mode) = std::env::var("REPLICATOR_MIN_PROBE") else {
            return;
        };
        ops::ensure_not_running().expect("close EVE first");

        let installs = paths::discover(None);
        let profiles = enumerate(&installs).unwrap();
        let db_path = dirs::data_dir()
            .map(|d| d.join("rip.replicator/replicator.sqlite"))
            .filter(|p| p.exists())
            .expect("app database not found");
        let db = crate::db::Db::open(&db_path).unwrap();

        if mode == "harvest" {
            // The staged file is the one with a .bak sibling.
            let staged = profiles
                .iter()
                .filter(|p| matches!(p, ProfileFile::Character { .. }))
                .find(|p| p.path().with_extension("dat.bak").exists())
                .expect("no staged character found (no .dat.bak sibling)");
            let bytes = std::fs::read(staged.path()).unwrap();
            let sizes = settings::geometry_sizes(&bytes).unwrap();
            let clamped: Vec<(String, i64, i64)> = sizes
                .into_iter()
                .filter(|(_, w, h)| *w > 1 || *h > 1)
                .collect();
            println!("clamped minima learned ({}):", clamped.len());
            let mut sorted = clamped.clone();
            sorted.sort();
            for (name, w, h) in &sorted {
                println!("  {name}: {w} x {h}");
            }
            db.set_window_floors(&clamped).unwrap();

            let bak = staged.path().with_extension("dat.bak");
            std::fs::copy(&bak, staged.path()).unwrap();
            std::fs::remove_file(&bak).unwrap();
            println!("layout restored from {}", bak.display());
            return;
        }

        let Some(name) = mode.strip_prefix("stage:") else {
            panic!("REPLICATOR_MIN_PROBE must be stage:<name> or harvest");
        };
        let char_ids: Vec<i64> = profiles
            .iter()
            .filter_map(|p| match p {
                ProfileFile::Character { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        let names = db.lookup_names(&char_ids).unwrap();
        let id = *names
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(name))
            .map(|(id, _)| id)
            .expect("character name not found in the name cache");
        let target = ops::find_character_file(&profiles, id).unwrap();
        let path = target.path().to_path_buf();
        let bak = path.with_extension("dat.bak");
        assert!(!bak.exists(), "a .bak already exists - harvest first");

        let bytes = std::fs::read(&path).unwrap();
        let decoded = decode(&bytes).expect("decode");
        let Value::Dict(mut items) = decoded.value else {
            panic!("not a dict")
        };
        let mut shrunk = 0usize;
        for (k, v) in items.iter_mut() {
            let is_windows = matches!(k, Value::Str(s) if s == "windows")
                || matches!(k, Value::Bytes(b) if b == b"windows");
            if !is_windows {
                continue;
            }
            let Value::Dict(entries) = v else { continue };
            for (ek, ev) in entries.iter_mut() {
                let sized = matches!(ek, Value::Str(s) if s.starts_with("windowSizesAndPositions"))
                    || matches!(ek, Value::Bytes(b) if b.starts_with(b"windowSizesAndPositions"));
                if !sized {
                    continue;
                }
                let payload = match ev {
                    Value::Tuple(t) if t.len() == 2 => &mut t[1],
                    other => other,
                };
                let Value::Dict(rows) = payload else { continue };
                for (_, rv) in rows.iter_mut() {
                    let cell = match rv {
                        Value::Tuple(t) if t.len() == 2 => &mut t[1],
                        other => other,
                    };
                    let Value::Tuple(t) = cell else { continue };
                    if t.len() < 6 {
                        continue;
                    }
                    t[2] = Value::Int(1);
                    t[3] = Value::Int(1);
                    shrunk += 1;
                }
            }
        }
        let merged = Value::Dict(items);
        let out = encode(&merged, &EncodeOptions::default()).unwrap();
        assert_eq!(
            decode(&out).unwrap().value,
            merged,
            "round-trip check failed; refusing to write"
        );

        std::fs::write(&bak, &bytes).unwrap();
        ops::write_profile_bytes(&path, &out).unwrap();
        println!("staged: {name} (#{id}) - {shrunk} windows set to 1x1");
        println!("backup: {}", bak.display());
        println!("now: log {name} in, touch nothing, log out, then run harvest.");
    }

    // ----------------------- in-game probe staging --------------------

    /// Stage the in-game verification: rewrite ONE character's file
    /// with its own content, re-encoded (v1). Content-lossless by the
    /// sweep's proof; the login then answers only "does the client
    /// read v1". A byte-exact .bak sits next to the file.
    ///
    ///   REPLICATOR_STAGE_V1=auto cargo test --lib stage_v1_probe -- --ignored --nocapture
    ///   REPLICATOR_STAGE_V1=<char_id> ... to pick a specific character
    ///   REPLICATOR_RESTORE_V1=1 ...      to put the original bytes back
    #[test]
    #[ignore = "rewrites one real settings file (with backup); run explicitly"]
    fn stage_v1_probe() {
        use crate::eve::{paths, settings};

        let stage = std::env::var("REPLICATOR_STAGE_V1").ok();
        let restore = std::env::var("REPLICATOR_RESTORE_V1").is_ok();
        if stage.is_none() && !restore {
            return;
        }
        ops::ensure_not_running().expect("close EVE first");

        let installs = paths::discover(None);
        let profiles = enumerate(&installs).unwrap();

        if restore {
            let mut restored = 0;
            for p in &profiles {
                let bak = p.path().with_extension("dat.bak");
                if bak.exists() {
                    std::fs::copy(&bak, p.path()).unwrap();
                    std::fs::remove_file(&bak).unwrap();
                    println!("restored {}", p.path().display());
                    restored += 1;
                }
            }
            println!("{restored} file(s) restored from .bak");
            return;
        }

        let want = stage.unwrap();
        let target = if want == "auto" {
            // The least-recently-modified character: the least active,
            // so even a surprise costs the least.
            profiles
                .iter()
                .filter(|p| matches!(p, ProfileFile::Character { .. }))
                .min_by_key(|p| match p {
                    ProfileFile::Character { modified, .. } => modified.unwrap_or(i64::MAX),
                    _ => i64::MAX,
                })
                .expect("no character files found")
        } else {
            let id: i64 = want
                .parse()
                .expect("REPLICATOR_STAGE_V1 must be auto or a char id");
            ops::find_character_file(&profiles, id).expect("character not found")
        };
        let (id, path) = match target {
            ProfileFile::Character { id, path, .. } => (*id, path.clone()),
            _ => unreachable!(),
        };

        let bytes = std::fs::read(&path).unwrap();
        let bak = path.with_extension("dat.bak");
        assert!(
            !bak.exists(),
            "a .bak already exists - restore or remove it before staging again"
        );

        let groups = settings::list_groups(&bytes).expect("file must decode");
        let v1 = settings::selective_merge(&bytes, &bytes, &groups).expect("lossless re-encode");
        let check = settings::diff_settings(&bytes, &v1).expect("diff");
        assert!(check.is_empty(), "re-encode must be semantically identical");

        std::fs::write(&bak, &bytes).unwrap();
        ops::write_profile_bytes(&path, &v1).unwrap();

        // Best-effort name so the human knows who to log in.
        let name = dirs::data_dir()
            .map(|d| d.join("rip.replicator/replicator.sqlite"))
            .filter(|p| p.exists())
            .and_then(|p| crate::db::Db::open(&p).ok())
            .and_then(|db| db.lookup_names(&[id]).ok())
            .and_then(|m| m.get(&id).cloned())
            .unwrap_or_else(|| format!("character {id}"));

        println!("staged: {name} (#{id})");
        println!(
            "file:   {} ({} -> {} bytes, v1)",
            path.display(),
            bytes.len(),
            v1.len()
        );
        println!("backup: {}", bak.display());
        println!("log this character in; if the UI looks exactly as always, v1 is proven.");
        println!("undo:   REPLICATOR_RESTORE_V1=1 cargo test --lib stage_v1_probe -- --ignored --nocapture");
    }

    // ------------------------- offline QA sweep -----------------------

    /// The offline half of that verification: run the real code paths against
    /// every real settings file on this machine. Reads the live dirs,
    /// writes only to scratch. Not part of the normal run:
    ///   cargo test --lib offline_qa_sweep -- --ignored --nocapture
    #[test]
    #[ignore = "sweeps the machine's real EVE settings; run explicitly"]
    fn offline_qa_sweep() {
        use crate::eve::{paths, settings};

        let installs = paths::discover(None);
        let profiles = enumerate(&installs).unwrap();
        println!(
            "installs: {}  profile files: {}",
            installs.len(),
            profiles.len()
        );
        assert!(
            !profiles.is_empty(),
            "no settings files found - nothing to sweep"
        );

        // 1. Decode + self-merge every file through the production
        //    merge path. selective_merge already refuses to return
        //    bytes that fail the semantic round-trip; diffing the
        //    result against the original proves the merge changed
        //    nothing.
        let mut decodable = 0usize;
        let mut undecodable: Vec<String> = Vec::new();
        let mut merge_failures: Vec<String> = Vec::new();
        let mut layouts = 0usize;
        for p in &profiles {
            let name = p.path().file_name().unwrap().to_string_lossy().to_string();
            let bytes = std::fs::read(p.path()).unwrap();
            let groups = match settings::list_groups(&bytes) {
                Ok(g) => {
                    decodable += 1;
                    g
                }
                Err(e) => {
                    undecodable.push(format!("{name}: {e}"));
                    continue;
                }
            };
            match settings::selective_merge(&bytes, &bytes, &groups) {
                Ok(merged) => match settings::diff_settings(&bytes, &merged) {
                    Ok(d) if d.is_empty() => {}
                    Ok(d) => merge_failures.push(format!("{name}: self-merge drifted: {d:?}")),
                    Err(e) => merge_failures.push(format!("{name}: diff failed: {e}")),
                },
                Err(e) => merge_failures.push(format!("{name}: merge failed: {e}")),
            }
            if let Ok(Some(_)) = settings::window_layout(&bytes) {
                layouts += 1;
            }
        }
        println!(
            "decodable: {decodable}/{}  window layouts: {layouts}",
            profiles.len()
        );
        for u in &undecodable {
            println!("undecodable: {u}");
        }
        assert!(
            merge_failures.is_empty(),
            "self-merge must be lossless on every real file: {merge_failures:#?}"
        );

        // 2. Zip round trip with the real files: export everything,
        //    clone the tree to scratch, wreck every file, import, and
        //    demand byte-identical restoration.
        let tmp = TempDir::new().unwrap();
        let zip = tmp.path().join("qa.zip");
        let names_map: HashMap<i64, String> = HashMap::new();
        let summary = crate::portable::export_setup(&zip, &profiles, &names_map, 0).unwrap();
        println!(
            "zip export: {} files, {} characters",
            summary.files, summary.characters
        );

        let scratch = tmp.path().join("machine_b");
        let mut scratch_profiles: Vec<ProfileFile> = Vec::new();
        let mut originals: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
        for p in &profiles {
            let sd = p.settings_dir();
            let parent = sd.parent().and_then(|d| d.file_name()).unwrap_or_default();
            let dir = scratch.join(parent).join(sd.file_name().unwrap());
            std::fs::create_dir_all(&dir).unwrap();
            let dest = dir.join(p.path().file_name().unwrap());
            let bytes = std::fs::read(p.path()).unwrap();
            std::fs::write(&dest, b"WRECKED").unwrap();
            originals.push((dest.clone(), bytes));
            let sp = match p {
                ProfileFile::Character { id, .. } => ProfileFile::Character {
                    id: *id,
                    path: dest,
                    install_root: scratch.clone(),
                    settings_dir: dir.clone(),
                    modified: None,
                    size: 0,
                },
                ProfileFile::User { id, .. } => ProfileFile::User {
                    id: *id,
                    path: dest,
                    install_root: scratch.clone(),
                    settings_dir: dir.clone(),
                    modified: None,
                    size: 0,
                },
            };
            scratch_profiles.push(sp);
        }
        let report = crate::portable::import_setup(&zip, &scratch_profiles).unwrap();
        println!(
            "zip import: {} written, {} skipped",
            report.written.len(),
            report.skipped.len()
        );
        for s in report.skipped.iter().take(10) {
            println!("import skip: {s}");
        }
        let mut mismatches = 0usize;
        let mut still_wrecked = 0usize;
        for (path, original) in &originals {
            let restored = std::fs::read(path).unwrap();
            if restored == b"WRECKED" {
                still_wrecked += 1;
            } else if &restored != original {
                mismatches += 1;
                println!("byte mismatch after import: {}", path.display());
            }
        }
        println!(
            "restored byte-identical: {}/{}",
            originals.len() - mismatches - still_wrecked,
            originals.len()
        );
        // Same-id collapses (one char in two profiles) legitimately
        // leave the shadowed copy untouched; corruption never is.
        assert_eq!(mismatches, 0, "an imported file must be byte-identical");
    }

    // ---------------------- free_template_name ------------------------

    #[test]
    fn imported_template_names_suffix_past_collisions() {
        let taken = |names: &[&str]| {
            names
                .iter()
                .map(|s| s.to_string())
                .collect::<std::collections::HashSet<_>>()
        };

        assert_eq!(free_template_name("Doctrine", &taken(&[])), "Doctrine");
        assert_eq!(
            free_template_name("Doctrine", &taken(&["Doctrine"])),
            "Doctrine (2)"
        );
        assert_eq!(
            free_template_name(
                "Doctrine",
                &taken(&["Doctrine", "Doctrine (2)", "Doctrine (3)"])
            ),
            "Doctrine (4)",
            "every taken suffix is skipped, not just the first"
        );
        // A shelf already holding "(2)" but not the base takes the base.
        assert_eq!(
            free_template_name("Doctrine", &taken(&["Doctrine (2)"])),
            "Doctrine"
        );
    }

    // --------------------- build_accounts_response --------------------

    fn meta(user_id: i64, alias: Option<&str>, note: Option<&str>) -> crate::db::AccountMeta {
        crate::db::AccountMeta {
            user_id,
            alias: alias.map(str::to_string),
            note: note.map(str::to_string),
        }
    }

    fn pairing(user_id: i64, manual: bool, log_user_id: Option<i64>) -> crate::db::CharPairing {
        crate::db::CharPairing {
            user_id,
            manual,
            log_user_id,
        }
    }

    #[test]
    fn accounts_group_characters_under_their_mapped_user() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![
            usr(&dir, 9001),
            ch(&dir, 1001, 100, 5),
            ch(&dir, 1002, 100, 5),
            ch(&dir, 2001, 100, 5),
        ];
        let pairs = HashMap::from([
            (1001, pairing(9001, false, Some(9001))),
            (1002, pairing(9001, true, None)),
            (2001, pairing(9002, false, Some(9002))),
        ]);

        let out = build_accounts_response(
            &profiles,
            &pairs,
            &names(&[(1001, "Alice"), (1002, "Bob")]),
            &[meta(9001, Some("Main"), Some("the good one"))],
        );

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].user_id, 9001);
        assert_eq!(out[0].alias.as_deref(), Some("Main"));
        assert_eq!(out[0].note.as_deref(), Some("the good one"));
        assert_eq!(
            out[0]
                .characters
                .iter()
                .map(|c| c.name.as_deref().unwrap_or(""))
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert!(!out[0].characters[0].manual);
        assert!(
            out[0].characters[1].manual,
            "a hand-set pairing must reach the UI marked as such"
        );
        // 9002 has no live user file but the mapping knows it: its
        // character still deserves a row rather than vanishing.
        assert_eq!(out[1].user_id, 9002);
        assert_eq!(out[1].modified, None);
        assert_eq!(out[1].characters.len(), 1);
    }

    #[test]
    fn an_account_with_no_mapped_characters_still_lists() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![usr(&dir, 9001), ch(&dir, 1001, 100, 5)];

        let out = build_accounts_response(&profiles, &HashMap::new(), &names(&[]), &[]);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].user_id, 9001);
        assert!(out[0].characters.is_empty());
    }

    #[test]
    fn duplicate_profiles_do_not_duplicate_account_characters() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");
        let b = settings_dir(tmp.path(), "settings_Alt");
        let profiles = vec![usr(&a, 9001), ch(&a, 1001, 100, 5), ch(&b, 1001, 200, 5)];
        let pairs = HashMap::from([(1001, pairing(9001, false, Some(9001)))]);

        let out = build_accounts_response(&profiles, &pairs, &names(&[]), &[]);

        assert_eq!(out[0].characters.len(), 1);
    }

    #[test]
    fn a_hand_set_pairing_the_logs_contradict_is_flagged() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        let profiles = vec![usr(&dir, 9001), ch(&dir, 1001, 100, 5)];
        let pairs = HashMap::from([(1001, pairing(9001, true, Some(8888)))]);

        let out = build_accounts_response(&profiles, &pairs, &names(&[]), &[]);

        let c = &out[0].characters[0];
        assert!(c.disputed);
        assert_eq!(c.log_user_id, Some(8888));

        // Agreement is not a dispute, and neither is log silence.
        let pairs = HashMap::from([(1001, pairing(9001, true, Some(9001)))]);
        let out = build_accounts_response(&profiles, &pairs, &names(&[]), &[]);
        assert!(!out[0].characters[0].disputed);
        let pairs = HashMap::from([(1001, pairing(9001, true, None))]);
        let out = build_accounts_response(&profiles, &pairs, &names(&[]), &[]);
        assert!(!out[0].characters[0].disputed);
    }

    // ------------------------- attach_accounts ------------------------

    #[test]
    fn characters_pick_up_their_account_and_alias() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        let mut resp = build_characters_response(
            &[ch(&dir, 1001, 100, 5), ch(&dir, 2001, 100, 5)],
            &names(&[]),
        );
        let pairs = HashMap::from([(1001, 9001)]);
        let aliases = HashMap::from([(9001, "Main".to_string())]);

        attach_accounts(&mut resp, &pairs, &aliases);

        let alice = resp.characters.iter().find(|c| c.id == 1001).unwrap();
        assert_eq!(alice.user_id, Some(9001));
        assert_eq!(alice.account_alias.as_deref(), Some("Main"));
        let stranger = resp.characters.iter().find(|c| c.id == 2001).unwrap();
        assert_eq!(stranger.user_id, None);
        assert_eq!(stranger.account_alias, None);
    }
}

#[tauri::command]
pub fn play_jazz_chord() {
    use rodio::buffer::SamplesBuffer;
    use rodio::{OutputStream, Sink};
    use std::thread;

    thread::spawn(|| {
        let Ok((_stream, handle)) = OutputStream::try_default() else {
            return;
        };

        // First 5 chords of John Coltrane - "Giant Steps":
        //   | Bmaj7   D7    | Gmaj7   Bb7   | Ebmaj7
        // Classic Coltrane changes - descending major thirds
        // (B -> G -> Eb) linked by V7 dominants. Each chord is voiced
        // with the actual head-melody note as the top voice:
        //   F#5 -> D5 -> B4 -> G4 -> Bb4
        let chords: [&[f32]; 5] = [
            // Bmaj7   (B3 D#4 F#4 A#4 + F#5 melody = the 5th)
            &[246.94, 311.13, 369.99, 466.16, 739.99],
            // D7      (D4 F#4 A4 C5 + D5 melody = root oct-up)
            &[293.66, 369.99, 440.00, 523.25, 587.33],
            // Gmaj7   (G3 B3 D4 F#4 + B4 melody = the 3rd)
            &[196.00, 246.94, 293.66, 369.99, 493.88],
            // Bb7/13  (Bb3 D4 F4 + G4 melody = the 13th; no 7 to
            //          avoid the Ab-G half-step clash)
            &[233.08, 293.66, 349.23, 392.00],
            // Ebmaj7  (Eb3 G3 D4 + Bb4 melody = the 5th, whole note)
            &[155.56, 196.00, 293.66, 466.16],
        ];

        let chord_step_s = 0.55_f32; // next chord enters
        let chord_ring_s = 0.75_f32; // each chord rings
        let final_ring_s = 1.60_f32; // final chord rings longer
        let fade_in_s = 0.040_f32;
        let per_voice_gain = 0.035_f32;
        let tail_pad_s = 0.20_f32; // silence at end so audio buffer drains
        let sample_rate: u32 = 48_000;

        // Pre-render the entire progression into one PCM buffer.
        // Single Sink, no concurrent sinks, no per-chord allocations
        // during playback - eliminates the cpal/rodio mixer glitches
        // that come from juggling ~24 simultaneous sinks.
        let total_s = chord_step_s * (chords.len() as f32 - 1.0) + final_ring_s + tail_pad_s;
        let total_samples = (sample_rate as f32 * total_s).ceil() as usize;
        let mut buf: Vec<f32> = vec![0.0; total_samples];

        let two_pi = 2.0 * std::f32::consts::PI;
        let fade_in_samples = (sample_rate as f32 * fade_in_s) as usize;

        for (chord_i, chord_freqs) in chords.iter().enumerate() {
            let is_last = chord_i == chords.len() - 1;
            let dur_s = if is_last { final_ring_s } else { chord_ring_s };
            let voice_samples = (sample_rate as f32 * dur_s) as usize;
            let start_sample = (sample_rate as f32 * chord_step_s * chord_i as f32) as usize;

            for &freq in chord_freqs.iter() {
                let phase_inc = two_pi * freq / sample_rate as f32;
                let mut phase = 0.0_f32;
                let voice_samples_f = voice_samples as f32;
                let fade_in_f = fade_in_samples.max(1) as f32;
                for i in 0..voice_samples {
                    let dst = start_sample + i;
                    if dst >= buf.len() {
                        break;
                    }
                    let fade_gain = if i < fade_in_samples {
                        i as f32 / fade_in_f
                    } else {
                        1.0
                    };
                    let ramp_gain = 1.0 - (i as f32 / voice_samples_f);
                    buf[dst] += phase.sin() * per_voice_gain * fade_gain * ramp_gain;
                    phase += phase_inc;
                    if phase > two_pi {
                        phase -= two_pi;
                    }
                }
            }
        }

        // Safety: if any sample crept above ~0.9 from constructive
        // interference, scale the whole buffer down so we don't clip.
        let peak = buf.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
        if peak > 0.9 {
            let scale = 0.9 / peak;
            for s in &mut buf {
                *s *= scale;
            }
        }

        let Ok(sink) = Sink::try_new(&handle) else {
            return;
        };
        sink.append(SamplesBuffer::new(1, sample_rate, buf));
        sink.sleep_until_end();
    });
}

/// One-shot at app startup. Errors surface to the frontend as a
/// rejected promise, which renders as no badge at all - offline or
/// rate-limited must not mean a scary banner.
#[tauri::command]
pub async fn check_update() -> AppResult<crate::update::UpdateCheck> {
    crate::update::fetch_update_check().await
}
