mod commands;
mod db;
mod error;
mod eve;
mod git_mirror;
mod keychain;
mod portable;
mod state;
#[cfg(test)]
mod testutil;
mod update;
mod window_geometry;

use std::sync::{Arc, Mutex};
use tauri::Manager;
use tracing_subscriber::EnvFilter;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // In-place updates, signed with the project's own minisign
        // key (not OS code signing). The frontend only calls this
        // when the user accepts the footline badge's offer.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Resize the window once and it opens that way forever; the
        // conf.json size is only the first-launch default. Hand-rolled
        // (tauri-plugin-window-state restores in physical pixels
        // before Wayland reports the real scale factor, so the window
        // grew by the display scale on every launch). Saving on focus
        // loss instead of graceful exit means a killed dev process
        // still remembers - alt-tabbing away is enough.
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(true) => note_size_baseline(window),
            tauri::WindowEvent::Focused(false) | tauri::WindowEvent::CloseRequested { .. } => {
                save_window_geometry(window)
            }
            _ => {}
        })
        .setup(|app| {
            // Arc so heavy async commands can move a handle into
            // spawn_blocking closures.
            let app_state = state::AppState::initialize(app.handle())?;
            let saved_geometry = app_state.db.get_setting(WINDOW_GEOMETRY_KEY).ok().flatten();
            app.manage(Arc::new(Mutex::new(app_state)));

            // Embed the app icon in the binary and apply it to the
            // main window programmatically. Covers Windows (title
            // bar + taskbar), macOS (dock), and Linux X11 (WM_HINTS).
            if let Some(window) = app.get_webview_window("main") {
                let icon_bytes = include_bytes!("../icons/icon.png");
                if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
                    let _ = window.set_icon(icon);
                }
                restore_window_geometry(&window, saved_geometry);
            }

            // Wayland has no per-window icon API - the compositor
            // looks up icons via a .desktop file whose
            // StartupWMClass matches the window's app_id. Write our
            // own .desktop entry + icon to standard XDG locations on
            // every startup (idempotent). After this, KWin/Sway/etc.
            // pick up the icon - usually visible on the next launch
            // once the compositor refreshes its desktop entry cache.
            #[cfg(target_os = "linux")]
            ensure_linux_desktop_entry();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::detect_installs,
            commands::list_characters,
            commands::set_install_override,
            commands::resolve_names,
            commands::copy_character,
            commands::copy_to_group,
            commands::save_template,
            commands::list_templates,
            commands::apply_template,
            commands::delete_template,
            commands::list_groups,
            commands::create_group,
            commands::delete_group,
            commands::set_group_members,
            commands::is_eve_running,
            commands::character_portraits,
            commands::copy_groups,
            commands::template_groups,
            commands::git_status,
            commands::git_set_remote,
            commands::git_snapshot,
            commands::git_history,
            commands::git_restore,
            commands::git_push,
            commands::git_clone,
            commands::git_pull,
            commands::pat_set,
            commands::pat_clear,
            commands::pat_present,
            commands::export_setup,
            commands::preview_import,
            commands::import_setup,
            commands::export_template,
            commands::import_template,
            commands::list_accounts,
            commands::set_account_meta,
            commands::set_char_account,
            commands::git_diff,
            commands::commit_groups,
            commands::diff_characters,
            commands::character_layout,
            commands::template_layout,
            commands::design_save_template,
            commands::design_apply,
            commands::layout_floors,
            commands::play_jazz_chord,
            commands::check_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

const WINDOW_GEOMETRY_KEY: &str = "window_geometry";

/// The toolkit's reading of the size we last applied or saved, logical
/// units. The reference that makes save/restore a fixed point: GTK
/// readings include decoration extents that set_size does not, so raw
/// readings grow the window every launch. See window_geometry::step.
static SIZE_BASELINE: Mutex<Option<(f64, f64)>> = Mutex::new(None);

/// Logical inner size, if the window can report one right now.
fn logical_inner(window: &tauri::Window) -> Option<(f64, f64)> {
    let inner = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?;
    let l = inner.to_logical::<f64>(scale);
    Some((l.width, l.height))
}

/// First unmaximized focus-in after launch: whatever the toolkit says
/// the window measures NOW corresponds to the size restore applied
/// (the user cannot have resized an unfocused window). That reading
/// anchors every later delta.
fn note_size_baseline(window: &tauri::Window) {
    let Ok(mut slot) = SIZE_BASELINE.lock() else {
        return;
    };
    if slot.is_some() || window.is_maximized().unwrap_or(false) {
        return;
    }
    *slot = logical_inner(window);
}

/// Runs on focus loss, so the scale factor and any pending resizes
/// have settled by the time anything is recorded.
fn save_window_geometry(window: &tauri::Window) {
    let Some(app_state) = window
        .app_handle()
        .try_state::<Arc<Mutex<state::AppState>>>()
    else {
        return;
    };
    // try_lock, never lock: this runs on the main thread, and a push or
    // a first-run log parse can hold the state for minutes. Skipping one
    // save costs nothing; the next focus loss catches it.
    let Ok(guard) = app_state.try_lock() else {
        return;
    };
    let Some(current) = logical_inner(window) else {
        return;
    };
    let pos = window.outer_position().ok().and_then(|p| {
        let scale = window.scale_factor().ok()?;
        Some(p.to_logical::<f64>(scale))
    });

    let prev: Option<window_geometry::WindowGeometry> = guard
        .db
        .get_setting(WINDOW_GEOMETRY_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(window_geometry::WindowGeometry::sane);

    // A maximized window reports the work-area size; keep the last
    // free-floating size underneath so unmaximizing after a restart
    // lands somewhere reasonable.
    let geom = if window.is_maximized().unwrap_or(false) {
        match prev {
            Some(mut p) => {
                p.maximized = true;
                p
            }
            None => return,
        }
    } else {
        let Ok(mut baseline) = SIZE_BASELINE.lock() else {
            return;
        };
        let (size, new_baseline) =
            window_geometry::step(prev.map(|p| (p.width, p.height)), *baseline, current);
        *baseline = Some(new_baseline);
        window_geometry::WindowGeometry {
            width: size.0,
            height: size.1,
            x: pos.as_ref().map(|p| p.x),
            y: pos.as_ref().map(|p| p.y),
            maximized: false,
        }
    };
    if !geom.sane() {
        return;
    }
    if let Ok(s) = serde_json::to_string(&geom) {
        let _ = guard.db.set_setting(WINDOW_GEOMETRY_KEY, &s);
    }
}

fn restore_window_geometry(window: &tauri::WebviewWindow, saved: Option<String>) {
    let Some(geom) =
        saved.and_then(|s| serde_json::from_str::<window_geometry::WindowGeometry>(&s).ok())
    else {
        return;
    };
    if !geom.sane() {
        return;
    }
    let _ = window.set_size(tauri::LogicalSize::new(geom.width, geom.height));
    // No-op on Wayland (clients cannot place themselves); real on X11,
    // Windows and macOS.
    if let (Some(x), Some(y)) = (geom.x, geom.y) {
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
    if geom.maximized {
        let _ = window.maximize();
    }
}

#[cfg(target_os = "linux")]
fn ensure_linux_desktop_entry() {
    let Some(home) = dirs::home_dir() else { return };

    // 1) Install icons at every hicolor size we have. KDE/GNOME
    //    panels pick a size close to their target render size; if
    //    only one exists they often fall back to a generic icon
    //    rather than scale ours. Auto-update each if the bundled
    //    bytes have changed since the last run.
    #[rustfmt::skip]
    let icon_variants: &[(&str, &[u8])] = &[
        ("32x32",   include_bytes!("../icons/32x32.png")),
        ("64x64",   include_bytes!("../icons/64x64.png")),
        ("128x128", include_bytes!("../icons/128x128.png")),
        ("256x256", include_bytes!("../icons/128x128@2x.png")),
    ];
    for (size, bytes) in icon_variants {
        let dir = home.join(format!(".local/share/icons/hicolor/{size}/apps"));
        let path = dir.join("replicator.png");
        if std::fs::create_dir_all(&dir).is_ok() {
            let needs_write = std::fs::read(&path)
                .map(|existing| existing.as_slice() != *bytes)
                .unwrap_or(true);
            if needs_write {
                let _ = std::fs::write(&path, bytes);
            }
        }
    }

    // 2) Write the .desktop entry. The filename matches the bundle
    //    identifier, which is also what Tauri sets as the Wayland
    //    app_id, which is what compositors match against
    //    StartupWMClass.
    let Some(exe) = launch_target(
        std::env::var_os("APPIMAGE").map(std::path::PathBuf::from),
        std::env::current_exe().ok(),
    ) else {
        return;
    };
    let apps_dir = home.join(".local/share/applications");
    let desktop_path = apps_dir.join("rip.replicator.desktop");
    let content = desktop_entry(&exe, is_packaged_install(&exe));
    if std::fs::create_dir_all(&apps_dir).is_err() {
        return;
    }
    // Idempotent: only rewrite if the binary path has moved or the
    // content otherwise drifted.
    if std::fs::read_to_string(&desktop_path).ok().as_deref() != Some(content.as_str()) {
        let _ = std::fs::write(&desktop_path, &content);
    }
}

/// What the menu entry launches. Inside an AppImage, current_exe points
/// into the transient /tmp mount that vanishes on exit, so an entry
/// built from it is dead by the time anyone clicks it; the runtime
/// exports the image's own path as APPIMAGE, and that is the target.
#[cfg(target_os = "linux")]
fn launch_target(
    appimage: Option<std::path::PathBuf>,
    exe: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    appimage.or(exe)
}

/// A deb, rpm or distro package already ships a menu entry under /usr
/// (or /opt, or the nix store). Ours then exists only so the compositor
/// can match the Wayland app_id to an icon, and must stay out of the
/// menu or "Replicator" shows up twice.
#[cfg(target_os = "linux")]
fn is_packaged_install(exe: &std::path::Path) -> bool {
    exe.starts_with("/usr") || exe.starts_with("/opt") || exe.starts_with("/nix/store")
}

#[cfg(target_os = "linux")]
fn desktop_entry(exe: &std::path::Path, hidden_from_menu: bool) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Replicator\n\
         GenericName=EVE Online UI Manager\n\
         Comment=Copy and version EVE Online UI settings across characters\n\
         Exec={}\n\
         Icon=replicator\n\
         StartupWMClass=rip.replicator\n\
         Categories=Utility;\n\
         Terminal=false\n\
         {}",
        desktop_exec_quote(exe),
        if hidden_from_menu {
            "NoDisplay=true\n"
        } else {
            ""
        }
    )
}

/// Exec values are shell-like: a path with a space needs double quotes,
/// and inside them the spec reserves the double quote, backtick, dollar
/// and backslash, each escaped with a backslash.
#[cfg(target_os = "linux")]
fn desktop_exec_quote(path: &std::path::Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if matches!(c, '"' | '$' | '\\' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(all(test, target_os = "linux"))]
mod desktop_entry_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn an_appimage_launches_itself_not_its_mount() {
        let mount = PathBuf::from("/tmp/.mount_ReplicXYZ/usr/bin/replicator");
        let image = PathBuf::from("/home/pilot/Apps/Replicator.AppImage");
        assert_eq!(
            launch_target(Some(image.clone()), Some(mount.clone())),
            Some(image)
        );
        assert_eq!(launch_target(None, Some(mount.clone())), Some(mount));
    }

    #[test]
    fn exec_is_quoted_and_escaped() {
        assert_eq!(
            desktop_exec_quote(Path::new("/home/pilot/My Apps/Replicator.AppImage")),
            "\"/home/pilot/My Apps/Replicator.AppImage\""
        );
        assert_eq!(
            desktop_exec_quote(Path::new("/x/a\"b$c")),
            "\"/x/a\\\"b\\$c\""
        );
    }

    #[test]
    fn packaged_installs_keep_their_entry_out_of_the_menu() {
        assert!(is_packaged_install(Path::new("/usr/bin/replicator")));
        assert!(!is_packaged_install(Path::new(
            "/home/pilot/Apps/Replicator.AppImage"
        )));
        let packaged = desktop_entry(Path::new("/usr/bin/replicator"), true);
        assert!(packaged.contains("NoDisplay=true"));
        assert!(packaged.contains("StartupWMClass=rip.replicator"));
        let portable = desktop_entry(Path::new("/home/p/Replicator.AppImage"), false);
        assert!(!portable.contains("NoDisplay"));
        assert!(portable.contains("Exec=\"/home/p/Replicator.AppImage\""));
    }
}
