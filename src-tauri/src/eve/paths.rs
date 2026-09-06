use serde::Serialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
pub struct EveInstall {
    pub label: String,
    pub kind: InstallKind,
    pub root: PathBuf,
    pub settings_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    WindowsNative,
    MacosNative,
    LinuxProton,
    Manual,
}

pub fn discover(override_root: Option<&Path>) -> Vec<EveInstall> {
    let mut out = Vec::new();
    if let Some(root) = override_root {
        if let Some(install) = build_install("Manual override", InstallKind::Manual, root) {
            out.push(install);
        }
    }
    out.extend(discover_windows_native());
    out.extend(discover_macos_native());
    out.extend(discover_linux_proton());

    out.sort_by(|a, b| a.root.cmp(&b.root));
    out.dedup_by(|a, b| a.root == b.root);
    out
}

fn discover_windows_native() -> Vec<EveInstall> {
    if !cfg!(target_os = "windows") {
        return Vec::new();
    }
    let local = match std::env::var("LOCALAPPDATA")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir)
    {
        Some(p) => p,
        None => return Vec::new(),
    };
    let root = local.join("CCP").join("EVE");
    build_install("Windows", InstallKind::WindowsNative, &root)
        .into_iter()
        .collect()
}

fn discover_macos_native() -> Vec<EveInstall> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    macos_settings_roots(&home)
        .iter()
        .filter(|p| p.exists())
        .filter_map(|p| build_install("macOS", InstallKind::MacosNative, p))
        .collect()
}

/// Where the Mac client keeps its settings, current layout first. Since
/// the native launcher it is `~/Library/Application Support/CCP/EVE`
/// (CCP's documented location, and what every peer tool reads); the
/// Wine-wrapped clients before it kept a `p_drive` under the launcher's
/// own folder, and those two layouts stay for anyone still on one.
fn macos_settings_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Application Support/CCP/EVE"),
        home.join("Library/Application Support/EVE Online/p_drive/Local Settings/Application Data/CCP/EVE"),
        home.join("Library/Application Support/EVE Online/p_drive/users/crossover/Local Settings/Application Data/CCP/EVE"),
    ]
}

fn discover_linux_proton() -> Vec<EveInstall> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    let proton_roots = [
        home.join(".steam/steam/steamapps/compatdata/8500"),
        home.join(".local/share/Steam/steamapps/compatdata/8500"),
        home.join(".steam/debian-installation/steamapps/compatdata/8500"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam/steamapps/compatdata/8500"),
    ];

    let mut out = Vec::new();
    for proton in proton_roots.iter().filter(|p| p.exists()) {
        let pfx = proton.join("pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE");
        if let Some(install) = build_install("Steam Proton", InstallKind::LinuxProton, &pfx) {
            out.push(install);
        }
    }
    out
}

fn build_install(label: &str, kind: InstallKind, root: &Path) -> Option<EveInstall> {
    if !root.exists() {
        return None;
    }
    let settings_dirs = find_settings_dirs(root);
    if settings_dirs.is_empty() {
        return None;
    }
    Some(EveInstall {
        label: label.to_string(),
        kind,
        root: root.to_path_buf(),
        settings_dirs,
    })
}

fn find_settings_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.starts_with("settings_") && contains_dat_files(entry.path()) {
            out.push(entry.path().to_path_buf());
        }
    }
    out
}

fn contains_dat_files(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                (n.starts_with("core_char_") || n.starts_with("core_user_")) && n.ends_with(".dat")
            })
        })
        .unwrap_or(false)
}

pub fn looks_like_settings_dir(dir: &Path) -> bool {
    contains_dat_files(dir)
}

/// The known EVE server environments, by the name that terminates the
/// server-scoped cache directory (e.g. `c_ccp_eve_tq_tranquility`,
/// `c_ccp_eve_sisi_singularity`). Order matters only for docs.
const SERVERS: &[&str] = &[
    "tranquility",
    "singularity",
    "thunderdome",
    "serenity",
    "infinity",
    "duality",
];

/// Which server a settings dir belongs to, parsed from its ancestry:
/// EVE nests `settings_*` inside a per-server cache dir whose name ends
/// with `_<server>`. `None` for layouts that carry no server component
/// (manual overrides pointing straight at a settings dir, test
/// fixtures) - callers treat unknown as "do not restrict".
pub fn server_of(settings_dir: &Path) -> Option<&'static str> {
    for ancestor in settings_dir.ancestors().skip(1) {
        let Some(name) = ancestor.file_name() else {
            continue;
        };
        let name = name.to_string_lossy().to_lowercase();
        for server in SERVERS {
            if name == *server || name.ends_with(&format!("_{server}")) {
                return Some(server);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use tempfile::TempDir;

    #[test]
    fn contains_dat_files_accepts_char_and_user_files() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "a");
        write_char(&a, 1001, "c");
        assert!(contains_dat_files(&a));

        let b = settings_dir(tmp.path(), "b");
        write_user(&b, 9001, "u");
        assert!(contains_dat_files(&b));
    }

    #[test]
    fn contains_dat_files_rejects_unrelated_content() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "empty");
        assert!(!contains_dat_files(&dir));

        std::fs::write(dir.join("readme.txt"), "x").unwrap();
        std::fs::write(dir.join("core_char_1.bak"), "x").unwrap();
        std::fs::write(dir.join("other_1.dat"), "x").unwrap();
        assert!(!contains_dat_files(&dir));
    }

    #[test]
    fn contains_dat_files_on_a_missing_dir_is_false() {
        let tmp = TempDir::new().unwrap();
        assert!(!contains_dat_files(&tmp.path().join("nope")));
    }

    #[test]
    fn looks_like_settings_dir_is_the_folder_picker_guard() {
        // Settings > Browse rejects folders that fail this check, so it
        // needs to be true for a real settings dir specifically.
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        assert!(!looks_like_settings_dir(&dir));
        write_char(&dir, 1001, "c");
        assert!(looks_like_settings_dir(&dir));
    }

    #[test]
    fn find_settings_dirs_requires_both_the_prefix_and_dat_files() {
        let tmp = TempDir::new().unwrap();
        let good = settings_dir(tmp.path(), "settings_Default");
        write_char(&good, 1001, "c");
        // Right prefix, no .dat files.
        settings_dir(tmp.path(), "settings_Empty");
        // Has .dat files, wrong prefix.
        let wrong = settings_dir(tmp.path(), "cache");
        write_char(&wrong, 2002, "c");

        assert_eq!(find_settings_dirs(tmp.path()), vec![good]);
    }

    #[test]
    fn find_settings_dirs_descends_into_nested_layouts() {
        // Proton prefixes bury settings under a couple of levels, so
        // the walk has to reach depth 3.
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("c_drive/EVE");
        std::fs::create_dir_all(&nested).unwrap();
        let dir = settings_dir(&nested, "settings_Default");
        write_char(&dir, 1001, "c");

        assert_eq!(find_settings_dirs(tmp.path()), vec![dir]);
    }

    #[test]
    fn find_settings_dirs_stops_below_depth_four() {
        let tmp = TempDir::new().unwrap();
        let too_deep = tmp.path().join("a/b/c/d");
        std::fs::create_dir_all(&too_deep).unwrap();
        let dir = settings_dir(&too_deep, "settings_Default");
        write_char(&dir, 1001, "c");

        assert!(
            find_settings_dirs(tmp.path()).is_empty(),
            "max_depth(3) bounds the walk; deeper hits would mean scanning whole drives"
        );
    }

    #[test]
    fn find_settings_dirs_collects_sibling_profiles() {
        let tmp = TempDir::new().unwrap();
        let a = settings_dir(tmp.path(), "settings_Default");
        let b = settings_dir(tmp.path(), "settings_Alt");
        write_char(&a, 1001, "c");
        write_char(&b, 2002, "c");

        let mut found = find_settings_dirs(tmp.path());
        found.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(found, want);
    }

    #[test]
    fn macos_looks_in_the_current_layout_before_the_wine_one() {
        // A current Mac has no p_drive at all; checking only the Wine
        // layout meant "no EVE installs detected" on every Mac.
        let roots = macos_settings_roots(Path::new("/Users/pilot"));
        assert_eq!(
            roots[0],
            Path::new("/Users/pilot/Library/Application Support/CCP/EVE")
        );
        assert!(
            roots
                .iter()
                .any(|p| p.to_string_lossy().contains("p_drive")),
            "the legacy layout must still be searched"
        );
    }

    #[test]
    fn build_install_returns_none_when_the_root_is_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(build_install("x", InstallKind::Manual, &tmp.path().join("nope")).is_none());
    }

    #[test]
    fn build_install_returns_none_when_no_profiles_are_present() {
        let tmp = TempDir::new().unwrap();
        assert!(
            build_install("x", InstallKind::Manual, tmp.path()).is_none(),
            "an EVE folder with no settings_* dirs is not a usable install"
        );
    }

    #[test]
    fn build_install_captures_label_kind_and_profiles() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");

        let inst = build_install("Steam Proton", InstallKind::LinuxProton, tmp.path()).unwrap();

        assert_eq!(inst.label, "Steam Proton");
        assert_eq!(inst.kind, InstallKind::LinuxProton);
        assert_eq!(inst.root, tmp.path());
        assert_eq!(inst.settings_dirs, vec![dir]);
    }

    #[test]
    fn discover_includes_a_valid_manual_override() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");

        let found = discover(Some(tmp.path()));

        // Asserts containment rather than an exact count: the host
        // running these tests may have a real EVE install too.
        let ours = found
            .iter()
            .find(|i| i.root == tmp.path())
            .expect("override should be discovered");
        assert_eq!(ours.kind, InstallKind::Manual);
        assert_eq!(ours.label, "Manual override");
    }

    #[test]
    fn discover_drops_an_override_that_holds_no_profiles() {
        let tmp = TempDir::new().unwrap();
        assert!(
            !discover(Some(tmp.path()))
                .iter()
                .any(|i| i.root == tmp.path()),
            "an override pointing at an empty folder must not appear as an install"
        );
    }

    // ----------------------------- server_of --------------------------

    #[test]
    fn server_of_reads_the_server_from_the_cache_dir_name() {
        // The real on-disk shape, verified on a live install.
        let p = Path::new(
            "/home/x/.steam/steam/steamapps/compatdata/8500/pfx/drive_c/users/steamuser/AppData/Local/CCP/EVE/c_ccp_eve_tq_tranquility/settings_Default",
        );
        assert_eq!(server_of(p), Some("tranquility"));

        let p = Path::new("/x/CCP/EVE/c_ccp_eve_sisi_singularity/settings_Default");
        assert_eq!(server_of(p), Some("singularity"));

        let p = Path::new("/x/CCP/EVE/d_eve_sharedcache_serenity/settings_Alt");
        assert_eq!(server_of(p), Some("serenity"));

        let p = Path::new("/x/CCP/EVE/c_thunderdome/settings_Default");
        assert_eq!(server_of(p), Some("thunderdome"));
    }

    #[test]
    fn server_of_is_none_without_a_server_component() {
        assert_eq!(server_of(Path::new("/tmp/xyz/settings_Default")), None);
        assert_eq!(server_of(Path::new("/")), None);
    }

    #[test]
    fn server_of_ignores_the_settings_dir_itself() {
        // A profile someone named after a server must not fool the
        // parser: only ancestors count.
        let p = Path::new("/x/CCP/EVE/c_ccp_eve_tq_tranquility/settings_singularity");
        assert_eq!(server_of(p), Some("tranquility"));
    }

    #[test]
    fn discover_dedups_installs_sharing_a_root() {
        let tmp = TempDir::new().unwrap();
        let dir = settings_dir(tmp.path(), "settings_Default");
        write_char(&dir, 1001, "c");

        let found = discover(Some(tmp.path()));

        assert_eq!(
            found.iter().filter(|i| i.root == tmp.path()).count(),
            1,
            "the same root must not be listed twice"
        );
    }
}
