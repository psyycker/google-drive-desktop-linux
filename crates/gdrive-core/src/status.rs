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
    /// Newest first.
    pub recent: VecDeque<Activity>,
    /// Newest first.
    pub errors: VecDeque<ItemError>,
    pub last_synced: Option<DateTime<Utc>>,
}

impl Status {
    pub fn new(state: SyncState, sync_root: String) -> Self {
        Self {
            state,
            message: None,
            account: None,
            quota: None,
            sync_root,
            pending: 0,
            transfers: Vec::new(),
            recent: VecDeque::new(),
            errors: VecDeque::new(),
            last_synced: None,
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
