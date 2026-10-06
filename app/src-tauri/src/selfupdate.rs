//! Restarting the app after the daemon has installed a newer AppImage.
//!
//! The daemon replaces the AppImage file and restarts itself; this app keeps running
//! its old, already-mounted copy until it relaunches. It does so on its own only
//! while the window is hidden, so it never disappears from under the user.

use gdrive_core::status::Status;
use gdrive_core::update::{self, Version};
use tauri::Manager;

use crate::MAIN_WINDOW;

const REPLACE_PID_ENV: &str = "GDRIVE_REPLACE_PID";

/// True when the daemon runs a newer version than this app and we can relaunch into it.
pub fn outdated(status: &Status) -> bool {
    update::running_appimage().is_some()
        && Version::parse(&status.version).is_some_and(|daemon| daemon > Version::current())
}

/// Relaunches into the updated AppImage if needed and the window is not on screen.
pub fn relaunch_if_outdated(app: &tauri::AppHandle, status: &Status) {
    if !outdated(status) {
        return;
    }
    let visible = app.get_webview_window(MAIN_WINDOW).and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if !visible {
        tracing::info!("daemon is {}, app is {}; relaunching", status.version, update::CURRENT_VERSION);
        relaunch(app);
    }
}

/// Starts the (updated) AppImage and exits this instance.
pub fn relaunch(app: &tauri::AppHandle) {
    let Some(appimage) = update::running_appimage() else { return };
    let mut cmd = std::process::Command::new(&appimage);
    update::clean_appimage_launch(&mut cmd);
    let spawned = cmd
        .env(REPLACE_PID_ENV, std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => app.exit(0),
        Err(e) => tracing::warn!("could not relaunch {}: {e}", appimage.display()),
    }
}

/// In a relaunched instance, waits for the instance it replaces to exit, so the
/// single-instance check doesn't hand us straight back to the old copy.
pub fn wait_for_replaced_instance() {
    let Some(pid) = std::env::var(REPLACE_PID_ENV).ok().and_then(|p| p.parse::<u32>().ok()) else { return };
    std::env::remove_var(REPLACE_PID_ENV);
    let proc_dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while proc_dir.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
