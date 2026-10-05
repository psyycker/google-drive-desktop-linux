//! Tray icon via the StatusNotifierItem D-Bus protocol (`ksni`), which GNOME's
//! AppIndicator extension and KDE display natively — no libappindicator needed.

use std::sync::Arc;
use std::time::Duration;

use gdrive_core::ipc::Request;
use gdrive_core::status::{Status, SyncState};
use ksni::menu::{MenuItem, StandardItem};
use ksni::TrayMethods;
use tokio::sync::Notify;

use crate::icons::{self, TrayKind};
use crate::{commands, daemon};

const POLL_INTERVAL: Duration = Duration::from_millis(1500);
const DRIVE_WEB_URL: &str = "https://drive.google.com";

/// Everything the tray displays, derived from one status poll. Compared between polls
/// so the D-Bus properties are only touched when something visible changed.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayView {
    pub kind: TrayKind,
    pub text: String,
    /// Whether the daemon has a signed-in account (enables pause/sync actions).
    pub active: bool,
    pub paused: bool,
    pub setup_needed: bool,
    pub sync_root: Option<String>,
}

impl TrayView {
    fn unreachable() -> Self {
        Self {
            kind: TrayKind::Error,
            text: "Sync service not running".into(),
            active: false,
            paused: false,
            setup_needed: false,
            // The folder can still be opened from the config file on disk.
            sync_root: gdrive_core::config::Config::load()
                .ok()
                .map(|c| c.sync_root.to_string_lossy().into_owned()),
        }
    }

    fn from_status(s: &Status) -> Self {
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let (kind, text) = match s.state {
            SyncState::SetupRequired => (TrayKind::SignedOut, "Setup required".to_string()),
            SyncState::SignedOut => (TrayKind::SignedOut, "Signed out".to_string()),
            SyncState::SigningIn => (TrayKind::SignedOut, "Waiting for sign-in in the browser…".to_string()),
            SyncState::Starting => (TrayKind::Syncing, "Starting…".to_string()),
            SyncState::Idle if !s.errors.is_empty() => (
                TrayKind::Idle,
                format!("Up to date · {} couldn't sync", plural(s.errors.len(), "item", "items")),
            ),
            SyncState::Idle => (TrayKind::Idle, "Up to date".to_string()),
            SyncState::Syncing => {
                let n = s.transfers.len();
                let text = if n > 0 {
                    format!("Syncing {}…", plural(n, "file", "files"))
                } else if s.pending > 0 {
                    format!("Syncing {}…", plural(s.pending, "item", "items"))
                } else {
                    "Syncing…".to_string()
                };
                (TrayKind::Syncing, text)
            }
            SyncState::Paused => (TrayKind::Paused, "Paused".to_string()),
            SyncState::Offline => (TrayKind::Offline, "Offline — retrying".to_string()),
            SyncState::Error => (
                TrayKind::Error,
                s.message.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| "Sync error".into()),
            ),
        };
        let active = !matches!(
            s.state,
            SyncState::SetupRequired | SyncState::SignedOut | SyncState::SigningIn
        );
        Self {
            kind,
            text,
            active,
            paused: s.state == SyncState::Paused,
            setup_needed: !active,
            sync_root: Some(s.sync_root.clone()).filter(|r| !r.is_empty()),
        }
    }
}

struct GDriveTray {
    app: tauri::AppHandle,
    refresh: Arc<Notify>,
    view: TrayView,
}

impl GDriveTray {
    /// Runs a daemon request in the background, then refreshes the tray.
    fn send(&self, req: Request) {
        let refresh = self.refresh.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = daemon::call(req).await {
                tracing::warn!("tray action failed: {e:#}");
            }
            refresh.notify_one();
        });
    }
}

impl ksni::Tray for GDriveTray {
    fn id(&self) -> String {
        "gdrive-linux".into()
    }

    fn title(&self) -> String {
        format!("Google Drive — {}", self.view.text)
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        icons::tray_pixmap(self.view.kind)
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Google Drive".into(),
            description: self.view.text.clone(),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        crate::show_main_window(&self.app);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let v = &self.view;
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem { label: v.text.replace('_', "__"), enabled: false, ..Default::default() }.into(),
            MenuItem::Separator,
            StandardItem {
                label: "Open Google Drive folder".into(),
                icon_name: "folder".into(),
                enabled: v.sync_root.is_some(),
                activate: Box::new(|t: &mut Self| {
                    if let Some(root) = t.view.sync_root.clone() {
                        commands::spawn_xdg_open(&root);
                    }
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Google Drive on the web".into(),
                icon_name: "web-browser".into(),
                activate: Box::new(|_| commands::spawn_xdg_open(DRIVE_WEB_URL)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
        ];
        if v.paused {
            items.push(
                StandardItem {
                    label: "Resume syncing".into(),
                    icon_name: "media-playback-start".into(),
                    enabled: v.active,
                    activate: Box::new(|t: &mut Self| t.send(Request::Resume)),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "Pause syncing".into(),
                    icon_name: "media-playback-pause".into(),
                    enabled: v.active,
                    activate: Box::new(|t: &mut Self| t.send(Request::Pause)),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(
            StandardItem {
                label: "Sync now".into(),
                icon_name: "view-refresh".into(),
                enabled: v.active && !v.paused,
                activate: Box::new(|t: &mut Self| t.send(Request::SyncNow)),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: if v.setup_needed { "Set up Google Drive…" } else { "Show window" }.into(),
                activate: Box::new(|t: &mut Self| crate::show_main_window(&t.app)),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.app.exit(0)),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Starts the tray and its status poller. Returns `false` if no tray could be shown
/// (no StatusNotifierItem host), in which case only the window is available.
pub async fn start(app: tauri::AppHandle, refresh: Arc<Notify>) -> bool {
    let initial = current_view().await;
    let tray = GDriveTray { app: app.clone(), refresh: refresh.clone(), view: initial.clone() };
    let handle = match tray.spawn().await {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!("tray icon unavailable: {e}");
            None
        }
    };
    let shown = handle.is_some();

    tauri::async_runtime::spawn(async move {
        let mut last = initial;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = refresh.notified() => {}
            }
            let view = current_view().await;
            if view != last {
                if let Some(h) = &handle {
                    let v = view.clone();
                    h.update(move |t| t.view = v).await;
                }
                last = view;
            }
        }
    });
    shown
}

async fn current_view() -> TrayView {
    match daemon::status_or_spawn().await {
        Ok(s) => TrayView::from_status(&s),
        Err(_) => TrayView::unreachable(),
    }
}
