use crate::error::{AppError, AppResult};
use crate::eve::characters::ProfileFile;
use git2::{Cred, IndexAddOption, PushOptions, RemoteCallbacks, Repository, Signature};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize, Clone)]
pub struct MirrorStatus {
    pub initialized: bool,
    pub head: Option<String>,
    pub remote: Option<String>,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct CommitEntry {
    pub oid: String,
    pub short: String,
    pub summary: String,
    pub timestamp: i64,
    pub author: String,
}

const DEFAULT_BRANCH: &str = "main";

pub fn ensure_repo(path: &Path) -> AppResult<Repository> {
    if path.join(".git").exists() {
        Ok(Repository::open(path)?)
    } else {
        std::fs::create_dir_all(path)?;
        let repo = Repository::init(path)?;
        {
            let mut cfg = repo.config()?;
            let _ = cfg.set_str("commit.gpgsign", "false");
            let _ = cfg.set_str("user.name", "Replicator");
            let _ = cfg.set_str("user.email", "replicator@local");
        }
        let head_path = repo.path().join("HEAD");
        std::fs::write(head_path, format!("ref: refs/heads/{DEFAULT_BRANCH}\n"))?;
        Ok(repo)
    }
}

pub fn status(mirror: &Path) -> AppResult<MirrorStatus> {
    if !mirror.join(".git").exists() {
        return Ok(MirrorStatus {
            initialized: false,
            head: None,
            remote: None,
            branch: DEFAULT_BRANCH.to_string(),
            dirty: false,
        });
    }
    let repo = Repository::open(mirror)?;
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .map(|o| o.to_string());
    let remote = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(|s| s.to_string()));
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    let dirty = !repo.statuses(None).map(|s| s.is_empty()).unwrap_or(true);
    Ok(MirrorStatus {
        initialized: true,
        head,
        remote,
        branch,
        dirty,
    })
}

/// The bundled git speaks HTTPS (and local paths), not SSH: libgit2 is
/// built without the ssh transport, so a `git@github.com:` remote would
/// only fail at push time with a transport error nobody can act on.
fn check_remote_url(url: &str) -> AppResult<()> {
    let lower = url.trim().to_ascii_lowercase();
    let scp_like = !lower.contains("://")
        && lower
            .split_once('@')
            .is_some_and(|(_, rest)| rest.contains(':'));
    if lower.starts_with("ssh://")
        || lower.starts_with("git://")
        || lower.starts_with("git+ssh://")
        || scp_like
    {
        return Err(AppError::Config(
            "only HTTPS remotes are supported - use https://github.com/you/repo.git with a PAT; SSH remotes cannot be pushed from Replicator".into(),
        ));
    }
    Ok(())
}

pub fn set_remote(mirror: &Path, url: &str) -> AppResult<()> {
    check_remote_url(url)?;
    let repo = ensure_repo(mirror)?;
    if repo.find_remote("origin").is_ok() {
        repo.remote_set_url("origin", url)?;
    } else {
        repo.remote("origin", url)?;
    }
    Ok(())
}

pub fn snapshot(
    mirror: &Path,
    profiles: &[ProfileFile],
    name_lookup: &std::collections::HashMap<i64, String>,
    message: &str,
) -> AppResult<Option<String>> {
    let repo = ensure_repo(mirror)?;
    sync_tree(mirror, profiles, name_lookup)?;

    let mut index = repo.index()?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    index.write()?;
    let tree_oid = index.write_tree()?;

    let parent = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|o| repo.find_commit(o).ok());

    if let Some(p) = parent.as_ref() {
        if p.tree_id() == tree_oid {
            return Ok(None);
        }
    }

    let tree = repo.find_tree(tree_oid)?;
    let sig = Signature::now("Replicator", "replicator@local")?;
    let parents: Vec<&git2::Commit> = parent.as_ref().into_iter().collect();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
    Ok(Some(oid.to_string()))
}

/// Mirror layout: `characters/<profile>/<label>_<id>.dat` and
/// `users/<profile>/user_<id>.dat`, where `<profile>` is the sanitized
/// basename of the settings dir the file lives in (`settings_Default`,
/// `settings_Alt`, ...). The same character can hold different settings
/// in different profile dirs; keying by id alone would silently keep
/// only one of them. The basename - not the full path - is used so a
/// clone restores onto another machine, where the same profiles live
/// under different absolute paths. Two settings dirs sharing a basename
/// (two installs with the same profile name) still collapse; that is
/// the remaining known flattening.
fn sync_tree(
    mirror: &Path,
    profiles: &[ProfileFile],
    name_lookup: &std::collections::HashMap<i64, String>,
) -> AppResult<()> {
    let chars_dir = mirror.join("characters");
    let users_dir = mirror.join("users");
    if chars_dir.exists() {
        clear_dir_contents(&chars_dir)?;
    } else {
        std::fs::create_dir_all(&chars_dir)?;
    }
    if users_dir.exists() {
        clear_dir_contents(&users_dir)?;
    } else {
        std::fs::create_dir_all(&users_dir)?;
    }

    for p in profiles {
        let profile = profile_dir_label(p.settings_dir());
        match p {
            ProfileFile::Character { id, path, .. } => {
                let label = name_lookup
                    .get(id)
                    .map(|n| sanitize(n))
                    .unwrap_or_else(|| id.to_string());
                let dir = chars_dir.join(&profile);
                std::fs::create_dir_all(&dir)?;
                let dest = dir.join(format!("{label}_{id}.dat"));
                std::fs::copy(path, dest)?;
            }
            ProfileFile::User { id, path, .. } => {
                let dir = users_dir.join(&profile);
                std::fs::create_dir_all(&dir)?;
                let dest = dir.join(format!("user_{id}.dat"));
                std::fs::copy(path, dest)?;
            }
        }
    }
    Ok(())
}

/// The mirror subtree a profile file belongs to: the sanitized basename
/// of its settings dir, prefixed with the server for anything not on
/// Tranquility so a Singularity `settings_Default` cannot collide with
/// the Tranquility one. Tranquility stays unprefixed to keep existing
/// mirrors byte-stable.
pub(crate) fn profile_dir_label(settings_dir: &Path) -> String {
    let base = settings_dir
        .file_name()
        .map(|n| sanitize(&n.to_string_lossy()))
        .unwrap_or_else(|| "unknown".to_string());
    match crate::eve::paths::server_of(settings_dir) {
        Some(server) if server != "tranquility" => format!("{server}_{base}"),
        _ => base,
    }
}

fn clear_dir_contents(dir: &Path) -> AppResult<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.file_name().and_then(|n| n.to_str()) == Some(".git") {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

pub(crate) fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '_'
            } else {
                '-'
            }
        })
        .collect()
}

pub fn history(mirror: &Path, limit: usize) -> AppResult<Vec<CommitEntry>> {
    if !mirror.join(".git").exists() {
        return Ok(Vec::new());
    }
    let repo = Repository::open(mirror)?;
    let mut walk = match repo.revwalk() {
        Ok(w) => w,
        Err(_) => return Ok(Vec::new()),
    };
    if walk.push_head().is_err() {
        return Ok(Vec::new());
    }
    walk.set_sorting(git2::Sort::TIME)?;

    let mut out = Vec::new();
    for oid in walk.take(limit) {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let summary = commit.summary().unwrap_or("").to_string();
        let author = commit.author().name().unwrap_or("unknown").to_string();
        out.push(CommitEntry {
            oid: oid.to_string(),
            short: oid.to_string()[..7].to_string(),
            summary,
            timestamp: commit.time().seconds(),
            author,
        });
    }
    Ok(out)
}

/// Whole-file restore; the production path goes through
/// `restore_commit_with`, this wrapper keeps the older tests honest.
#[cfg(test)]
pub fn restore_commit(
    mirror: &Path,
    oid: &str,
    profiles: &[ProfileFile],
) -> AppResult<RestoreReport> {
    restore_commit_with(mirror, oid, profiles, None)
}

/// Restore a commit's files over the live settings. With a selection,
/// only the chosen groups travel back in time: the historical bytes
/// are decoded and merged into the live file, so every unselected
/// group keeps its present-day value.
pub fn restore_commit_with(
    mirror: &Path,
    oid: &str,
    profiles: &[ProfileFile],
    selection: Option<&crate::eve::ops::CopySelection>,
) -> AppResult<RestoreReport> {
    let repo = Repository::open(mirror)?;
    let oid = git2::Oid::from_str(oid).map_err(|e| AppError::Other(format!("bad oid: {e}")))?;
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;

    let mut report = RestoreReport::default();

    for p in profiles {
        let profile = profile_dir_label(p.settings_dir());
        let (blob, groups, describe) = match p {
            ProfileFile::Character { id, path, .. } => {
                let needle = format!("_{id}.dat");
                let blob = find_profile_blob(&repo, &tree, "characters", &profile, &needle)?;
                let groups = selection.map(|s| s.char_groups.as_slice());
                (blob, groups, (format!("character {id}"), path))
            }
            ProfileFile::User { id, path, .. } => {
                let target = format!("user_{id}.dat");
                let blob = find_profile_blob(&repo, &tree, "users", &profile, &target)?;
                let groups = selection.map(|s| s.user_groups.as_slice());
                (blob, groups, (format!("user {id}"), path))
            }
        };
        let (label, path) = describe;

        // A selection with no groups of this file's kind means "leave
        // these files alone" - not a skip worth reporting.
        if matches!(groups, Some(g) if g.is_empty()) {
            continue;
        }

        let Some(old_bytes) = blob else {
            report.skipped.push(format!("{label} not in commit"));
            continue;
        };

        match groups {
            None => write_restored(path, &old_bytes, &label, &mut report),
            Some(g) => {
                let live = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        report.skipped.push(format!(
                            "{label}: selective restore needs a readable live file ({e})"
                        ));
                        continue;
                    }
                };
                match crate::eve::settings::selective_merge(&old_bytes, &live, g) {
                    Ok(bytes) => write_restored(path, &bytes, &label, &mut report),
                    Err(e) => report
                        .skipped
                        .push(format!("{label}: selective restore failed: {e}")),
                }
            }
        }
    }
    Ok(report)
}

/// One restored file. A failed write goes in the report: the files
/// already put back must not be hidden behind one read-only stray.
fn write_restored(path: &Path, bytes: &[u8], label: &str, report: &mut RestoreReport) {
    match crate::eve::ops::write_profile_bytes(path, bytes) {
        Ok(()) => report.restored += 1,
        Err(e) => report.skipped.push(format!("{label}: write failed ({e})")),
    }
}

/// One profile file's semantic difference within a commit, against the
/// commit's first parent.
#[derive(Debug, Serialize, Clone)]
pub struct FileDiff {
    /// "character" | "user"
    pub kind: String,
    pub id: i64,
    /// The mirror filename's label - the cached character name at
    /// snapshot time, or the bare id.
    pub label: String,
    /// The mirror profile subtree this file lives in - the same
    /// character can change independently per settings profile.
    pub profile: String,
    /// "changed" | "added" | "removed" | "opaque" (undecodable bytes
    /// that differ).
    pub status: String,
    pub groups: Vec<crate::eve::settings::GroupDiff>,
}

/// Every profile-file blob in a tree, keyed by (profile, kind, id):
/// the profile dir keeps per-profile files distinct - collapsing them
/// made a change in the later-sorting profile invisible to the diff -
/// while leaving the label out of the key keeps a renamed character
/// (the filename embeds the cached name) comparing against itself.
type IdentityKey = (String, String, i64);

fn tree_identity_map(
    tree: &git2::Tree,
) -> AppResult<std::collections::BTreeMap<IdentityKey, (String, git2::Oid)>> {
    let mut out = std::collections::BTreeMap::new();
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let (Some(name), oid) = (entry.name(), entry.id()) {
                if let Some((kind, id, label)) = parse_mirror_name(dir, name) {
                    let profile = profile_of_tree_dir(dir);
                    out.entry((profile, kind, id)).or_insert((label, oid));
                }
            }
        }
        git2::TreeWalkResult::Ok
    })?;
    Ok(out)
}

/// The profile component of a tree-walk dir like `characters/settings_Default/`
/// (empty for the legacy flat layout).
fn profile_of_tree_dir(dir: &str) -> String {
    dir.trim_end_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("")
        .to_string()
}

/// (kind, id, label) of a mirror file, or None for anything that is
/// not a profile file. The `label == "user"` clause only applies at
/// the legacy flat root: inside `characters/` a character legitimately
/// named "user" is a character.
fn parse_mirror_name(dir: &str, name: &str) -> Option<(String, i64, String)> {
    let stem = name.strip_suffix(".dat")?;
    let (label, id_str) = stem.rsplit_once('_')?;
    let id: i64 = id_str.parse().ok()?;
    if dir.starts_with("users") || (dir.is_empty() && label == "user") {
        Some(("user".to_string(), id, id_str.to_string()))
    } else {
        let label = if label.is_empty() {
            id_str.to_string()
        } else {
            label.to_string()
        };
        Some(("character".to_string(), id, label))
    }
}

/// What a commit changed, semantically: per character and account
/// file, which settings groups differ from the first parent. The root
/// commit reports everything as added.
pub fn commit_semantic_diff(mirror: &Path, oid: &str) -> AppResult<Vec<FileDiff>> {
    let repo = Repository::open(mirror)?;
    let oid = git2::Oid::from_str(oid).map_err(|e| AppError::Other(format!("bad oid: {e}")))?;
    let commit = repo.find_commit(oid)?;
    let new_map = tree_identity_map(&commit.tree()?)?;
    let old_map = match commit.parent(0) {
        Ok(p) => tree_identity_map(&p.tree()?)?,
        Err(_) => Default::default(),
    };

    let mut out = Vec::new();
    for ((profile, kind, id), (label, new_oid)) in &new_map {
        match old_map.get(&(profile.clone(), kind.clone(), *id)) {
            Some((_, old_oid)) if old_oid == new_oid => {}
            Some((_, old_oid)) => {
                let old_b = repo.find_blob(*old_oid)?;
                let new_b = repo.find_blob(*new_oid)?;
                match crate::eve::settings::diff_settings(old_b.content(), new_b.content()) {
                    Ok(groups) if groups.is_empty() => {}
                    Ok(groups) => out.push(FileDiff {
                        kind: kind.clone(),
                        id: *id,
                        label: label.clone(),
                        profile: profile.clone(),
                        status: "changed".into(),
                        groups,
                    }),
                    Err(_) => out.push(FileDiff {
                        kind: kind.clone(),
                        id: *id,
                        label: label.clone(),
                        profile: profile.clone(),
                        status: "opaque".into(),
                        groups: Vec::new(),
                    }),
                }
            }
            None => out.push(FileDiff {
                kind: kind.clone(),
                id: *id,
                label: label.clone(),
                profile: profile.clone(),
                status: "added".into(),
                groups: Vec::new(),
            }),
        }
    }
    for ((profile, kind, id), (label, _)) in &old_map {
        if !new_map.contains_key(&(profile.clone(), kind.clone(), *id)) {
            out.push(FileDiff {
                kind: kind.clone(),
                id: *id,
                label: label.clone(),
                profile: profile.clone(),
                status: "removed".into(),
                groups: Vec::new(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then(a.id.cmp(&b.id))
            .then(a.profile.cmp(&b.profile))
    });
    Ok(out)
}

/// The union of decodable settings groups across a commit's character
/// and user files, in first-seen file order - what a selective restore
/// can offer.
pub fn commit_groups(mirror: &Path, oid: &str) -> AppResult<(Vec<String>, Vec<String>)> {
    let repo = Repository::open(mirror)?;
    let oid = git2::Oid::from_str(oid).map_err(|e| AppError::Other(format!("bad oid: {e}")))?;
    let commit = repo.find_commit(oid)?;
    let map = tree_identity_map(&commit.tree()?)?;

    let mut chars: Vec<String> = Vec::new();
    let mut users: Vec<String> = Vec::new();
    for ((_, kind, _), (_, blob_oid)) in &map {
        let blob = repo.find_blob(*blob_oid)?;
        let Ok(groups) = crate::eve::settings::list_groups(blob.content()) else {
            continue;
        };
        let into = if kind == "user" {
            &mut users
        } else {
            &mut chars
        };
        for g in groups {
            if !into.contains(&g) {
                into.push(g);
            }
        }
    }
    Ok((chars, users))
}

#[derive(Debug, Default, Serialize)]
pub struct RestoreReport {
    pub restored: usize,
    pub skipped: Vec<String>,
}

/// Look the blob up under `<root>/<profile>/` first (the current
/// layout), then directly under `<root>/` - the flat layout older
/// commits used - so pre-layout history stays restorable.
fn find_profile_blob(
    repo: &Repository,
    tree: &git2::Tree,
    root: &str,
    profile: &str,
    name_suffix_or_eq: &str,
) -> AppResult<Option<Vec<u8>>> {
    let root_tree = match subtree_by_name(repo, tree, root)? {
        Some(t) => t,
        None => return Ok(None),
    };
    if let Some(profile_tree) = subtree_by_name(repo, &root_tree, profile)? {
        if let Some(blob) = find_blob_in_tree(repo, &profile_tree, name_suffix_or_eq)? {
            return Ok(Some(blob));
        }
    }
    find_blob_in_tree(repo, &root_tree, name_suffix_or_eq)
}

fn subtree_by_name<'r>(
    repo: &'r Repository,
    tree: &git2::Tree,
    name: &str,
) -> AppResult<Option<git2::Tree<'r>>> {
    let entry = match tree.get_name(name) {
        Some(e) => e,
        None => return Ok(None),
    };
    let obj = entry.to_object(repo)?;
    Ok(obj.as_tree().cloned())
}

fn find_blob_in_tree(
    repo: &Repository,
    tree: &git2::Tree,
    name_suffix_or_eq: &str,
) -> AppResult<Option<Vec<u8>>> {
    for e in tree.iter() {
        if e.kind() != Some(git2::ObjectType::Blob) {
            continue;
        }
        let name = e.name().unwrap_or("");
        if name == name_suffix_or_eq || name.ends_with(name_suffix_or_eq) {
            let blob = repo.find_blob(e.id())?;
            return Ok(Some(blob.content().to_vec()));
        }
    }
    Ok(None)
}

pub fn push(mirror: &Path, branch: &str, pat: &str) -> AppResult<()> {
    let repo = Repository::open(mirror)?;
    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| AppError::Config("remote 'origin' not configured".into()))?;

    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(move |_url, _user_from_url, _allowed| {
        Cred::userpass_plaintext("x-access-token", pat)
    });

    let mut opts = PushOptions::new();
    opts.remote_callbacks(callbacks);

    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    remote.push(&[refspec.as_str()], Some(&mut opts))?;
    Ok(())
}

/// Credential callbacks that present the PAT when one is stored. With
/// no PAT the transport proceeds anonymously, which works for public
/// HTTPS remotes and local-path remotes (tests).
fn auth_callbacks(pat: Option<String>) -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();
    if let Some(pat) = pat {
        callbacks.credentials(move |_url, _user_from_url, _allowed| {
            Cred::userpass_plaintext("x-access-token", &pat)
        });
    }
    callbacks
}

#[derive(Debug, Serialize)]
pub struct PullReport {
    pub updated: bool,
    pub head: Option<String>,
}

/// Clone `url` into the mirror - the fresh-machine restore path, where
/// the folder the app just created is empty. Refuses an initialized or
/// non-empty mirror rather than clobbering snapshots that were never
/// pushed.
pub fn clone(mirror: &Path, url: &str, pat: Option<String>) -> AppResult<MirrorStatus> {
    check_remote_url(url)?;
    if mirror.join(".git").exists() {
        return Err(AppError::Config(
            "mirror is already initialized - use pull to update it".into(),
        ));
    }
    if mirror.exists() && std::fs::read_dir(mirror)?.next().is_some() {
        return Err(AppError::Config(
            "mirror folder is not empty; move its contents aside before cloning".into(),
        ));
    }

    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(auth_callbacks(pat));
    let repo = git2::build::RepoBuilder::new()
        .fetch_options(fetch)
        .clone(url, mirror)?;
    {
        // Same local identity ensure_repo sets, so snapshots taken in
        // the cloned mirror behave identically.
        let mut cfg = repo.config()?;
        let _ = cfg.set_str("commit.gpgsign", "false");
        let _ = cfg.set_str("user.name", "Replicator");
        let _ = cfg.set_str("user.email", "replicator@local");
    }
    drop(repo);
    status(mirror)
}

/// Fetch origin and fast-forward the current branch. Never merges:
/// when local snapshots and the remote have diverged the caller gets
/// an error instead of a silent rewrite of either history.
pub fn pull(mirror: &Path, pat: Option<String>) -> AppResult<PullReport> {
    let repo = Repository::open(mirror)?;
    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| AppError::Config("remote 'origin' not configured".into()))?;

    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(auth_callbacks(pat));
    remote.fetch(&[] as &[&str], Some(&mut fetch), None)?;

    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_else(|| DEFAULT_BRANCH.to_string());
    let remote_ref = repo
        .find_reference(&format!("refs/remotes/origin/{branch}"))
        .map_err(|_| AppError::Config(format!("origin has no '{branch}' branch to pull from")))?;
    let target = remote_ref
        .target()
        .ok_or_else(|| AppError::Other("origin ref has no target".into()))?;

    let annotated = repo.find_annotated_commit(target)?;
    let (analysis, _) = repo.merge_analysis(&[&annotated])?;

    if analysis.is_up_to_date() {
        return Ok(PullReport {
            updated: false,
            head: Some(target.to_string()),
        });
    }
    if analysis.is_fast_forward() || analysis.is_unborn() {
        // The mirror working tree is derived state (sync_tree rebuilds
        // it on every snapshot), so a forced checkout loses nothing.
        let commit = repo.find_commit(target)?;
        repo.checkout_tree(
            commit.as_object(),
            Some(git2::build::CheckoutBuilder::new().force()),
        )?;
        let refname = format!("refs/heads/{branch}");
        match repo.find_reference(&refname) {
            Ok(mut r) => {
                r.set_target(target, "pull: fast-forward")?;
            }
            Err(_) => {
                repo.reference(&refname, target, true, "pull: create branch")?;
            }
        }
        repo.set_head(&refname)?;
        return Ok(PullReport {
            updated: true,
            head: Some(target.to_string()),
        });
    }
    Err(AppError::Config(
        "local snapshots and the remote have diverged. Push from the machine whose history you want to keep; on the other machine, move the mirror aside and clone again".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A live EVE settings dir plus an empty mirror dir, the two halves
    /// every snapshot/restore test needs.
    struct Env {
        _tmp: TempDir,
        live: PathBuf,
        mirror: PathBuf,
    }

    fn env() -> Env {
        let tmp = TempDir::new().unwrap();
        let live = settings_dir(tmp.path(), "settings_Default");
        let mirror = tmp.path().join("mirror");
        Env {
            _tmp: tmp,
            live,
            mirror,
        }
    }

    fn names(pairs: &[(i64, &str)]) -> HashMap<i64, String> {
        pairs.iter().map(|(i, n)| (*i, n.to_string())).collect()
    }

    /// A real blue.Marshal settings file with integer-valued groups,
    /// for the semantic-diff tests. `write_char`'s plain strings are
    /// deliberately NOT marshal - they exercise the opaque path.
    fn marshal_bytes(groups: &[(&str, i64)]) -> Vec<u8> {
        use blue_marshal::{encode, EncodeOptions, Value};
        let dict = Value::Dict(
            groups
                .iter()
                .map(|(k, v)| (Value::Str((*k).to_string()), Value::Int(*v)))
                .collect(),
        );
        encode(&dict, &EncodeOptions::default()).unwrap()
    }

    fn write_marshal_char(dir: &Path, id: i64, groups: &[(&str, i64)]) {
        std::fs::write(
            dir.join(format!("core_char_{id}.dat")),
            marshal_bytes(groups),
        )
        .unwrap();
    }

    // ----------------------------- sanitize ---------------------------

    #[test]
    fn sanitize_keeps_safe_characters() {
        assert_eq!(sanitize("Alice"), "Alice");
        assert_eq!(sanitize("my-char_01"), "my-char_01");
    }

    #[test]
    fn sanitize_turns_whitespace_into_underscores() {
        assert_eq!(sanitize("Alice Smith"), "Alice_Smith");
        assert_eq!(sanitize("A\tB"), "A_B");
    }

    #[test]
    fn sanitize_replaces_path_separators_and_specials() {
        // A character name containing a slash would otherwise write
        // outside the mirror's characters/ directory.
        assert_eq!(sanitize("a/b"), "a-b");
        assert_eq!(sanitize("../etc"), "---etc");
        assert_eq!(sanitize("Bob's Ship"), "Bob-s_Ship");
    }

    #[test]
    fn sanitize_replaces_non_ascii() {
        assert_eq!(sanitize("\u{d1}o\u{f1}o"), "-o-o");
    }

    // ---------------------------- ensure_repo -------------------------

    #[test]
    fn ensure_repo_initialises_on_the_default_branch() {
        let e = env();

        ensure_repo(&e.mirror).unwrap();

        assert!(e.mirror.join(".git").exists());
        let head = std::fs::read_to_string(e.mirror.join(".git/HEAD")).unwrap();
        assert_eq!(head.trim(), "ref: refs/heads/main");
    }

    #[test]
    fn ensure_repo_is_idempotent() {
        let e = env();
        ensure_repo(&e.mirror).unwrap();
        std::fs::write(e.mirror.join("marker"), "x").unwrap();

        ensure_repo(&e.mirror).unwrap();

        assert!(
            e.mirror.join("marker").exists(),
            "reopening must not reinitialise and discard the working tree"
        );
    }

    // ------------------------------ status ----------------------------

    #[test]
    fn status_of_a_missing_mirror_reports_uninitialised() {
        let e = env();

        let s = status(&e.mirror).unwrap();

        assert!(!s.initialized);
        assert_eq!(s.head, None);
        assert_eq!(s.remote, None);
        assert_eq!(s.branch, "main");
        assert!(!s.dirty);
    }

    #[test]
    fn status_after_a_snapshot_is_clean_with_a_head() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        let s = status(&e.mirror).unwrap();

        assert!(s.initialized);
        assert!(s.head.is_some());
        assert!(
            !s.dirty,
            "everything snapshotted should have been committed"
        );
    }

    #[test]
    fn status_detects_a_dirty_working_tree() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        std::fs::write(
            e.mirror.join("characters/settings_Default/1001_1001.dat"),
            "tampered",
        )
        .unwrap();

        assert!(status(&e.mirror).unwrap().dirty);
    }

    // ----------------------------- set_remote -------------------------

    #[test]
    fn set_remote_creates_then_updates_origin() {
        let e = env();

        set_remote(&e.mirror, "https://example.com/a.git").unwrap();
        assert_eq!(
            status(&e.mirror).unwrap().remote,
            Some("https://example.com/a.git".to_string())
        );

        set_remote(&e.mirror, "https://example.com/b.git").unwrap();
        assert_eq!(
            status(&e.mirror).unwrap().remote,
            Some("https://example.com/b.git".to_string()),
            "editing the remote must replace the URL, not fail on a duplicate"
        );
    }

    #[test]
    fn set_remote_initialises_the_repo_if_needed() {
        let e = env();
        set_remote(&e.mirror, "https://example.com/a.git").unwrap();
        assert!(e.mirror.join(".git").exists());
    }

    #[test]
    fn ssh_remotes_are_refused_with_a_reason() {
        // libgit2 is built without the ssh transport; a git@ remote
        // used to be accepted here and then fail at push with a
        // transport error.
        let e = env();
        for bad in [
            "git@github.com:you/repo.git",
            "ssh://git@github.com/you/repo.git",
            "git://github.com/you/repo.git",
        ] {
            let err = set_remote(&e.mirror, bad).unwrap_err();
            assert!(err.to_string().contains("HTTPS"), "{bad}: {err}");
        }
        assert!(set_remote(&e.mirror, "https://github.com/you/repo.git").is_ok());
        assert!(
            set_remote(&e.mirror, e.mirror.to_str().unwrap()).is_ok(),
            "local paths stay usable"
        );
    }

    // ----------------------------- snapshot ---------------------------

    #[test]
    fn snapshot_writes_both_character_and_user_trees() {
        let e = env();
        write_char(&e.live, 1001, "char bytes");
        write_user(&e.live, 9001, "user bytes");
        let profiles = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];

        snapshot(
            &e.mirror,
            &profiles,
            &names(&[(1001, "Alice Smith")]),
            "first",
        )
        .unwrap();

        assert_eq!(
            read(
                &e.mirror
                    .join("characters/settings_Default/Alice_Smith_1001.dat")
            ),
            "char bytes"
        );
        assert_eq!(
            read(&e.mirror.join("users/settings_Default/user_9001.dat")),
            "user bytes"
        );
    }

    #[test]
    fn snapshot_falls_back_to_the_id_when_the_name_is_unknown() {
        let e = env();
        write_char(&e.live, 1001, "char bytes");
        let profiles = vec![char_profile(&e.live, 1001)];

        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        assert!(e
            .mirror
            .join("characters/settings_Default/1001_1001.dat")
            .exists());
    }

    #[test]
    fn an_unchanged_snapshot_creates_no_commit() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];

        let first = snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();
        let second = snapshot(&e.mirror, &profiles, &names(&[]), "second").unwrap();

        assert!(first.is_some());
        assert_eq!(
            second, None,
            "identical trees must not pile up empty commits in the history"
        );
    }

    #[test]
    fn snapshot_commits_again_once_settings_change() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];
        let first = snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        write_char(&e.live, 1001, "v2");
        let second = snapshot(&e.mirror, &profiles, &names(&[]), "second").unwrap();

        assert!(second.is_some());
        assert_ne!(first, second);
        assert_eq!(
            read(&e.mirror.join("characters/settings_Default/1001_1001.dat")),
            "v2"
        );
    }

    #[test]
    fn snapshot_drops_characters_that_no_longer_exist() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        write_char(&e.live, 2002, "v1");
        let both = vec![char_profile(&e.live, 1001), char_profile(&e.live, 2002)];
        snapshot(&e.mirror, &both, &names(&[]), "first").unwrap();

        // Character 2002 is gone from the install on the next pass.
        let one = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &one, &names(&[]), "second").unwrap();

        assert!(
            !e.mirror
                .join("characters/settings_Default/2002_2002.dat")
                .exists(),
            "sync_tree clears the tree so deletions propagate into the commit"
        );
        assert!(e
            .mirror
            .join("characters/settings_Default/1001_1001.dat")
            .exists());
    }

    #[test]
    fn a_renamed_character_does_not_leave_a_stale_copy() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[(1001, "Old Name")]), "first").unwrap();

        snapshot(
            &e.mirror,
            &profiles,
            &names(&[(1001, "New Name")]),
            "second",
        )
        .unwrap();

        assert!(!e
            .mirror
            .join("characters/settings_Default/Old_Name_1001.dat")
            .exists());
        assert!(e
            .mirror
            .join("characters/settings_Default/New_Name_1001.dat")
            .exists());
    }

    #[test]
    fn snapshot_preserves_binary_content_exactly() {
        let e = env();
        let payload: Vec<u8> = vec![0x00, 0xFF, 0x1B, 0x80, 0x0A];
        std::fs::write(e.live.join("core_char_1001.dat"), &payload).unwrap();
        let profiles = vec![char_profile(&e.live, 1001)];

        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        assert_eq!(
            std::fs::read(e.mirror.join("characters/settings_Default/1001_1001.dat")).unwrap(),
            payload
        );
    }

    // ------------------------------ history ---------------------------

    #[test]
    fn history_of_an_uninitialised_mirror_is_empty() {
        let e = env();
        assert!(history(&e.mirror, 10).unwrap().is_empty());
    }

    #[test]
    fn history_of_an_initialised_but_empty_repo_is_empty() {
        let e = env();
        ensure_repo(&e.mirror).unwrap();
        assert!(
            history(&e.mirror, 10).unwrap().is_empty(),
            "a repo with no commits must not error out of the Git view"
        );
    }

    #[test]
    fn history_returns_newest_first_with_summaries() {
        let e = env();
        let profiles = vec![char_profile(&e.live, 1001)];
        write_char(&e.live, 1001, "v1");
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();
        write_char(&e.live, 1001, "v2");
        snapshot(&e.mirror, &profiles, &names(&[]), "second").unwrap();

        let h = history(&e.mirror, 10).unwrap();

        assert_eq!(h.len(), 2);
        assert_eq!(h[0].summary, "second");
        assert_eq!(h[1].summary, "first");
        assert_eq!(h[0].author, "Replicator");
        assert_eq!(h[0].short.len(), 7);
        assert!(h[0].oid.starts_with(&h[0].short));
    }

    #[test]
    fn history_respects_its_limit() {
        let e = env();
        let profiles = vec![char_profile(&e.live, 1001)];
        for i in 0..5 {
            write_char(&e.live, 1001, &format!("v{i}"));
            snapshot(&e.mirror, &profiles, &names(&[]), &format!("commit {i}")).unwrap();
        }

        assert_eq!(history(&e.mirror, 2).unwrap().len(), 2);
    }

    // -------------------------- restore_commit ------------------------

    #[test]
    fn restore_returns_live_files_to_their_snapshotted_bytes() {
        let e = env();
        write_char(&e.live, 1001, "good config");
        write_user(&e.live, 9001, "good user config");
        let profiles = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[(1001, "Alice")]), "good")
            .unwrap()
            .unwrap();

        // The user then wrecks their UI.
        write_char(&e.live, 1001, "WRECKED");
        write_user(&e.live, 9001, "WRECKED");

        let report = restore_commit(&e.mirror, &oid, &profiles).unwrap();

        assert_eq!(report.restored, 2);
        assert!(report.skipped.is_empty());
        assert_eq!(read(&e.live.join("core_char_1001.dat")), "good config");
        assert_eq!(read(&e.live.join("core_user_9001.dat")), "good user config");
    }

    #[test]
    fn restore_matches_a_character_even_after_it_was_renamed() {
        // Mirror files are named <sanitized_name>_<id>.dat, so restore
        // matches on the id suffix. A rename between snapshot and
        // restore must not orphan the file.
        let e = env();
        write_char(&e.live, 1001, "good config");
        let profiles = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[(1001, "Old Name")]), "good")
            .unwrap()
            .unwrap();
        write_char(&e.live, 1001, "WRECKED");

        let report = restore_commit(&e.mirror, &oid, &profiles).unwrap();

        assert_eq!(report.restored, 1);
        assert_eq!(read(&e.live.join("core_char_1001.dat")), "good config");
    }

    #[test]
    fn restore_reports_characters_absent_from_the_chosen_commit() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let snapshotted = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &snapshotted, &names(&[]), "first")
            .unwrap()
            .unwrap();

        // A character that joined after the snapshot was taken.
        write_char(&e.live, 2002, "newer character");
        let now = vec![char_profile(&e.live, 1001), char_profile(&e.live, 2002)];

        let report = restore_commit(&e.mirror, &oid, &now).unwrap();

        assert_eq!(report.restored, 1);
        assert_eq!(
            report.skipped,
            vec!["character 2002 not in commit".to_string()]
        );
        assert_eq!(
            read(&e.live.join("core_char_2002.dat")),
            "newer character",
            "a character missing from the commit must be left alone, not truncated"
        );
    }

    #[test]
    fn restore_reports_users_absent_from_the_chosen_commit() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let oid = snapshot(
            &e.mirror,
            &[char_profile(&e.live, 1001)],
            &names(&[]),
            "first",
        )
        .unwrap()
        .unwrap();

        write_user(&e.live, 9001, "later user file");
        let now = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];

        let report = restore_commit(&e.mirror, &oid, &now).unwrap();

        assert_eq!(report.skipped, vec!["user 9001 not in commit".to_string()]);
        assert_eq!(read(&e.live.join("core_user_9001.dat")), "later user file");
    }

    #[test]
    fn restore_rejects_a_malformed_oid() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        assert!(restore_commit(&e.mirror, "not-an-oid", &profiles).is_err());
    }

    #[test]
    fn restoring_an_older_commit_walks_history_backwards() {
        let e = env();
        let profiles = vec![char_profile(&e.live, 1001)];
        write_char(&e.live, 1001, "v1");
        let first = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();
        write_char(&e.live, 1001, "v2");
        snapshot(&e.mirror, &profiles, &names(&[]), "second").unwrap();

        restore_commit(&e.mirror, &first, &profiles).unwrap();

        assert_eq!(read(&e.live.join("core_char_1001.dat")), "v1");
    }

    #[test]
    fn restore_preserves_binary_content_exactly() {
        let e = env();
        let payload: Vec<u8> = vec![0x00, 0xFF, 0x1B, 0x80, 0x0A];
        std::fs::write(e.live.join("core_char_1001.dat"), &payload).unwrap();
        let profiles = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();
        std::fs::write(e.live.join("core_char_1001.dat"), b"clobbered").unwrap();

        restore_commit(&e.mirror, &oid, &profiles).unwrap();

        assert_eq!(
            std::fs::read(e.live.join("core_char_1001.dat")).unwrap(),
            payload
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_restore_write_is_reported_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let e = env();
        write_char(&e.live, 1001, "good");
        write_char(&e.live, 2002, "good too");
        let profiles = vec![char_profile(&e.live, 1001), char_profile(&e.live, 2002)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "good")
            .unwrap()
            .unwrap();
        write_char(&e.live, 1001, "WRECKED");
        write_char(&e.live, 2002, "WRECKED");
        let locked = e.live.join("core_char_1001.dat");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();

        let report = restore_commit(&e.mirror, &oid, &profiles).unwrap();

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(report.restored, 1);
        assert_eq!(read(&e.live.join("core_char_2002.dat")), "good too");
        assert!(
            report.skipped.iter().any(|s| s.contains("write failed")),
            "the failed write must be in the report: {:?}",
            report.skipped
        );
    }

    // ------------------------ find_profile_blob -----------------------

    fn commit_tree_of(mirror: &Path, oid: &str) -> (Repository, git2::Oid) {
        let repo = Repository::open(mirror).unwrap();
        let oid = git2::Oid::from_str(oid).unwrap();
        (repo, oid)
    }

    #[test]
    fn find_profile_blob_matches_exactly_and_by_suffix() {
        let e = env();
        write_char(&e.live, 1001, "char bytes");
        write_user(&e.live, 9001, "user bytes");
        let profiles = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[(1001, "Alice")]), "first")
            .unwrap()
            .unwrap();

        let (repo, oid) = commit_tree_of(&e.mirror, &oid);
        let tree = repo.find_commit(oid).unwrap().tree().unwrap();

        // Users match on the exact filename.
        assert_eq!(
            find_profile_blob(&repo, &tree, "users", "settings_Default", "user_9001.dat").unwrap(),
            Some(b"user bytes".to_vec())
        );
        // Characters match on the `_<id>.dat` suffix, since the stored
        // name prefix can change between snapshots.
        assert_eq!(
            find_profile_blob(&repo, &tree, "characters", "settings_Default", "_1001.dat").unwrap(),
            Some(b"char bytes".to_vec())
        );
    }

    #[test]
    fn find_profile_blob_returns_none_for_unknown_names_dirs_and_profiles() {
        let e = env();
        write_char(&e.live, 1001, "char bytes");
        let profiles = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        let (repo, oid) = commit_tree_of(&e.mirror, &oid);
        let tree = repo.find_commit(oid).unwrap().tree().unwrap();

        assert_eq!(
            find_profile_blob(&repo, &tree, "characters", "settings_Default", "_4004.dat").unwrap(),
            None
        );
        assert_eq!(
            find_profile_blob(&repo, &tree, "nonexistent", "settings_Default", "_1001.dat")
                .unwrap(),
            None
        );
        // A profile dir the commit has never seen must not borrow
        // another profile's bytes via the legacy fallback (the root
        // holds only subtrees in the current layout).
        assert_eq!(
            find_profile_blob(&repo, &tree, "characters", "settings_Other", "_1001.dat").unwrap(),
            None
        );
    }

    #[test]
    fn a_longer_id_does_not_satisfy_a_shorter_id_suffix() {
        // Suffix matching could in principle let character 91001 serve
        // a request for 1001; the leading underscore is what prevents it.
        let e = env();
        write_char(&e.live, 91001, "wrong character");
        let profiles = vec![char_profile(&e.live, 91001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        let (repo, oid) = commit_tree_of(&e.mirror, &oid);
        let tree = repo.find_commit(oid).unwrap().tree().unwrap();

        assert_eq!(
            find_profile_blob(&repo, &tree, "characters", "settings_Default", "_1001.dat").unwrap(),
            None
        );
    }

    // --------------------- per-profile mirror layout -------------------

    #[test]
    fn snapshot_keeps_per_profile_variants_distinct() {
        // The same character (and account) can hold different settings
        // in different settings_* dirs; flattening them by id was the
        // bug this layout exists to fix.
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let d2 = settings_dir(tmp.path(), "settings_Alt");
        let mirror = tmp.path().join("mirror");
        write_char(&d1, 1001, "default variant");
        write_char(&d2, 1001, "alt variant");
        write_user(&d1, 9001, "default user");
        write_user(&d2, 9001, "alt user");
        let profiles = vec![
            char_profile(&d1, 1001),
            char_profile(&d2, 1001),
            user_profile(&d1, 9001),
            user_profile(&d2, 9001),
        ];

        snapshot(&mirror, &profiles, &names(&[]), "first").unwrap();

        assert_eq!(
            read(&mirror.join("characters/settings_Default/1001_1001.dat")),
            "default variant"
        );
        assert_eq!(
            read(&mirror.join("characters/settings_Alt/1001_1001.dat")),
            "alt variant"
        );
        assert_eq!(
            read(&mirror.join("users/settings_Default/user_9001.dat")),
            "default user"
        );
        assert_eq!(
            read(&mirror.join("users/settings_Alt/user_9001.dat")),
            "alt user"
        );
    }

    #[test]
    fn restore_returns_each_profile_dir_its_own_variant() {
        let tmp = TempDir::new().unwrap();
        let d1 = settings_dir(tmp.path(), "settings_Default");
        let d2 = settings_dir(tmp.path(), "settings_Alt");
        let mirror = tmp.path().join("mirror");
        write_char(&d1, 1001, "default variant");
        write_char(&d2, 1001, "alt variant");
        let profiles = vec![char_profile(&d1, 1001), char_profile(&d2, 1001)];
        let oid = snapshot(&mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        write_char(&d1, 1001, "WRECKED");
        write_char(&d2, 1001, "WRECKED");

        let report = restore_commit(&mirror, &oid, &profiles).unwrap();

        assert_eq!(report.restored, 2);
        assert_eq!(read(&d1.join("core_char_1001.dat")), "default variant");
        assert_eq!(
            read(&d2.join("core_char_1001.dat")),
            "alt variant",
            "each settings dir must get its own bytes back, not a shared flattened blob"
        );
    }

    #[test]
    fn non_tranquility_profiles_get_a_server_prefixed_mirror_dir() {
        // Singularity and Tranquility both ship a settings_Default;
        // without the prefix they would collide in the mirror exactly
        // the way same-server profiles once did.
        let tmp = TempDir::new().unwrap();
        let tq = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_tq_tranquility"),
            "settings_Default",
        );
        let sisi = settings_dir(
            &tmp.path().join("EVE/c_ccp_eve_sisi_singularity"),
            "settings_Default",
        );
        let mirror = tmp.path().join("mirror");
        write_char(&tq, 1001, "tq bytes");
        write_char(&sisi, 1001, "sisi bytes");
        let profiles = vec![char_profile(&tq, 1001), char_profile(&sisi, 1001)];

        let oid = snapshot(&mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        assert_eq!(
            read(&mirror.join("characters/settings_Default/1001_1001.dat")),
            "tq bytes"
        );
        assert_eq!(
            read(&mirror.join("characters/singularity_settings_Default/1001_1001.dat")),
            "sisi bytes"
        );

        write_char(&tq, 1001, "WRECKED");
        write_char(&sisi, 1001, "WRECKED");
        let report = restore_commit(&mirror, &oid, &profiles).unwrap();

        assert_eq!(report.restored, 2);
        assert_eq!(read(&tq.join("core_char_1001.dat")), "tq bytes");
        assert_eq!(read(&sisi.join("core_char_1001.dat")), "sisi bytes");
    }

    #[test]
    fn restore_reads_the_legacy_flat_layout() {
        // Commits made before the per-profile layout hold blobs directly
        // under characters/ and users/; they must stay restorable.
        let e = env();
        write_char(&e.live, 1001, "WRECKED");
        write_user(&e.live, 9001, "WRECKED");

        let repo = ensure_repo(&e.mirror).unwrap();
        std::fs::create_dir_all(e.mirror.join("characters")).unwrap();
        std::fs::create_dir_all(e.mirror.join("users")).unwrap();
        std::fs::write(e.mirror.join("characters/Alice_1001.dat"), "legacy char").unwrap();
        std::fs::write(e.mirror.join("users/user_9001.dat"), "legacy user").unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("legacy", "legacy@local").unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "legacy layout", &tree, &[])
            .unwrap();

        let profiles = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];
        let report = restore_commit(&e.mirror, &oid.to_string(), &profiles).unwrap();

        assert_eq!(report.restored, 2);
        assert!(report.skipped.is_empty());
        assert_eq!(read(&e.live.join("core_char_1001.dat")), "legacy char");
        assert_eq!(read(&e.live.join("core_user_9001.dat")), "legacy user");
    }

    // --------------------------- clear_dir_contents -------------------

    #[test]
    fn clear_dir_contents_removes_files_and_nested_dirs() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir_all(dir.join("nested/deeper")).unwrap();
        std::fs::write(dir.join("a.dat"), "x").unwrap();
        std::fs::write(dir.join("nested/b.dat"), "x").unwrap();

        clear_dir_contents(&dir).unwrap();

        assert!(dir.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    // --------------------------- clone / pull -------------------------

    /// A "remote": an ordinary mirror repo with one snapshot in it,
    /// reached over the local path transport (no network, no auth).
    fn remote_with_one_commit() -> (TempDir, PathBuf, PathBuf, Vec<ProfileFile>) {
        let tmp = TempDir::new().unwrap();
        let live = settings_dir(tmp.path(), "settings_Default");
        let remote = tmp.path().join("remote");
        write_char(&live, 1001, "v1");
        let profiles = vec![char_profile(&live, 1001)];
        snapshot(&remote, &profiles, &names(&[]), "first").unwrap();
        (tmp, live, remote, profiles)
    }

    #[test]
    fn clone_populates_an_empty_mirror_and_sets_origin() {
        let (tmp, _live, remote, _profiles) = remote_with_one_commit();
        let mirror = tmp.path().join("mirror");
        // The app pre-creates the (empty) mirror dir on startup.
        std::fs::create_dir_all(&mirror).unwrap();

        let s = clone(&mirror, remote.to_str().unwrap(), None).unwrap();

        assert!(s.initialized);
        assert_eq!(s.remote.as_deref(), remote.to_str());
        let h = history(&mirror, 10).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].summary, "first");
    }

    #[test]
    fn clone_refuses_an_initialized_mirror() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        snapshot(
            &e.mirror,
            &[char_profile(&e.live, 1001)],
            &names(&[]),
            "local",
        )
        .unwrap();

        let err = clone(&e.mirror, "https://example.com/x.git", None).unwrap_err();

        assert!(
            err.to_string().contains("already initialized"),
            "an existing history must never be clobbered by a clone, got: {err}"
        );
    }

    #[test]
    fn clone_refuses_a_non_empty_uninitialized_folder() {
        let tmp = TempDir::new().unwrap();
        let mirror = tmp.path().join("mirror");
        std::fs::create_dir_all(&mirror).unwrap();
        std::fs::write(mirror.join("stray.txt"), "x").unwrap();

        let err = clone(&mirror, "https://example.com/x.git", None).unwrap_err();

        assert!(err.to_string().contains("not empty"));
    }

    #[test]
    fn a_cloned_mirror_restores_onto_wrecked_live_files() {
        // The whole point of clone support: fresh machine, clone, restore.
        let (tmp, live, remote, profiles) = remote_with_one_commit();
        let mirror = tmp.path().join("mirror");
        clone(&mirror, remote.to_str().unwrap(), None).unwrap();

        write_char(&live, 1001, "WRECKED");
        let h = history(&mirror, 10).unwrap();
        let report = restore_commit(&mirror, &h[0].oid, &profiles).unwrap();

        assert_eq!(report.restored, 1);
        assert_eq!(read(&live.join("core_char_1001.dat")), "v1");
    }

    #[test]
    fn pull_fast_forwards_a_cloned_mirror() {
        let (tmp, live, remote, profiles) = remote_with_one_commit();
        let mirror = tmp.path().join("mirror");
        clone(&mirror, remote.to_str().unwrap(), None).unwrap();

        // The remote moves on.
        write_char(&live, 1001, "v2");
        snapshot(&remote, &profiles, &names(&[]), "second").unwrap();

        let r = pull(&mirror, None).unwrap();

        assert!(r.updated);
        let h = history(&mirror, 10).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].summary, "second");
        // The working tree advanced too, so restores see the new bytes.
        assert_eq!(
            read(&mirror.join("characters/settings_Default/1001_1001.dat")),
            "v2"
        );
    }

    #[test]
    fn pull_when_up_to_date_reports_no_update() {
        let (tmp, _live, remote, _profiles) = remote_with_one_commit();
        let mirror = tmp.path().join("mirror");
        clone(&mirror, remote.to_str().unwrap(), None).unwrap();

        let r = pull(&mirror, None).unwrap();

        assert!(!r.updated);
        assert!(r.head.is_some());
    }

    #[test]
    fn pull_into_a_set_remote_only_repo_brings_the_history() {
        // The unborn-HEAD path: init + set_remote without cloning, then
        // pull - an alternative fresh-machine flow.
        let (tmp, _live, remote, _profiles) = remote_with_one_commit();
        let mirror = tmp.path().join("mirror");
        set_remote(&mirror, remote.to_str().unwrap()).unwrap();

        let r = pull(&mirror, None).unwrap();

        assert!(r.updated);
        assert_eq!(history(&mirror, 10).unwrap().len(), 1);
    }

    #[test]
    fn pull_refuses_diverged_histories() {
        let (tmp, live, remote, profiles) = remote_with_one_commit();
        let mirror = tmp.path().join("mirror");
        clone(&mirror, remote.to_str().unwrap(), None).unwrap();

        // Both sides snapshot different content on top of the shared
        // first commit.
        write_char(&live, 1001, "local v2");
        snapshot(&mirror, &profiles, &names(&[]), "local second").unwrap();
        write_char(&live, 1001, "remote v2");
        snapshot(&remote, &profiles, &names(&[]), "remote second").unwrap();

        let err = pull(&mirror, None).unwrap_err();

        assert!(err.to_string().contains("diverged"));
        assert_eq!(
            history(&mirror, 10).unwrap()[0].summary,
            "local second",
            "a refused pull must leave local history untouched"
        );
    }

    #[test]
    fn pull_without_a_remote_is_a_config_error() {
        let e = env();
        ensure_repo(&e.mirror).unwrap();

        let err = pull(&e.mirror, None).unwrap_err();

        assert!(err.to_string().contains("origin"));
    }

    // ---------------------- commit_semantic_diff ----------------------

    #[test]
    fn a_commits_diff_names_the_groups_that_changed() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("windows", 1), ("overview", 2)]);
        let profiles = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[(1001, "Alice")]), "first").unwrap();

        write_marshal_char(
            &e.live,
            1001,
            &[("windows", 99), ("overview", 2), ("audio", 5)],
        );
        let oid = snapshot(&e.mirror, &profiles, &names(&[(1001, "Alice")]), "second")
            .unwrap()
            .unwrap();

        let d = commit_semantic_diff(&e.mirror, &oid).unwrap();

        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, "character");
        assert_eq!(d[0].label, "Alice");
        assert_eq!(d[0].status, "changed");
        let kinds: Vec<(&str, &str)> = d[0]
            .groups
            .iter()
            .map(|g| (g.group.as_str(), g.kind.as_str()))
            .collect();
        assert_eq!(kinds, vec![("windows", "changed"), ("audio", "added")]);
    }

    #[test]
    fn the_root_commit_reports_everything_as_added() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("ui", 1)]);
        write_user(&e.live, 9001, "user bytes");
        let profiles = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        let d = commit_semantic_diff(&e.mirror, &oid).unwrap();

        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|f| f.status == "added"));
    }

    #[test]
    fn an_unchanged_commit_diffs_to_nothing_even_across_a_rename() {
        // The filename embeds the cached character name; learning a
        // name between snapshots moves the file. Identity keying must
        // see through that instead of reporting removed + added.
        let e = env();
        write_marshal_char(&e.live, 1001, &[("ui", 1)]);
        write_char(&e.live, 2002, "second char so the tree changes");
        let profiles = vec![char_profile(&e.live, 1001), char_profile(&e.live, 2002)];
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        write_char(&e.live, 2002, "modified so a second commit exists");
        let oid = snapshot(&e.mirror, &profiles, &names(&[(1001, "Alice")]), "second")
            .unwrap()
            .unwrap();

        let d = commit_semantic_diff(&e.mirror, &oid).unwrap();

        assert!(
            !d.iter().any(|f| f.id == 1001),
            "a renamed-but-identical file must not appear: {d:?}"
        );
    }

    #[test]
    fn undecodable_changed_bytes_are_reported_as_opaque() {
        let e = env();
        write_char(&e.live, 1001, "v1");
        let profiles = vec![char_profile(&e.live, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        write_char(&e.live, 1001, "v2");
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "second")
            .unwrap()
            .unwrap();

        let d = commit_semantic_diff(&e.mirror, &oid).unwrap();

        assert_eq!(d.len(), 1);
        assert_eq!(d[0].status, "opaque");
    }

    #[test]
    fn a_change_in_one_profile_is_not_masked_by_another() {
        // Regression: identity used to be keyed (kind, id) only, so
        // the alphabetically-first profile's unchanged blob swallowed
        // a real change in the other profile - the ledger showed a
        // commit that "changed nothing".
        let e = env();
        let alt = settings_dir(e.live.parent().unwrap(), "settings_Alt");
        write_marshal_char(&e.live, 1001, &[("ui", 1)]);
        std::fs::write(alt.join("core_char_1001.dat"), marshal_bytes(&[("ui", 1)])).unwrap();
        let profiles = vec![char_profile(&e.live, 1001), char_profile(&alt, 1001)];
        snapshot(&e.mirror, &profiles, &names(&[]), "first").unwrap();

        // Change ONLY the Default profile (sorts after settings_Alt).
        write_marshal_char(&e.live, 1001, &[("ui", 99)]);
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "second")
            .unwrap()
            .unwrap();

        let d = commit_semantic_diff(&e.mirror, &oid).unwrap();

        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].profile, "settings_Default");
        assert_eq!(d[0].status, "changed");
    }

    #[test]
    fn a_character_named_user_is_still_a_character() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("ui", 1)]);
        let profiles = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[(1001, "user")]), "first")
            .unwrap()
            .unwrap();

        let d = commit_semantic_diff(&e.mirror, &oid).unwrap();

        assert_eq!(d.len(), 1);
        assert_eq!(d[0].kind, "character", "{d:?}");
        assert_eq!(d[0].label, "user");
    }

    // ------------------------ selective restore -----------------------

    #[test]
    fn a_selective_restore_reverts_only_the_chosen_group() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("windows", 1), ("overview", 2)]);
        let profiles = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "good state")
            .unwrap()
            .unwrap();

        // Both groups drift afterwards.
        write_marshal_char(&e.live, 1001, &[("windows", 50), ("overview", 60)]);

        let sel = crate::eve::ops::CopySelection {
            char_groups: vec!["windows".to_string()],
            user_groups: Vec::new(),
        };
        let report = restore_commit_with(&e.mirror, &oid, &profiles, Some(&sel)).unwrap();

        assert_eq!(report.restored, 1);
        let live = std::fs::read(e.live.join("core_char_1001.dat")).unwrap();
        let groups = crate::eve::settings::list_groups(&live).unwrap();
        assert_eq!(groups, vec!["windows", "overview"]);
        let d = crate::eve::settings::diff_settings(
            &marshal_bytes(&[("windows", 1), ("overview", 60)]),
            &live,
        )
        .unwrap();
        assert!(
            d.is_empty(),
            "windows reverts to the snapshot, overview keeps today's value: {d:?}"
        );
    }

    #[test]
    fn a_selective_restore_with_no_groups_of_a_kind_leaves_those_files_alone() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("windows", 1)]);
        write_user(&e.live, 9001, "user v1");
        let profiles = vec![char_profile(&e.live, 1001), user_profile(&e.live, 9001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        write_marshal_char(&e.live, 1001, &[("windows", 9)]);
        write_user(&e.live, 9001, "user v2");

        let sel = crate::eve::ops::CopySelection {
            char_groups: vec!["windows".to_string()],
            user_groups: Vec::new(),
        };
        let report = restore_commit_with(&e.mirror, &oid, &profiles, Some(&sel)).unwrap();

        assert_eq!(report.restored, 1);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert_eq!(
            read(&e.live.join("core_user_9001.dat")),
            "user v2",
            "no user groups selected means user files stay untouched"
        );
    }

    #[test]
    fn a_selective_restore_over_an_undecodable_live_file_skips_with_a_reason() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("windows", 1)]);
        let profiles = vec![char_profile(&e.live, 1001)];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        write_char(&e.live, 1001, "the live file is garbage now");

        let sel = crate::eve::ops::CopySelection {
            char_groups: vec!["windows".to_string()],
            user_groups: Vec::new(),
        };
        let report = restore_commit_with(&e.mirror, &oid, &profiles, Some(&sel)).unwrap();

        assert_eq!(report.restored, 0);
        assert!(report.skipped[0].contains("selective restore failed"));
        assert_eq!(
            read(&e.live.join("core_char_1001.dat")),
            "the live file is garbage now",
            "a failed selective restore must not touch the file"
        );
    }

    // ------------------------- commit_groups --------------------------

    #[test]
    fn commit_groups_unions_decodable_groups_by_kind() {
        let e = env();
        write_marshal_char(&e.live, 1001, &[("windows", 1), ("ui", 2)]);
        write_marshal_char(&e.live, 2002, &[("ui", 3), ("overview", 4)]);
        write_user(&e.live, 9001, "not marshal - contributes nothing");
        let profiles = vec![
            char_profile(&e.live, 1001),
            char_profile(&e.live, 2002),
            user_profile(&e.live, 9001),
        ];
        let oid = snapshot(&e.mirror, &profiles, &names(&[]), "first")
            .unwrap()
            .unwrap();

        let (chars, users) = commit_groups(&e.mirror, &oid).unwrap();

        let mut sorted = chars.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["overview", "ui", "windows"]);
        assert!(users.is_empty());
    }
}
