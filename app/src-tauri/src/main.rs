//! gdrive-app: the tray icon and status/settings window for gdrive-linux.
//!
//! The app is only a client of the `gdrived` daemon (over the IPC socket). Quitting it
//! never stops syncing.

mod autostart;
mod commands;
mod daemon;
mod icons;
mod tray;

use std::sync::Arc;

use gdrive_core::status::SyncState;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tokio::sync::Notify;

pub const MAIN_WINDOW: &str = "main";

/// Shared app state: lets anything (tray menu, commands) ask the tray poller to
/// refresh right away instead of waiting for the next tick.
pub struct AppState {
    pub refresh: Arc<Notify>,
}

/// Shows, un-minimises and focuses the main window, and tells the page to refresh.
pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = w.emit("app://shown", ());
    }
}

/// Multi-call entry point for single-file distributions (AppImage): `--daemon` runs
/// the bundled `gdrived`, `--cli ARGS…` runs the bundled `gdrive`. Inside an AppImage
/// each invocation gets its own mount, so a daemon started this way keeps running
/// after the app (and its mount) goes away.
fn exec_bundled_tool() {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os().skip(1);
    let tool = match args.next() {
        Some(a) if a == "--daemon" => "gdrived",
        Some(a) if a == "--cli" => "gdrive",
        _ => return,
    };
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::PathBuf::from));
    let exe = dir.map(|d| d.join(tool)).filter(|p| p.is_file()).unwrap_or_else(|| tool.into());
    let err = std::process::Command::new(&exe).args(args).exec();
    eprintln!("failed to run {}: {err}", exe.display());
    std::process::exit(127);
}

fn main() {
    exec_bundled_tool();

    // WebKitGTK's DMA-BUF renderer kills the process with "Error 71 (Protocol error)
    // dispatching to Wayland display" on NVIDIA (and some other) drivers. This window is
    // tiny, so the fallback renderer costs nothing noticeable. Must happen before GTK
    // starts and before any other thread exists.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gdrive_app=info".into()),
        )
        .init();

    let show_requested = std::env::args().any(|a| a == "--show");
    let refresh = Arc::new(Notify::new());

    let app = tauri::Builder::default()
        // Must be registered first: a second launch just surfaces the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { refresh: refresh.clone() })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::pause,
            commands::resume,
            commands::sync_now,
            commands::full_resync,
            commands::start_login,
            commands::sign_out,
            commands::get_config,
            commands::set_config,
            commands::open_path,
            commands::open_url,
            commands::get_autostart,
            commands::set_autostart,
        ])
        .on_window_event(|window, event| {
            // Closing the window only hides it; the tray keeps the app alive.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == MAIN_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
                if let Some(icon) = icons::window_icon() {
                    let _ = w.set_icon(icon);
                }
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(startup(handle, show_requested, refresh));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application");

    app.run(|_app, event| {
        // Hiding the last window must not end the app; only an explicit exit (Quit) does.
        if let RunEvent::ExitRequested { api, code, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}

async fn startup(app: tauri::AppHandle, show_requested: bool, refresh: Arc<Notify>) {
    // Make sure the daemon is up before deciding anything based on its state.
    let status = daemon::status_or_spawn().await;
    let needs_setup = match &status {
        Ok(s) => matches!(s.state, SyncState::SetupRequired | SyncState::SignedOut),
        Err(_) => daemon::local_setup_needed(),
    };

    let tray_ok = tray::start(app.clone(), refresh).await;
    if show_requested || needs_setup || !tray_ok {
        // Without a tray there would be no way to reach the app, so show the window.
        show_main_window(&app);
    }
}
