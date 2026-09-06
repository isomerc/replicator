use crate::error::AppResult;
use crate::eve::paths::EveInstall;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileFile {
    Character {
        id: i64,
        path: PathBuf,
        install_root: PathBuf,
        settings_dir: PathBuf,
        modified: Option<i64>,
        size: u64,
    },
    User {
        id: i64,
        path: PathBuf,
        install_root: PathBuf,
        settings_dir: PathBuf,
        modified: Option<i64>,
        size: u64,
    },
}

impl ProfileFile {
    pub fn path(&self) -> &Path {
        match self {
            ProfileFile::Character { path, .. } | ProfileFile::User { path, .. } => path,
        }
    }
    pub fn settings_dir(&self) -> &Path {
        match self {
            ProfileFile::Character { settings_dir, .. }
            | ProfileFile::User { settings_dir, .. } => settings_dir,
        }
    }
}

pub fn enumerate(installs: &[EveInstall]) -> AppResult<Vec<ProfileFile>> {
    let mut out = Vec::new();
    for install in installs {
        for sd in &install.settings_dirs {
            for entry in std::fs::read_dir(sd)?.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy().to_string();
                if !name.ends_with(".dat") {
                    continue;
                }
                let meta = entry.metadata()?;
                if !meta.is_file() {
                    continue;
                }
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                let size = meta.len();

                if let Some(id) = parse_id(&name, "core_char_") {
                    out.push(ProfileFile::Character {
                        id,
                        path,
                        install_root: install.root.clone(),
                        settings_dir: sd.clone(),
                        modified,
                        size,
                    });
                } else if let Some(id) = parse_id(&name, "core_user_") {
                    out.push(ProfileFile::User {
                        id,
                        path,
                        install_root: install.root.clone(),
                        settings_dir: sd.clone(),
                        modified,
                        size,
                    });
                }
            }
        }
    }
    Ok(out)
}

fn parse_id(name: &str, prefix: &str) -> Option<i64> {
    let rest = name.strip_prefix(prefix)?.strip_suffix(".dat")?;
    rest.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eve::paths::InstallKind;
    use crate::testutil::*;
    use tempfile::TempDir;

    fn id_of(p: &ProfileFile) -> i64 {
        match p {
            ProfileFile::Character { id, .. } | ProfileFile::User { id, .. } => *id,
        }
    }

    fn install(root: &Path, dirs: Vec<PathBuf>) -> EveInstall {
        EveInstall {
            label: "test".into(),
            kind: InstallKind::Manual,
            root: root.to_path_buf(),
            settings_dirs: dirs,
        }
    }

    #[test]
    fn parse_id_reads_the_numeric_suffix() {
        assert_eq!(
            parse_id("core_char_2035047876.dat", "core_char_"),
            Some(2035047876)
        );
        assert_eq!(
            parse_id("core_user_4342021.dat", "core_user_"),
            Some(4342021)
        );
    }

    #[test]
    fn parse_id_rejects_a_mismatched_prefix() {
        assert_eq!(parse_id("core_user_123.dat", "core_char_"), None);
    }

    #[test]
    fn parse_id_rejects_a_mismatched_extension() {
        assert_eq!(parse_id("core_char_123.txt", "core_char_"), None);
    }

    #[test]
    fn parse_id_rejects_the_user_placeholder() {
        // `core_user__.dat` leaves "_" after the prefix, which is not a
        // number - this is what keeps the placeholder out of the
        // profile list entirely.
        assert_eq!(parse_id("core_user__.dat", "core_user_"), None);
    }

    #[test]
    fn enumerate_classifies_char_and_user_files() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");
        write_user(&dir, 9001, "u");

        let profiles = enumerate(&[install(tmp.path(), vec![dir.clone()])]).unwrap();

        assert_eq!(profiles.len(), 2);
        assert!(profiles
            .iter()
            .any(|p| matches!(p, ProfileFile::Character { id: 1001, .. })));
        assert!(profiles
            .iter()
            .any(|p| matches!(p, ProfileFile::User { id: 9001, .. })));
    }

    #[test]
    fn enumerate_skips_the_user_placeholder() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");
        write_user_placeholder(&dir);

        let profiles = enumerate(&[install(tmp.path(), vec![dir.clone()])]).unwrap();

        assert_eq!(profiles.len(), 1);
        assert!(matches!(
            profiles[0],
            ProfileFile::Character { id: 1001, .. }
        ));
    }

    #[test]
    fn enumerate_ignores_non_dat_files_and_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");
        std::fs::write(dir.join("prefs.ini"), "x").unwrap();
        std::fs::write(dir.join("core_char_9.bak"), "x").unwrap();
        std::fs::create_dir(dir.join("core_char_777.dat")).unwrap();

        let profiles = enumerate(&[install(tmp.path(), vec![dir.clone()])]).unwrap();

        assert_eq!(profiles.len(), 1, "got {profiles:?}");
        assert_eq!(id_of(&profiles[0]), 1001);
    }

    #[test]
    fn enumerate_records_size_and_settings_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "twelve bytes");

        let profiles = enumerate(&[install(tmp.path(), vec![dir.clone()])]).unwrap();

        match &profiles[0] {
            ProfileFile::Character {
                size,
                settings_dir,
                modified,
                ..
            } => {
                assert_eq!(*size, 12);
                assert_eq!(settings_dir, &dir);
                assert!(modified.is_some(), "mtime drives the UI's staleness column");
            }
            other => panic!("expected a character, got {other:?}"),
        }
    }

    #[test]
    fn enumerate_spans_multiple_settings_dirs_and_installs() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");
        let b = settings_dir(tmp.path(), "settings_Alt");
        let other_root = TempDir::new().unwrap();
        let c = settings_dir(other_root.path(), "settings_Default");
        write_char(&a, 1001, "c");
        write_char(&b, 2002, "c");
        write_char(&c, 3003, "c");

        let profiles = enumerate(&[
            install(tmp.path(), vec![a, b]),
            install(other_root.path(), vec![c]),
        ])
        .unwrap();

        let mut ids: Vec<i64> = profiles.iter().map(id_of).collect();
        ids.sort();
        assert_eq!(ids, vec![1001, 2002, 3003]);
    }

    #[test]
    fn enumerate_surfaces_an_unreadable_settings_dir_as_an_error() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("settings_Gone");

        assert!(
            enumerate(&[install(tmp.path(), vec![missing])]).is_err(),
            "a vanished settings dir should surface, not silently yield zero characters"
        );
    }

    #[test]
    fn enumerate_of_no_installs_is_empty() {
        assert!(enumerate(&[]).unwrap().is_empty());
    }

    #[test]
    fn profile_file_accessors_agree_with_the_variant() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        let c = char_profile(&dir, 1001);
        let u = user_profile(&dir, 9001);

        assert_eq!(c.path(), dir.join("core_char_1001.dat"));
        assert_eq!(c.settings_dir(), dir);
        assert_eq!(u.path(), dir.join("core_user_9001.dat"));
        assert_eq!(u.settings_dir(), dir);
    }
}
