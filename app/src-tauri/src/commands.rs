//! Tauri commands invoked from the frontend (wrapped in `app/src/api/commands.ts`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gdrive_core::config::Config;
use gdrive_core::ipc::{self, Request, Response};
use gdrive_core::status::Status;
use tauri::State;

use crate::{autostart, daemon, AppState};

type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Sends a request that just expects `Ok`, then nudges the tray to refresh.
async fn simple(state: &State<'_, AppState>, req: Request) -> CmdResult<()> {
    let res = daemon::call(req).await.map_err(err);
    state.refresh.notify_one();
    match res? {
        Response::Ok => Ok(()),
        other => Err(format!("unexpected reply from gdrived: {other:?}")),
    }
}

#[tauri::command]
pub async fn get_status() -> CmdResult<Status> {
    daemon::status_or_spawn().await.map_err(err)
}

#[tauri::command]
pub async fn pause(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::Pause).await
}

#[tauri::command]
pub async fn resume(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::Resume).await
}

#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::SyncNow).await
}

#[tauri::command]
pub async fn full_resync(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::FullResync).await
}

#[tauri::command]
pub async fn redownload(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::Redownload).await
}

#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::SignOut).await
}

/// Starts the OAuth flow. The daemon opens the browser itself; the URL is returned
/// so the UI can show it in case that did not work.
#[tauri::command]
pub async fn start_login(state: State<'_, AppState>) -> CmdResult<String> {
    let res = daemon::call(Request::StartLogin).await.map_err(err);
    state.refresh.notify_one();
    match res? {
        Response::LoginUrl { url } => Ok(url),
        other => Err(format!("unexpected reply from gdrived: {other:?}")),
    }
}

/// Reads the configuration from the daemon, or straight from disk if it is not running.
#[tauri::command]
pub async fn get_config() -> CmdResult<Config> {
    match ipc::request(&Request::GetConfig).await {
        Ok(Response::Config { config }) => Ok(config),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected reply from gdrived: {other:?}")),
        Err(_) => Config::load().map_err(err),
    }
}

/// Saves the configuration through the daemon (which validates and applies it). If the
/// daemon is not running, writes the config file directly; it is read on next start.
#[tauri::command]
pub async fn set_config(state: State<'_, AppState>, config: Config) -> CmdResult<()> {
    let res = ipc::request(&Request::SetConfig { config: config.clone() }).await;
    state.refresh.notify_one();
    match res {
        Ok(Response::Ok) | Ok(Response::Config { .. }) => Ok(()),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected reply from gdrived: {other:?}")),
        Err(_) => config.save().map_err(err),
    }
}

/// Opens a file or folder with the default handler. Falls back to the nearest existing
/// parent, so a just-deleted file still opens its folder.
#[tauri::command]
pub async fn open_path(path: String) -> CmdResult<()> {
    let mut p = PathBuf::from(&path);
    if !p.is_absolute() {
        return Err(format!("not an absolute path: {path}"));
    }
    while !p.exists() {
        match p.parent() {
            Some(parent) => p = parent.to_path_buf(),
            None => return Err(format!("{path} does not exist")),
        }
    }
    spawn_xdg_open_checked(&p.to_string_lossy())
}

#[tauri::command]
pub async fn open_url(url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) URLs can be opened".into());
    }
    spawn_xdg_open_checked(&url)
}

#[tauri::command]
pub fn app_version() -> String {
    gdrive_core::update::CURRENT_VERSION.to_owned()
}

#[tauri::command]
pub async fn check_for_updates(state: State<'_, AppState>) -> CmdResult<()> {
    simple(&state, Request::CheckForUpdates).await
}

/// Relaunches into the updated AppImage (after the daemon installed a new version).
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    crate::selfupdate::relaunch(&app);
}

#[tauri::command]
pub async fn get_autostart() -> CmdResult<bool> {
    Ok(autostart::is_enabled())
}

#[tauri::command]
pub async fn set_autostart(enabled: bool) -> CmdResult<()> {
    autostart::set_enabled(enabled).map_err(err)
}

fn spawn_xdg_open_checked(target: &str) -> CmdResult<()> {
    let mut cmd = Command::new("xdg-open");
    // Inside an AppImage the inherited library/data paths point at our bundled copies,
    // which breaks the file manager xdg-open launches.
    gdrive_core::update::clean_appimage_launch(&mut cmd);
    let child = cmd
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run xdg-open: {e}"))?;
    let target = target.to_owned();
    std::thread::spawn(move || match child.wait_with_output() {
        Ok(out) if !out.status.success() => tracing::warn!(
            "xdg-open {target} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => tracing::warn!("xdg-open {target}: {e}"),
        _ => {}
    });
    Ok(())
}

/// Fire-and-forget variant for the tray menu.
pub fn spawn_xdg_open(target: impl AsRef<Path>) {
    if let Err(e) = spawn_xdg_open_checked(&target.as_ref().to_string_lossy()) {
        tracing::warn!("{e}");
    }
}
