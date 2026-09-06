//! Fixtures for building fake EVE settings trees on disk.
//!
//! Every test that touches the filesystem builds a real directory in a
//! `tempfile::TempDir` rather than mocking `std::fs`. The code under
//! test does genuine `read_dir`/`read`/`write` calls, so mocking would
//! test the mock; this way the tests exercise the same syscalls the
//! app makes against Wine/Proton paths.

#![cfg(test)]

use crate::eve::characters::ProfileFile;
use std::fs::{File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Create `root/<name>/` and return it. Mirrors EVE's layout, where an
/// install root holds one directory per profile, each named
/// `settings_<Profile>`.
pub fn settings_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `core_char_<id>.dat` with the given contents.
pub fn write_char(dir: &Path, id: i64, contents: &str) -> PathBuf {
    let p = dir.join(format!("core_char_{id}.dat"));
    std::fs::write(&p, contents).unwrap();
    p
}

/// Write `core_user_<id>.dat` with the given contents.
pub fn write_user(dir: &Path, id: i64, contents: &str) -> PathBuf {
    let p = dir.join(format!("core_user_{id}.dat"));
    std::fs::write(&p, contents).unwrap();
    p
}

/// Write the `core_user__.dat` placeholder EVE leaves behind before a
/// character has been selected. Nothing should ever write to it.
pub fn write_user_placeholder(dir: &Path) -> PathBuf {
    let p = dir.join("core_user__.dat");
    std::fs::write(&p, "placeholder").unwrap();
    p
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Build a `ProfileFile::Character` pointing at an existing file.
pub fn char_profile(dir: &Path, id: i64) -> ProfileFile {
    ProfileFile::Character {
        id,
        path: dir.join(format!("core_char_{id}.dat")),
        install_root: dir.parent().unwrap_or(dir).to_path_buf(),
        settings_dir: dir.to_path_buf(),
        modified: Some(0),
        size: 0,
    }
}

/// Build a `ProfileFile::User` pointing at an existing file.
pub fn user_profile(dir: &Path, id: i64) -> ProfileFile {
    ProfileFile::User {
        id,
        path: dir.join(format!("core_user_{id}.dat")),
        install_root: dir.parent().unwrap_or(dir).to_path_buf(),
        settings_dir: dir.to_path_buf(),
        modified: Some(0),
        size: 0,
    }
}

/// Force a file's mtime to a fixed unix timestamp. The launcher-log
/// scanner gates on mtime, so tests need deterministic control of it
/// rather than relying on wall-clock write order.
pub fn set_mtime(path: &Path, unix_secs: u64) {
    let f = File::options().write(true).open(path).unwrap();
    let times = FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs));
    f.set_times(times).unwrap();
}
