use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 200;
const MAX_ERRORS: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    /// No OAuth client configured yet.
    SetupRequired,
    /// Configured, but no Google account is signed in.
    SignedOut,
    /// Sign-in started; waiting for the browser redirect.
    SigningIn,
    Starting,
    /// Everything is in sync.
    Idle,
    Syncing,
    Paused,
    /// Network is unreachable; retrying.
    Offline,
    /// A fatal problem stopped syncing (see `message`).
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub email: String,
    pub display_name: String,
    pub photo_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quota {
    pub used: u64,
    /// `None` for unlimited plans.
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transfer {
    pub path: String,
    pub direction: Direction,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Uploaded,
    Downloaded,
    CreatedFolderLocal,
    CreatedFolderRemote,
    MovedLocal,
    MovedRemote,
    DeletedLocal,
    DeletedRemote,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Activity {
    pub time: DateTime<Utc>,
    pub kind: ActivityKind,
    /// Path relative to the sync root.
    pub path: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemError {
    pub time: DateTime<Utc>,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    /// A newer version exists. If `automatic`, it installs once syncing is idle.
    Available,
    Downloading,
    /// Verified and swapped in; the daemon is restarting into it.
    Installing,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateInfo {
    pub latest: String,
    pub phase: UpdatePhase,
    /// True when this install can update itself (AppImage in a writable folder,
    /// automatic updates enabled); otherwise the user updates manually.
    pub automatic: bool,
    pub release_url: String,
    pub message: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// Snapshot of the daemon's state, served over IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub state: SyncState,
    /// Human-readable explanation for `Error`/`Offline`/`SigningIn` states.
    pub message: Option<String>,
    pub account: Option<Account>,
    pub quota: Option<Quota>,
    pub sync_root: String,
    /// Number of items waiting to be reconciled.
    pub pending: usize,
    pub transfers: Vec<Transfer>,
    /// Current total download speed across all transfers, in bytes per second.
    #[serde(default)]
    pub download_bps: u64,
    /// Current total upload speed across all transfers, in bytes per second.
    #[serde(default)]
    pub upload_bps: u64,
    /// Newest first.
    pub recent: VecDeque<Activity>,
    /// Newest first.
    pub errors: VecDeque<ItemError>,
    pub last_synced: Option<DateTime<Utc>>,
    /// Version of the running daemon.
    #[serde(default)]
    pub version: String,
    /// Set when a newer release exists.
    #[serde(default)]
    pub update: Option<UpdateInfo>,
}

/// "4.2 MB", "310 KB", "12 B" (binary units, like file managers).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

impl Status {
    /// "↓ 4.2 MB/s · ↑ 310 KB/s" for whichever directions are moving; `None` when idle.
    pub fn speed_text(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.download_bps > 0 {
            parts.push(format!("↓ {}/s", human_bytes(self.download_bps)));
        }
        if self.upload_bps > 0 {
            parts.push(format!("↑ {}/s", human_bytes(self.upload_bps)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    pub fn new(state: SyncState, sync_root: String) -> Self {
        Self {
            state,
            message: None,
            account: None,
            quota: None,
            sync_root,
            pending: 0,
            transfers: Vec::new(),
            download_bps: 0,
            upload_bps: 0,
            recent: VecDeque::new(),
            errors: VecDeque::new(),
            last_synced: None,
            version: crate::update::CURRENT_VERSION.to_owned(),
            update: None,
        }
    }

    pub fn push_activity(&mut self, kind: ActivityKind, path: impl Into<String>, detail: Option<String>) {
        self.recent.push_front(Activity { time: Utc::now(), kind, path: path.into(), detail });
        self.recent.truncate(MAX_RECENT);
    }

    pub fn push_error(&mut self, path: impl Into<String>, message: impl Into<String>) {
        let path = path.into();
        // Repeated failures of the same item replace the older entry.
        self.errors.retain(|e| e.path != path);
        self.errors.push_front(ItemError { time: Utc::now(), path, message: message.into() });
        self.errors.truncate(MAX_ERRORS);
    }

    pub fn clear_error(&mut self, path: &str) {
        self.errors.retain(|e| e.path != path);
    }
}
