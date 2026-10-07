//! The sync engine: watches both sides, and reconciles dirty items.
//!
//! For each dirty item the reconciler looks at three versions of it:
//! `S` (synced tree — last agreed state), `R` (remote tree) and `L` (local disk),
//! and derives what changed on which side since `S`. Metadata operations
//! (mkdir, moves, deletes) run inline on the engine task; content transfers run
//! as background tasks, bounded by a semaphore, and report back when done.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, Watcher};
use tokio::sync::{mpsc, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::api::{ApiError, DriveClient, NotDownloadable, Progress, SessionExpired, UploadTarget, SHORTCUT_MIME};
use crate::auth::AuthRevoked;
use crate::bandwidth::SpeedMeter;
use crate::config::{Config, META_DIR};
use crate::db::{Db, RemoteItem, RemoteLookup, SyncedItem, UploadSession};
use crate::gdoc;
use crate::local::{self, depth, rel_str, IgnoreRules, LocalStat};
use crate::status::{Account, ActivityKind, Direction, Quota, Status, SyncState, Transfer};

const DEBOUNCE: Duration = Duration::from_millis(1500);
const MAX_DEBOUNCE: Duration = Duration::from_secs(10);
const DEFER_DELAY: Duration = Duration::from_secs(3);
const QUOTA_REFRESH: Duration = Duration::from_secs(300);
const FULL_SCAN_INTERVAL: Duration = Duration::from_secs(3600);
const MAX_PATH_DEPTH: usize = 256;

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Pause,
    Resume,
    SyncNow,
    FullResync,
}

/// Owner's handle on a running engine.
pub struct EngineHandle {
    tx: mpsc::UnboundedSender<Command>,
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    pub async fn stop(mut self) {
        self.cancel.cancel();
        // The engine only awaits at points where its state is consistent, so if it
        // doesn't wind down promptly it's safe to abort it.
        if tokio::time::timeout(Duration::from_secs(5), &mut self.join).await.is_err() {
            tracing::warn!("sync engine did not stop in time; aborting it");
            self.join.abort();
            let _ = self.join.await;
        }
    }
}

/// Starts the engine on the current tokio runtime.
pub fn start(config: &Config, api: DriveClient, db: Db, status: Arc<RwLock<Status>>) -> EngineHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let engine = Engine::new(config, api, db, status, cancel.clone());
    let stop = cancel.clone();
    // Long awaits inside the engine (listing a huge Drive, API calls) don't watch the
    // token themselves; racing the whole run against it makes stopping immediate.
    let join = tokio::spawn(async move {
        tokio::select! {
            _ = stop.cancelled() => {}
            _ = engine.run(rx) => {}
        }
    });
    EngineHandle { tx, cancel, join }
}

/// A unit of reconciliation work.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Work {
    /// A Drive item (anything with a remote id or a synced row).
    Id(String),
    /// A local path with no synced row — a new local file or folder.
    Path(PathBuf),
}

enum Outcome {
    Done,
    /// Can't be handled yet (e.g. parent not created, transfer in flight); retry shortly.
    Defer,
}

struct Retry {
    attempts: u32,
    due: Instant,
}

struct Inflight {
    direction: Direction,
    total: u64,
    progress: Progress,
    started: Arc<AtomicBool>,
}

struct Done {
    path: PathBuf,
    id: Option<String>,
    result: Result<()>,
}

enum TransferKind {
    Download { remote: RemoteItem, expected: Option<LocalStat> },
    Upload { existing_id: Option<String>, parent_id: String, name: String },
}

/// State shared with transfer tasks.
struct Shared {
    root: PathBuf,
    api: DriveClient,
    db: Mutex<Db>,
    status: Arc<RwLock<Status>>,
    use_trash: bool,
}

impl Shared {
    fn db(&self) -> MutexGuard<'_, Db> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn activity(&self, kind: ActivityKind, rel: &Path, detail: Option<String>) {
        tracing::info!("{kind:?}: {}{}", rel.display(), detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default());
        self.status.write().unwrap().push_activity(kind, rel_str(rel), detail);
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join(META_DIR).join("tmp")
    }

    fn record(&self, id: &str, rel: &Path, md5: Option<String>, l: LocalStat) -> Result<()> {
        self.db().upsert_synced(&SyncedItem {
            id: id.to_owned(),
            rel_path: rel.to_path_buf(),
            is_dir: l.is_dir,
            md5,
            size: l.size,
            mtime_ns: l.mtime_ns,
            inode: l.inode,
        })
    }

    /// Removes a local file or folder, preferring the desktop trash.
    fn remove_local(&self, abs: &Path) -> Result<()> {
        if self.use_trash && trash::delete(abs).is_ok() {
            return Ok(());
        }
        let meta = std::fs::symlink_metadata(abs)?;
        if meta.is_dir() {
            std::fs::remove_dir_all(abs)?;
        } else {
            std::fs::remove_file(abs)?;
        }
        Ok(())
    }

    /// Moves a local item aside to a "conflicted copy" name, returning the new relative path.
    fn move_to_conflict_copy(&self, rel: &Path) -> Result<PathBuf> {
        let abs = self.root.join(rel);
        let target = local::conflict_path(&abs);
        std::fs::rename(&abs, &target).with_context(|| format!("renaming {}", abs.display()))?;
        let new_rel = target.strip_prefix(&self.root)?.to_path_buf();
        self.activity(ActivityKind::Conflict, rel, Some(format!("local version kept as {}", rel_str(&new_rel))));
        Ok(new_rel)
    }

    fn write_link_file(&self, remote: &RemoteItem, rel: &Path, email: &str) -> Result<LocalStat> {
        let tmp = self.tmp_dir().join(format!("{}.link", remote.id));
        std::fs::write(&tmp, gdoc::render(&remote.id, remote.link.as_deref(), email))?;
        if let Some(mtime) = remote.mtime {
            let ft = filetime::FileTime::from_unix_time(mtime.timestamp(), mtime.timestamp_subsec_nanos());
            filetime::set_file_mtime(&tmp, ft)?;
        }
        let abs = self.root.join(rel);
        std::fs::rename(&tmp, &abs)?;
        local::stat(&abs).ok_or_else(|| anyhow!("{} vanished", abs.display()))
    }
}

struct Engine {
    shared: Arc<Shared>,
    root: PathBuf,
    ignore: IgnoreRules,
    poll_interval: Duration,
    semaphore: Arc<Semaphore>,
    cancel: CancellationToken,

    email: String,
    root_id: String,
    paused: bool,
    /// Set when syncing must stop until the user intervenes (e.g. sync folder vanished).
    fatal: Option<String>,
    offline: bool,

    dirty_ids: HashSet<String>,
    dirty_paths: HashSet<PathBuf>,
    retry: HashMap<Work, Retry>,
    inflight: HashMap<PathBuf, Inflight>,
    inflight_ids: HashSet<String>,
    done_tx: mpsc::UnboundedSender<Done>,
    done_rx: Option<mpsc::UnboundedReceiver<Done>>,

    reconcile_at: Option<Instant>,
    first_event_at: Option<Instant>,
    last_quota: Option<Instant>,
    last_full_scan: Instant,
    download_meter: SpeedMeter,
    upload_meter: SpeedMeter,
}

/// The local file went away before its upload finished. Nothing is left to upload; any
/// deletion it stands for is picked up when its path is reconciled again.
#[derive(Debug)]
struct Vanished(String);

impl std::fmt::Display for Vanished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} no longer exists", self.0)
    }
}

impl std::error::Error for Vanished {}

/// Saved upload sessions older than this are not resumed: Drive expires them after
/// about a week.
fn upload_session_cutoff() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::days(6)
}

fn is_fatal(err: &anyhow::Error) -> bool {
    ApiError::is_offline(err) || err.is::<AuthRevoked>()
}

impl Engine {
    fn new(config: &Config, api: DriveClient, db: Db, status: Arc<RwLock<Status>>, cancel: CancellationToken) -> Self {
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let root = config.sync_root.clone();
        api.bandwidth().set_limits(config.max_download_mb_per_sec, config.max_upload_mb_per_sec);
        if let Err(e) = db.prune_upload_sessions(upload_session_cutoff()) {
            tracing::warn!("pruning upload sessions: {e:#}");
        }
        Self {
            shared: Arc::new(Shared {
                root: root.clone(),
                api,
                db: Mutex::new(db),
                status,
                use_trash: config.use_local_trash,
            }),
            root,
            ignore: IgnoreRules::new(&config.ignore),
            poll_interval: Duration::from_secs(config.poll_interval_secs.max(1)),
            semaphore: Arc::new(Semaphore::new(config.max_concurrent_transfers.clamp(1, 16))),
            cancel,
            email: String::new(),
            root_id: String::new(),
            paused: false,
            fatal: None,
            offline: false,
            dirty_ids: HashSet::new(),
            dirty_paths: HashSet::new(),
            retry: HashMap::new(),
            inflight: HashMap::new(),
            inflight_ids: HashSet::new(),
            done_tx,
            done_rx: Some(done_rx),
            reconcile_at: None,
            first_event_at: None,
            last_quota: None,
            last_full_scan: Instant::now(),
            download_meter: SpeedMeter::default(),
            upload_meter: SpeedMeter::default(),
        }
    }

    fn db(&self) -> MutexGuard<'_, Db> {
        self.shared.db()
    }

    fn set_state(&self, state: SyncState, message: Option<String>) {
        let mut st = self.shared.status.write().unwrap();
        if state == SyncState::Idle && st.state != SyncState::Idle {
            st.last_synced = Some(Utc::now());
        }
        st.state = state;
        st.message = message;
    }

    // ------------------------------------------------------------------ main loop

    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        self.set_state(SyncState::Starting, None);
        let (fs_tx, mut fs_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
        let mut done_rx = self.done_rx.take().expect("run called once");
        let mut watcher = None;

        // Startup is retried until it succeeds (e.g. waiting for the network at boot).
        let mut delay = Duration::from_secs(2);
        loop {
            match self.startup(&fs_tx, &mut watcher).await {
                Ok(()) => break,
                Err(e) if e.is::<AuthRevoked>() => {
                    self.set_state(SyncState::SignedOut, Some(e.to_string()));
                    return;
                }
                Err(e) => {
                    let state = if ApiError::is_offline(&e) { SyncState::Offline } else { SyncState::Error };
                    tracing::warn!("startup failed: {e:#}");
                    self.set_state(state, Some(format!("{e:#}")));
                    tokio::select! {
                        _ = self.cancel.cancelled() => return,
                        _ = tokio::time::sleep(delay) => {}
                        Some(cmd) = commands.recv() => { if let Command::Pause = cmd { self.paused = true; } }
                    }
                    delay = (delay * 2).min(Duration::from_secs(120));
                }
            }
        }

        let mut poll = tokio::time::interval(self.poll_interval);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ui_tick = tokio::time::interval(Duration::from_millis(500));
        self.reconcile_at = Some(Instant::now());

        loop {
            let wake_at = self.next_wake();
            tokio::select! {
                _ = self.cancel.cancelled() => break,
                Some(cmd) = commands.recv() => self.handle_command(cmd).await,
                Some(ev) = fs_rx.recv() => self.handle_fs_event(ev),
                Some(done) = done_rx.recv() => self.handle_done(done),
                _ = poll.tick() => {
                    if !self.paused && self.fatal.is_none() {
                        self.poll_remote_guarded().await;
                    }
                }
                _ = ui_tick.tick() => self.refresh_status(),
                _ = sleep_until_opt(wake_at) => {
                    self.reconcile_at = None;
                    self.first_event_at = None;
                    if !self.paused && self.fatal.is_none() {
                        if self.last_full_scan.elapsed() > FULL_SCAN_INTERVAL {
                            self.full_scan();
                        }
                        match self.reconcile().await {
                            Ok(()) => self.offline = false,
                            Err(e) => self.handle_cycle_error(e),
                        }
                    }
                    self.refresh_status();
                }
            }
            if self.shared.status.read().unwrap().state == SyncState::SignedOut {
                break;
            }
        }
        drop(watcher);
    }

    async fn startup(
        &mut self,
        fs_tx: &mpsc::UnboundedSender<notify::Result<notify::Event>>,
        watcher: &mut Option<notify::RecommendedWatcher>,
    ) -> Result<()> {
        let about = self.shared.api.about().await?;
        self.email = about.user.email_address.clone();
        self.apply_about(about);
        self.last_quota = Some(Instant::now());

        // Signing in with a different account must never reuse the old state:
        // the reconciler would see every old item as "deleted on Drive".
        let known = self.db().get_meta("email")?;
        if known.as_deref().is_some_and(|e| e != self.email) {
            tracing::warn!("account changed from {known:?} to {}; resetting sync state", self.email);
            self.db().reset()?;
        }

        self.check_root(true)?;
        // Partial downloads from a previous run that was interrupted.
        if let Ok(entries) = std::fs::read_dir(self.shared.tmp_dir()) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }

        // Watch before scanning so nothing slips between the scan and the watch.
        if watcher.is_none() {
            let tx = fs_tx.clone();
            let mut w = notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            })?;
            w.watch(&self.root, notify::RecursiveMode::Recursive)
                .with_context(|| format!("watching {}", self.root.display()))?;
            *watcher = Some(w);
        }

        let page_token = self.db().get_meta("page_token")?;
        let root_id = self.db().get_meta("root_id")?;
        match (page_token, root_id) {
            (Some(_), Some(root_id)) => {
                self.root_id = root_id;
                self.set_state(SyncState::Syncing, Some("Checking for changes…".into()));
                self.poll_remote().await?;
            }
            _ => self.full_remote_listing().await?,
        }
        self.db().set_meta("email", &self.email)?;
        // Catch changes made while we weren't running, and remote items never synced:
        // the work queue lives in memory, so after a crash or restart mid-sync this is
        // the only way to finish the job.
        self.scan_for_changes()?;
        Ok(())
    }

    /// Ensures the sync root exists and is the one we synced before.
    fn check_root(&self, create: bool) -> Result<()> {
        let marker = self.root.join(META_DIR);
        if !marker.is_dir() {
            if self.db().synced_count()? > 0 {
                bail!(
                    "The sync folder {} is missing or not mounted. Restore it, or sign out and back in to start over.",
                    self.root.display()
                );
            }
            if !create {
                bail!("sync folder {} is missing", self.root.display());
            }
            std::fs::create_dir_all(&marker)?;
        }
        std::fs::create_dir_all(self.shared.tmp_dir())?;
        Ok(())
    }

    fn apply_about(&self, about: crate::api::About) {
        let mut st = self.shared.status.write().unwrap();
        st.account = Some(Account {
            email: about.user.email_address,
            display_name: about.user.display_name,
            photo_link: about.user.photo_link,
        });
        st.quota = Some(Quota {
            used: about.storage_quota.usage.and_then(|u| u.parse().ok()).unwrap_or(0),
            limit: about.storage_quota.limit.and_then(|l| l.parse().ok()),
        });
    }

    fn next_wake(&self) -> Option<Instant> {
        let retry = self.retry.values().map(|r| r.due).min();
        match (self.reconcile_at, retry) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn schedule_reconcile(&mut self, delay: Duration) {
        let at = Instant::now() + delay;
        self.reconcile_at = Some(self.reconcile_at.map_or(at, |cur| cur.min(at)));
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Pause => self.paused = true,
            Command::Resume | Command::SyncNow => {
                self.paused = false;
                if self.fatal.take().is_some() {
                    self.full_scan();
                }
                for r in self.retry.values_mut() {
                    r.due = Instant::now();
                }
                self.poll_remote_guarded().await;
                self.schedule_reconcile(Duration::ZERO);
            }
            Command::FullResync => {
                self.paused = false;
                self.fatal = None;
                self.set_state(SyncState::Syncing, Some("Re-reading your whole Drive…".into()));
                match self.full_remote_listing().await {
                    Ok(()) => {
                        let ids = self.db().all_synced().map(|v| v.into_iter().map(|s| s.id).collect::<Vec<_>>());
                        if let Ok(ids) = ids {
                            self.dirty_ids.extend(ids);
                        }
                        self.full_scan();
                        self.schedule_reconcile(Duration::ZERO);
                    }
                    Err(e) => self.handle_cycle_error(e),
                }
            }
        }
        self.refresh_status();
    }

    fn handle_fs_event(&mut self, ev: notify::Result<notify::Event>) {
        let ev = match ev {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!("file watcher error: {e}; rescanning");
                self.full_scan();
                self.schedule_reconcile(DEBOUNCE);
                return;
            }
        };
        if ev.need_rescan() {
            self.full_scan();
        }
        // Reads (opening a file or listing a folder, including our own scans) change nothing.
        if matches!(ev.kind, EventKind::Access(kind) if kind != AccessKind::Close(AccessMode::Write)) {
            return;
        }
        let mut relevant = false;
        for path in ev.paths {
            let Ok(rel) = path.strip_prefix(&self.root) else { continue };
            if rel.as_os_str().is_empty() || self.ignore.is_ignored(rel) {
                continue;
            }
            self.dirty_paths.insert(rel.to_path_buf());
            relevant = true;
        }
        if relevant {
            // Debounce: wait for a quiet period so editors finish writing, but
            // never postpone processing for more than MAX_DEBOUNCE.
            let now = Instant::now();
            let first = *self.first_event_at.get_or_insert(now);
            self.reconcile_at = Some((now + DEBOUNCE).min(first + MAX_DEBOUNCE));
        }
    }

    fn handle_done(&mut self, done: Done) {
        self.inflight.remove(&done.path);
        if let Some(id) = &done.id {
            self.inflight_ids.remove(id);
        }
        let key = match &done.id {
            Some(id) => Work::Id(id.clone()),
            None => Work::Path(done.path.clone()),
        };
        let succeeded = done.result.is_ok();
        match done.result {
            Ok(()) => {
                self.retry.remove(&key);
                self.shared.status.write().unwrap().clear_error(&rel_str(&done.path));
            }
            Err(e) if e.is::<Vanished>() => {
                tracing::debug!("{e}; skipping its upload");
                self.retry.remove(&key);
                self.shared.status.write().unwrap().clear_error(&rel_str(&done.path));
                self.dirty_paths.insert(done.path.clone());
            }
            Err(e) if ApiError::is_not_found(&e) && done.id.is_some() => {
                // Updating a file that's gone from Drive: forget it, so it's re-uploaded as new.
                let _ = self.db().delete_synced(done.id.as_deref().unwrap());
                self.dirty_paths.insert(done.path.clone());
            }
            Err(e) if is_fatal(&e) => {
                self.handle_cycle_error(e);
                self.push_retry(key, None);
            }
            Err(e) => self.push_retry(key, Some(&e)),
        }
        // Re-examine the item: it may have changed while the transfer ran. A failed one
        // comes back through its retry entry once the backoff is over; re-examining it
        // now would start the transfer again straight away.
        if succeeded {
            if let Some(id) = done.id {
                self.dirty_ids.insert(id);
            }
            self.dirty_paths.insert(done.path);
        }
        self.schedule_reconcile(Duration::from_millis(200));
        self.refresh_status();
    }

    fn handle_cycle_error(&mut self, e: anyhow::Error) {
        if e.is::<AuthRevoked>() {
            self.set_state(SyncState::SignedOut, Some(e.to_string()));
        } else if ApiError::is_offline(&e) {
            tracing::info!("offline: {e}");
            self.offline = true;
            self.schedule_reconcile(Duration::from_secs(30));
        } else {
            tracing::error!("sync cycle failed: {e:#}");
            self.shared.status.write().unwrap().push_error("", format!("{e:#}"));
            self.schedule_reconcile(Duration::from_secs(30));
        }
    }

    fn push_retry(&mut self, work: Work, err: Option<&anyhow::Error>) {
        // Retrying can't help; the item is looked at again when it changes on Drive.
        let retry_in = if err.is_some_and(|e| e.is::<NotDownloadable>()) {
            self.retry.remove(&work);
            None
        } else {
            let entry = self.retry.entry(work.clone()).or_insert(Retry { attempts: 0, due: Instant::now() });
            entry.attempts += 1;
            let backoff = Duration::from_secs(5 * 2u64.pow(entry.attempts.min(9))).min(Duration::from_secs(1800));
            entry.due = Instant::now() + backoff;
            Some(backoff)
        };
        if let Some(e) = err {
            let path = match &work {
                Work::Path(p) => rel_str(p),
                Work::Id(id) => {
                    // Separate statement: the DB guard must be dropped before
                    // remote_path() locks the (non-reentrant) mutex again.
                    let synced = self.db().get_synced(id).ok().flatten();
                    match synced {
                        Some(s) => rel_str(&s.rel_path),
                        None => self.remote_path(id).ok().flatten().map_or_else(|| id.clone(), |(_, p)| rel_str(&p)),
                    }
                }
            };
            match retry_in {
                Some(backoff) => tracing::warn!("{path}: {e:#} (retry in {backoff:?})"),
                None => tracing::warn!("{path}: {e:#} (not retrying until it changes on Drive)"),
            }
            self.shared.status.write().unwrap().push_error(path, format!("{e:#}"));
        }
    }

    fn defer(&mut self, work: Work) {
        let due = Instant::now() + DEFER_DELAY;
        self.retry
            .entry(work)
            .and_modify(|r| {
                r.due = due;
                r.attempts += 1;
            })
            .or_insert(Retry { attempts: 1, due });
    }

    fn refresh_status(&mut self) {
        let transfers: Vec<Transfer> = self
            .inflight
            .iter()
            .filter(|(_, t)| t.started.load(Ordering::Relaxed))
            .map(|(p, t)| Transfer {
                path: rel_str(p),
                direction: t.direction,
                bytes_done: t.progress.load(Ordering::Relaxed).min(t.total),
                bytes_total: t.total,
            })
            .collect();
        let pending = self.dirty_ids.len() + self.dirty_paths.len() + self.inflight.len();
        let (state, message) = if let Some(msg) = &self.fatal {
            (SyncState::Error, Some(msg.clone()))
        } else if self.paused {
            (SyncState::Paused, None)
        } else if self.offline {
            (SyncState::Offline, Some("Can't reach Google Drive. Retrying…".into()))
        } else if pending > 0 {
            (SyncState::Syncing, None)
        } else {
            (SyncState::Idle, None)
        };
        let signed_out = self.shared.status.read().unwrap().state == SyncState::SignedOut;
        if !signed_out {
            self.set_state(state, message);
        }
        let mut st = self.shared.status.write().unwrap();
        st.transfers = transfers;
        st.pending = pending;
        let bandwidth = self.shared.api.bandwidth();
        st.download_bps = self.download_meter.sample(bandwidth.download.total());
        st.upload_bps = self.upload_meter.sample(bandwidth.upload.total());
    }

    // ------------------------------------------------------------------ remote tree

    /// Lists all of Drive from scratch into the remote tree.
    async fn full_remote_listing(&mut self) -> Result<()> {
        // Take the change token first so changes made during the listing are replayed after.
        let token = self.shared.api.start_page_token().await?;
        let root_id = self.shared.api.root_id().await?;
        self.set_state(SyncState::Syncing, Some("Reading your Drive…".into()));
        let mut pages = Vec::new();
        self.shared.api.list_all(|files| {
            pages.push(files);
            Ok(())
        })
        .await?;
        let count: usize = pages.iter().map(Vec::len).sum();
        tracing::info!("listed {count} remote items");
        let ids: Vec<String> = {
            let mut db = self.db();
            db.transaction(|db| {
                db.clear_remote()?;
                for f in pages.iter().flatten() {
                    db.upsert_remote(&RemoteItem::from(f))?;
                }
                db.set_meta("root_id", &root_id)?;
                db.set_meta("page_token", &token)?;
                Ok(())
            })?;
            db.all_remote_ids()?
        };
        self.root_id = root_id;
        self.dirty_ids.extend(ids);
        Ok(())
    }

    async fn poll_remote_guarded(&mut self) {
        match self.poll_remote().await {
            Ok(()) => {
                if self.offline {
                    self.offline = false;
                    self.schedule_reconcile(Duration::ZERO);
                }
            }
            Err(e) => self.handle_cycle_error(e),
        }
        if self.last_quota.is_none_or(|t| t.elapsed() > QUOTA_REFRESH) {
            if let Ok(about) = self.shared.api.about().await {
                self.apply_about(about);
                self.last_quota = Some(Instant::now());
            }
        }
    }

    /// Applies pending Drive changes to the remote tree and marks them dirty.
    async fn poll_remote(&mut self) -> Result<()> {
        let Some(mut token) = self.db().get_meta("page_token")? else {
            return Ok(());
        };
        loop {
            let page = self.shared.api.changes(&token).await?;
            let next = page.next_page_token.clone().or(page.new_start_page_token.clone());
            let mut changed = Vec::new();
            {
                let mut db = self.db();
                db.transaction(|db| {
                    for change in &page.changes {
                        let Some(id) = change.file_id.clone() else { continue };
                        match &change.file {
                            Some(f) if !change.removed && !f.trashed => db.upsert_remote(&RemoteItem::from(f))?,
                            _ => db.delete_remote(&id)?,
                        }
                        changed.push(id);
                    }
                    if let Some(t) = &next {
                        db.set_meta("page_token", t)?;
                    }
                    Ok(())
                })?;
            }
            if !changed.is_empty() {
                tracing::debug!("{} remote changes", changed.len());
                self.dirty_ids.extend(changed);
                self.schedule_reconcile(Duration::ZERO);
            }
            match (page.next_page_token, next) {
                (Some(t), _) => token = t,
                _ => return Ok(()),
            }
        }
    }

    /// Local file name for a remote item: sanitized, with a link extension for
    /// native Google files, and a " (n)" suffix when Drive has duplicate names.
    fn local_name(&self, db: &impl RemoteLookup, item: &RemoteItem) -> Result<String> {
        let base = |i: &RemoteItem| {
            let name = local::sanitize_name(&i.name);
            if i.is_native() {
                format!("{name}.{}", gdoc::extension_for(&i.mime))
            } else {
                name
            }
        };
        let name = base(item);
        if let Some(parent) = &item.parent_id {
            let twins: Vec<RemoteItem> = db
                .remote_named(parent, &item.name)?
                .into_iter()
                .filter(|t| t.mime != SHORTCUT_MIME && base(t) == name)
                .collect();
            if let Some(pos) = twins.iter().position(|t| t.id == item.id) {
                if pos > 0 {
                    return Ok(local::with_suffix(&name, &format!(" ({})", pos + 1)));
                }
            }
        }
        Ok(name)
    }

    /// Where a remote item belongs locally, or `None` if it's outside My Drive
    /// (shared-with-me, trashed ancestor, shortcut, …) or is the root itself.
    fn remote_path(&self, id: &str) -> Result<Option<(RemoteItem, PathBuf)>> {
        let db = self.db();
        self.remote_path_in(&*db, id)
    }

    fn remote_path_in(&self, db: &impl RemoteLookup, id: &str) -> Result<Option<(RemoteItem, PathBuf)>> {
        let Some(item) = db.get_remote(id)? else { return Ok(None) };
        if item.id == self.root_id || item.mime == SHORTCUT_MIME {
            return Ok(None);
        }
        let mut components = vec![self.local_name(db, &item)?];
        let mut cur = item.clone();
        loop {
            let Some(parent_id) = cur.parent_id.clone() else { return Ok(None) };
            if parent_id == self.root_id {
                break;
            }
            let Some(parent) = db.get_remote(&parent_id)? else { return Ok(None) };
            if !parent.is_dir() || components.len() > MAX_PATH_DEPTH {
                return Ok(None);
            }
            components.push(self.local_name(db, &parent)?);
            cur = parent;
        }
        let path: PathBuf = components.iter().rev().collect();
        if self.ignore.is_ignored(&path) {
            return Ok(None);
        }
        Ok(Some((item, path)))
    }

    /// Drive id of the folder that a local relative path's parent maps to.
    fn parent_id_for(&self, db: &Db, rel: &Path) -> Result<Option<String>> {
        match rel.parent() {
            None => Ok(None),
            Some(p) if p.as_os_str().is_empty() => Ok(Some(self.root_id.clone())),
            Some(p) => Ok(db.synced_by_path(p)?.filter(|s| s.is_dir).map(|s| s.id)),
        }
    }

    // ------------------------------------------------------------------ local tree

    /// Safety net for missed events (watcher overflow, periodic): see `scan_for_changes`.
    fn full_scan(&mut self) {
        if let Err(e) = self.scan_for_changes() {
            self.handle_cycle_error(e);
        }
    }

    /// Compares the synced tree against Drive and the disk in bulk, and marks dirty
    /// only the items that differ. Reconciling every item one by one instead costs a
    /// dozen queries and stats each, which takes minutes on a large Drive.
    fn scan_for_changes(&mut self) -> Result<()> {
        self.last_full_scan = Instant::now();
        let started = std::time::Instant::now();
        let (synced, remote) = {
            let db = self.db();
            (db.all_synced()?, db.remote_snapshot()?)
        };
        let mut remote_paths = HashMap::new();
        for id in remote.ids() {
            if let Some((_, path)) = self.remote_path_in(&remote, id)? {
                remote_paths.insert(id, path);
            }
        }

        // Local side: unknown paths are new (or renamed); known ones are compared by stat.
        let by_path: HashMap<&Path, &SyncedItem> = synced.iter().map(|s| (s.rel_path.as_path(), s)).collect();
        let mut unchanged_locally = HashSet::new();
        let mut new_paths = 0;
        let (ignore, root) = (&self.ignore, &self.root);
        let walker = walkdir::WalkDir::new(root).min_depth(1).follow_links(false).into_iter().filter_entry(|e| {
            e.path().strip_prefix(root).map(|rel| !ignore.is_ignored(rel)).unwrap_or(false)
        });
        for entry in walker.flatten() {
            let Ok(rel) = entry.path().strip_prefix(root) else { continue };
            let Some(l) = entry.metadata().ok().filter(|m| m.is_file() || m.is_dir()).map(|m| LocalStat::from_meta(&m))
            else {
                continue; // symlinks and special files are never synced
            };
            match by_path.get(rel) {
                None => {
                    self.dirty_paths.insert(rel.to_path_buf());
                    new_paths += 1;
                }
                Some(s) => {
                    let same = s.is_dir == l.is_dir
                        && s.inode == l.inode
                        && (s.is_dir || (s.size == l.size && s.mtime_ns == l.mtime_ns));
                    if same {
                        unchanged_locally.insert(s.id.as_str());
                    }
                }
            }
        }

        // Synced items changed on either side (or gone from one).
        let mut changed = 0;
        for s in &synced {
            let unchanged = unchanged_locally.contains(s.id.as_str())
                && remote_paths.get(s.id.as_str()).zip(remote.get(&s.id)).is_some_and(|(path, r)| {
                    *path == s.rel_path
                        && r.is_dir() == s.is_dir
                        && (s.is_dir || r.is_native() || !remote_content_changed(s, r))
                });
            if !unchanged {
                self.dirty_ids.insert(s.id.clone());
                changed += 1;
            }
        }
        // Drive items never synced.
        let synced_ids: HashSet<&str> = synced.iter().map(|s| s.id.as_str()).collect();
        let unsynced: Vec<String> =
            remote_paths.into_keys().filter(|id| !synced_ids.contains(id)).map(str::to_owned).collect();
        tracing::info!(
            "scanned {} synced items in {:?}: {changed} changed, {new_paths} new local, {} not yet synced",
            synced.len(),
            started.elapsed(),
            unsynced.len()
        );
        self.dirty_ids.extend(unsynced);
        self.schedule_reconcile(Duration::ZERO);
        Ok(())
    }

    fn inflight_under(&self, dir: &Path) -> bool {
        self.inflight.keys().any(|p| p.starts_with(dir))
    }

    // ------------------------------------------------------------------ reconciliation

    async fn reconcile(&mut self) -> Result<()> {
        self.check_root(false).inspect_err(|e| self.fatal = Some(e.to_string()))?;

        let now = Instant::now();
        let due: Vec<Work> = self.retry.iter().filter(|(_, r)| r.due <= now).map(|(w, _)| w.clone()).collect();
        for w in due {
            match w {
                Work::Id(id) => self.dirty_ids.insert(id),
                Work::Path(p) => self.dirty_paths.insert(p),
            };
        }

        let paths: Vec<PathBuf> = self.dirty_paths.drain().collect();
        let mut ids: HashSet<String> = std::mem::take(&mut self.dirty_ids);
        if paths.is_empty() && ids.is_empty() {
            return Ok(());
        }
        self.set_state(SyncState::Syncing, None);

        // Resolve dirty paths to synced items where possible.
        let mut unknown = Vec::new();
        let mut gone = Vec::new();
        {
            let db = self.db();
            for p in paths {
                if self.ignore.is_ignored(&p) {
                    continue;
                }
                if let Some(s) = db.synced_by_path(&p)? {
                    ids.insert(s.id);
                } else if std::fs::symlink_metadata(self.root.join(&p)).is_ok() {
                    unknown.push(p);
                } else {
                    gone.push(p);
                }
            }
        }
        // Neither synced nor on disk any more: nothing left to retry.
        for p in gone {
            if self.retry.remove(&Work::Path(p.clone())).is_some() {
                self.shared.status.write().unwrap().clear_error(&rel_str(&p));
            }
        }
        unknown.sort_by_key(|p| depth(p));

        // Local renames/moves first, so their old paths aren't mistaken for deletions.
        let mut new_local = Vec::new();
        for (i, p) in unknown.iter().enumerate() {
            match self.try_local_rename(p).await {
                Ok(Some(id)) => {
                    ids.insert(id);
                }
                Ok(None) => new_local.push(p.clone()),
                Err(e) if is_fatal(&e) => {
                    self.dirty_paths.extend(unknown[i..].iter().cloned());
                    self.dirty_ids.extend(ids);
                    return Err(e);
                }
                Err(e) => self.push_retry(Work::Path(p.clone()), Some(&e)),
            }
        }

        // Drive items, parents before children.
        let mut ordered: Vec<(usize, String)> = {
            let db = self.db();
            ids.into_iter()
                .map(|id| {
                    let d = match db.get_synced(&id).ok().flatten() {
                        Some(s) => depth(&s.rel_path),
                        None => self.remote_path_in(&*db, &id).ok().flatten().map_or(0, |(_, p)| depth(&p)),
                    };
                    (d, id)
                })
                .collect()
        };
        ordered.sort();
        let mut work: VecDeque<Work> = ordered.into_iter().map(|(_, id)| Work::Id(id)).collect();
        work.extend(new_local.into_iter().map(Work::Path));

        while let Some(w) = work.pop_front() {
            if self.paused || self.cancel.is_cancelled() {
                work.push_front(w);
                self.requeue(work);
                return Ok(());
            }
            let result = match &w {
                Work::Id(id) => self.reconcile_id(id).await,
                Work::Path(p) => self.reconcile_new_local(p, &mut work).await,
            };
            match result {
                Ok(Outcome::Done) => {
                    if self.retry.remove(&w).is_some() {
                        let path = match &w {
                            Work::Path(p) => Some(p.clone()),
                            Work::Id(id) => self.db().get_synced(id)?.map(|s| s.rel_path),
                        };
                        if let Some(p) = path {
                            self.shared.status.write().unwrap().clear_error(&rel_str(&p));
                        }
                    }
                }
                Ok(Outcome::Defer) => self.defer(w),
                Err(e) if is_fatal(&e) => {
                    work.push_front(w);
                    self.requeue(work);
                    return Err(e);
                }
                Err(e) => self.push_retry(w, Some(&e)),
            }
        }
        Ok(())
    }

    fn requeue(&mut self, work: VecDeque<Work>) {
        for w in work {
            match w {
                Work::Id(id) => self.dirty_ids.insert(id),
                Work::Path(p) => self.dirty_paths.insert(p),
            };
        }
    }

    async fn reconcile_id(&mut self, id: &str) -> Result<Outcome> {
        if id == self.root_id || self.inflight_ids.contains(id) {
            // In-flight items are re-examined when their transfer completes.
            return Ok(Outcome::Done);
        }
        let synced = self.db().get_synced(id)?;
        let remote = self.remote_path(id)?;
        match (synced, remote) {
            (None, None) => Ok(Outcome::Done),
            (None, Some((r, path))) => self.remote_new(r, path).await,
            (Some(s), None) => self.remote_gone(s),
            (Some(s), Some((r, path))) => self.reconcile_both(s, r, path).await,
        }
    }

    /// A Drive item we've never synced.
    async fn remote_new(&mut self, r: RemoteItem, rel: PathBuf) -> Result<Outcome> {
        if self.inflight.contains_key(&rel) {
            return Ok(Outcome::Defer);
        }
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            if self.db().synced_by_path(parent)?.is_none() {
                return Ok(Outcome::Defer); // parent folder not created yet
            }
        }
        let abs = self.root.join(&rel);
        match local::stat(&abs) {
            None if std::fs::symlink_metadata(&abs).is_ok() => {
                // A symlink or special file occupies the name; leave it alone.
                bail!("{} exists locally and is not a regular file or folder", abs.display());
            }
            None => self.create_local_from_remote(r, rel).await,
            Some(l) if l.is_dir && r.is_dir() => {
                self.shared.record(&r.id, &rel, None, l)?;
                self.sync_children_of_new_folder(&r.id).await?;
                Ok(Outcome::Done)
            }
            Some(l) if !l.is_dir && r.is_native() => {
                if gdoc::read(&abs).is_some_and(|link| link.doc_id == r.id) {
                    self.shared.record(&r.id, &rel, None, l)?;
                } else {
                    let copy = self.shared.move_to_conflict_copy(&rel)?;
                    self.dirty_paths.insert(copy);
                    let l = self.shared.write_link_file(&r, &rel, &self.email)?;
                    self.shared.record(&r.id, &rel, None, l)?;
                }
                Ok(Outcome::Done)
            }
            Some(l) if !l.is_dir && !r.is_dir() => {
                let md5 = hash(&abs).await?;
                if r.md5.as_deref() == Some(md5.as_str()) {
                    // Same content on both sides (e.g. re-linking after a reinstall).
                    self.shared.record(&r.id, &rel, Some(md5), l)?;
                    Ok(Outcome::Done)
                } else {
                    let copy = self.shared.move_to_conflict_copy(&rel)?;
                    self.dirty_paths.insert(copy);
                    self.spawn_download(r, rel, None);
                    Ok(Outcome::Done)
                }
            }
            Some(_) => {
                // File vs folder clash: keep the local one under another name.
                let copy = self.shared.move_to_conflict_copy(&rel)?;
                self.dirty_paths.insert(copy);
                self.create_local_from_remote(r, rel).await
            }
        }
    }

    async fn create_local_from_remote(&mut self, r: RemoteItem, rel: PathBuf) -> Result<Outcome> {
        let abs = self.root.join(&rel);
        if r.is_dir() {
            std::fs::create_dir(&abs).with_context(|| format!("creating {}", abs.display()))?;
            let l = local::stat(&abs).ok_or_else(|| anyhow!("{} vanished", abs.display()))?;
            self.shared.record(&r.id, &rel, None, l)?;
            self.shared.activity(ActivityKind::CreatedFolderLocal, &rel, None);
            self.sync_children_of_new_folder(&r.id).await?;
        } else if r.is_native() {
            let l = self.shared.write_link_file(&r, &rel, &self.email)?;
            self.shared.record(&r.id, &rel, None, l)?;
            self.shared.activity(ActivityKind::Downloaded, &rel, None);
        } else {
            self.spawn_download(r, rel, None);
        }
        Ok(Outcome::Done)
    }

    /// A folder that just appeared in My Drive (restored from trash, moved in from
    /// "Shared with me", …) may come without change entries for its contents, so
    /// queue the children we already know of, or fetch them if we know none.
    async fn sync_children_of_new_folder(&mut self, folder_id: &str) -> Result<()> {
        let known = self.db().remote_children(folder_id)?;
        if !known.is_empty() {
            self.dirty_ids.extend(known.into_iter().map(|c| c.id));
            return Ok(());
        }
        let children = self.shared.api.list_children(folder_id).await?;
        let db = self.db();
        for f in &children {
            db.upsert_remote(&RemoteItem::from(f))?;
        }
        drop(db);
        self.dirty_ids.extend(children.into_iter().map(|f| f.id));
        Ok(())
    }

    /// A synced item that is no longer in My Drive.
    fn remote_gone(&mut self, s: SyncedItem) -> Result<Outcome> {
        let abs = self.root.join(&s.rel_path);
        if self.inflight_under(&s.rel_path) {
            return Ok(Outcome::Defer);
        }
        let db_forget = |this: &Self| -> Result<()> {
            if s.is_dir {
                this.db().delete_synced_subtree(&s.rel_path)
            } else {
                this.db().delete_synced(&s.id)
            }
        };
        let Some(l) = local::stat(&abs) else {
            db_forget(self)?;
            return Ok(Outcome::Done);
        };
        if l.is_dir != s.is_dir {
            db_forget(self)?;
            self.dirty_paths.insert(s.rel_path);
            return Ok(Outcome::Done);
        }
        let clean = if s.is_dir { self.subtree_clean(&s.rel_path)? } else { l.size == s.size && l.mtime_ns == s.mtime_ns };
        db_forget(self)?;
        if clean {
            self.shared.remove_local(&abs)?;
            self.shared.activity(ActivityKind::DeletedLocal, &s.rel_path, None);
        } else {
            // Deleted on Drive but edited here: keep the local version and upload it again.
            self.shared.activity(
                ActivityKind::Conflict,
                &s.rel_path,
                Some("deleted on Drive but changed on this computer; uploading it again".into()),
            );
            self.dirty_paths.insert(s.rel_path);
        }
        Ok(Outcome::Done)
    }

    /// True if nothing under a local folder differs from the synced tree.
    fn subtree_clean(&self, dir: &Path) -> Result<bool> {
        let db = self.db();
        let root = &self.root;
        for entry in walkdir::WalkDir::new(root.join(dir)).min_depth(1).follow_links(false) {
            let entry = entry?;
            let rel = entry.path().strip_prefix(root)?;
            if self.ignore.is_ignored(rel) {
                continue;
            }
            let Some(l) = local::stat(entry.path()) else { continue };
            match db.synced_by_path(rel)? {
                Some(s) if s.is_dir && l.is_dir => {}
                Some(s) if !s.is_dir && !l.is_dir && s.size == l.size && s.mtime_ns == l.mtime_ns => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    async fn reconcile_both(&mut self, mut s: SyncedItem, r: RemoteItem, rpath: PathBuf) -> Result<Outcome> {
        let abs = self.root.join(&s.rel_path);
        let Some(l) = local::stat(&abs) else {
            return self.local_gone(s, r).await;
        };

        if l.is_dir != s.is_dir {
            // Replaced a file with a folder (or vice versa) locally: treat as delete + create.
            self.shared.api.trash(&r.id).await?;
            self.db().delete_remote(&r.id)?;
            self.db().delete_synced_subtree(&s.rel_path)?;
            self.shared.activity(ActivityKind::DeletedRemote, &s.rel_path, Some("replaced locally".into()));
            self.dirty_paths.insert(s.rel_path);
            return Ok(Outcome::Done);
        }

        // 1. Renamed or moved on Drive → mirror it locally.
        if rpath != s.rel_path {
            if self.inflight_under(&s.rel_path) {
                return Ok(Outcome::Defer);
            }
            let target = self.root.join(&rpath);
            if !target.parent().is_some_and(Path::is_dir) {
                return Ok(Outcome::Defer);
            }
            if std::fs::symlink_metadata(&target).is_ok() {
                let occupant = self.db().synced_by_path(&rpath)?;
                match occupant {
                    // Another synced item still sits there and will move away by itself.
                    Some(o) if self.retry.get(&Work::Id(s.id.clone())).is_none_or(|r| r.attempts < 3) => {
                        self.dirty_ids.insert(o.id);
                        return Ok(Outcome::Defer);
                    }
                    _ => {
                        let copy = self.shared.move_to_conflict_copy(&rpath)?;
                        self.dirty_paths.insert(copy);
                    }
                }
            }
            std::fs::rename(&abs, &target)
                .with_context(|| format!("moving {} to {}", abs.display(), target.display()))?;
            self.db().move_synced_subtree(&s.rel_path, &rpath)?;
            self.shared.activity(ActivityKind::MovedLocal, &rpath, Some(format!("from {}", rel_str(&s.rel_path))));
            s.rel_path = rpath;
        }

        if s.is_dir {
            return Ok(Outcome::Done);
        }
        let abs = self.root.join(&s.rel_path);
        let local_changed = l.size != s.size || l.mtime_ns != s.mtime_ns;

        // 2. Native Google files: the link file is read-only; just track its stat.
        if r.is_native() {
            if local_changed || l.inode != s.inode {
                self.shared.record(&s.id, &s.rel_path, None, l)?;
            }
            return Ok(Outcome::Done);
        }

        // 3. Content.
        let remote_changed = remote_content_changed(&s, &r);
        let mut local_md5 = None;
        let mut local_changed = local_changed;
        if local_changed {
            let md5 = hash(&abs).await?;
            if s.md5.as_deref() == Some(md5.as_str()) {
                // Touched but not modified.
                local_changed = false;
                self.shared.record(&s.id, &s.rel_path, s.md5.clone(), l)?;
            } else if r.md5.as_deref() == Some(md5.as_str()) {
                // Both sides converged on the same content.
                self.shared.record(&s.id, &s.rel_path, Some(md5), l)?;
                return Ok(Outcome::Done);
            }
            local_md5 = Some(md5);
        } else if l.inode != s.inode {
            self.shared.record(&s.id, &s.rel_path, s.md5.clone(), l)?;
        }

        match (local_changed, remote_changed) {
            (false, false) => {}
            (true, false) => {
                let parent_id = r.parent_id.clone().unwrap_or_else(|| self.root_id.clone());
                let name = r.name.clone();
                self.spawn_upload(s.rel_path, Some(s.id), parent_id, name, local_md5, l);
            }
            (false, true) => self.spawn_download(r, s.rel_path, Some(l)),
            (true, true) => {
                let copy = self.shared.move_to_conflict_copy(&s.rel_path)?;
                self.dirty_paths.insert(copy);
                self.spawn_download(r, s.rel_path, None);
            }
        }
        Ok(Outcome::Done)
    }

    /// A synced item whose local copy disappeared (and wasn't renamed).
    async fn local_gone(&mut self, s: SyncedItem, r: RemoteItem) -> Result<Outcome> {
        // If the parent folder went too, let the parent's deletion handle the whole subtree.
        if let Some(parent) = s.rel_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !self.root.join(parent).exists() {
                let parent_synced = self.db().synced_by_path(parent)?;
                if let Some(ps) = parent_synced {
                    self.dirty_ids.insert(ps.id);
                    return Ok(Outcome::Done);
                }
            }
        }
        if self.inflight_under(&s.rel_path) {
            return Ok(Outcome::Defer);
        }
        let remote_changed = !s.is_dir
            && !r.is_native()
            && match (&s.md5, &r.md5) {
                (Some(a), Some(b)) => a != b,
                _ => false,
            };
        if remote_changed {
            // Deleted here but edited on Drive: the edit wins; download it again.
            self.db().delete_synced(&s.id)?;
            self.shared.activity(
                ActivityKind::Conflict,
                &s.rel_path,
                Some("deleted on this computer but changed on Drive; restoring it".into()),
            );
            self.dirty_ids.insert(s.id);
            return Ok(Outcome::Done);
        }
        match self.shared.api.trash(&s.id).await {
            Ok(()) => {}
            Err(e) if ApiError::is_not_found(&e) => {}
            Err(e) => return Err(e),
        }
        let db = self.db();
        db.delete_remote(&s.id)?;
        if s.is_dir {
            db.delete_synced_subtree(&s.rel_path)?;
        } else {
            db.delete_synced(&s.id)?;
        }
        drop(db);
        self.shared.activity(ActivityKind::DeletedRemote, &s.rel_path, None);
        Ok(Outcome::Done)
    }

    /// Detects a local rename/move: an unknown path whose inode belongs to a synced
    /// item that's no longer at its old path. Returns the item's id if handled.
    async fn try_local_rename(&mut self, rel: &Path) -> Result<Option<String>> {
        let Some(l) = local::stat(&self.root.join(rel)) else { return Ok(None) };
        let (s, r, new_parent) = {
            let db = self.db();
            let candidate = db.synced_by_inode(l.inode)?.into_iter().find(|s| {
                s.is_dir == l.is_dir
                    && s.rel_path != rel
                    && local::stat(&self.root.join(&s.rel_path)).is_none_or(|cur| cur.inode != l.inode)
                    && (s.is_dir || (s.size == l.size && s.mtime_ns == l.mtime_ns))
            });
            let Some(s) = candidate else { return Ok(None) };
            let Some(r) = db.get_remote(&s.id)? else { return Ok(None) };
            let Some(new_parent) = self.parent_id_for(&db, rel)? else { return Ok(None) };
            (s, r, new_parent)
        };
        if s.is_dir && self.inflight_under(&s.rel_path) {
            self.defer(Work::Path(rel.to_path_buf()));
            return Ok(Some(s.id));
        }

        let file_name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let new_name = if r.is_native() {
            match file_name.rsplit_once('.') {
                Some((stem, ext)) if gdoc::is_link_extension(ext) => stem.to_owned(),
                _ => file_name,
            }
        } else {
            file_name
        };
        let current_local_name = s.rel_path.file_name().map(|n| n.to_string_lossy().into_owned());
        let renamed = rel.file_name().map(|n| n.to_string_lossy().into_owned()) != current_local_name;
        let old_parent = r.parent_id.clone().unwrap_or_else(|| self.root_id.clone());
        let moved = old_parent != new_parent;

        let updated = self
            .shared
            .api
            .update_metadata(
                &s.id,
                renamed.then_some(new_name.as_str()),
                moved.then_some((new_parent.as_str(), old_parent.as_str())),
            )
            .await?;
        {
            let mut db = self.db();
            db.upsert_remote(&RemoteItem::from(&updated))?;
            db.move_synced_subtree(&s.rel_path, rel)?;
        }
        self.shared.activity(ActivityKind::MovedRemote, rel, Some(format!("from {}", rel_str(&s.rel_path))));
        Ok(Some(s.id))
    }

    /// A local path with no synced row: a new file or folder to upload.
    async fn reconcile_new_local(&mut self, rel: &Path, work: &mut VecDeque<Work>) -> Result<Outcome> {
        if self.ignore.is_ignored(rel) || self.inflight.contains_key(rel) {
            return Ok(Outcome::Done);
        }
        let abs = self.root.join(rel);
        let Some(l) = local::stat(&abs) else { return Ok(Outcome::Done) };
        enum Next {
            Synced(String),
            AdoptRemote(String),
            Upload(String),
        }
        let next = {
            let db = self.db();
            if let Some(s) = db.synced_by_path(rel)? {
                Next::Synced(s.id)
            } else {
                let Some(parent_id) = self.parent_id_for(&db, rel)? else {
                    return Ok(Outcome::Defer);
                };
                // A Drive item with this local name is waiting to be synced: let the
                // remote side adopt or conflict-resolve this path instead.
                let name = rel.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let mut adopt = None;
                for candidate_name in remote_name_candidates(&name) {
                    for candidate in db.remote_named(&parent_id, &candidate_name)? {
                        if db.get_synced(&candidate.id)?.is_none() && self.local_name(&*db, &candidate)? == name {
                            adopt = Some(candidate.id);
                        }
                    }
                }
                match adopt {
                    Some(id) => Next::AdoptRemote(id),
                    None => Next::Upload(parent_id),
                }
            }
        };
        let parent_id = match next {
            Next::Synced(id) => {
                self.dirty_ids.insert(id);
                return Ok(Outcome::Done);
            }
            Next::AdoptRemote(id) => {
                work.push_front(Work::Id(id));
                return Ok(Outcome::Done);
            }
            Next::Upload(parent_id) => parent_id,
        };

        let name = rel.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if l.is_dir {
            let folder = self.shared.api.create_folder(&name, &parent_id).await?;
            self.db().upsert_remote(&RemoteItem::from(&folder))?;
            self.shared.record(&folder.id, rel, None, l)?;
            self.shared.activity(ActivityKind::CreatedFolderRemote, rel, None);
            // Folders moved in from outside arrive as a single event; queue their contents.
            if let Ok(entries) = std::fs::read_dir(&abs) {
                for entry in entries.flatten() {
                    let child = rel.join(entry.file_name());
                    if !self.ignore.is_ignored(&child) {
                        work.push_back(Work::Path(child));
                    }
                }
            }
        } else if gdoc::read(&abs).is_some() {
            // A copied .gdoc link file has no content to upload.
            tracing::debug!("skipping link file {}", rel.display());
        } else {
            self.spawn_upload(rel.to_path_buf(), None, parent_id, name, None, l);
        }
        Ok(Outcome::Done)
    }

    // ------------------------------------------------------------------ transfers

    fn spawn_download(&mut self, remote: RemoteItem, rel: PathBuf, expected: Option<LocalStat>) {
        let total = remote.size;
        let id = remote.id.clone();
        self.spawn_transfer(rel, Some(id), Direction::Download, total, TransferKind::Download { remote, expected }, None);
    }

    fn spawn_upload(
        &mut self,
        rel: PathBuf,
        existing_id: Option<String>,
        parent_id: String,
        name: String,
        md5: Option<String>,
        l: LocalStat,
    ) {
        let id = existing_id.clone();
        self.spawn_transfer(rel, id, Direction::Upload, l.size, TransferKind::Upload { existing_id, parent_id, name }, md5);
    }

    fn spawn_transfer(
        &mut self,
        rel: PathBuf,
        id: Option<String>,
        direction: Direction,
        total: u64,
        kind: TransferKind,
        md5: Option<String>,
    ) {
        let progress: Progress = Arc::new(AtomicU64::new(0));
        let started = Arc::new(AtomicBool::new(false));
        if let Some(id) = &id {
            self.inflight_ids.insert(id.clone());
        }
        self.inflight.insert(
            rel.clone(),
            Inflight { direction, total, progress: progress.clone(), started: started.clone() },
        );
        let shared = self.shared.clone();
        let semaphore = self.semaphore.clone();
        let done_tx = self.done_tx.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => Err(anyhow!("cancelled")),
                r = async {
                    let _permit = semaphore.acquire().await?;
                    started.store(true, Ordering::Relaxed);
                    match kind {
                        TransferKind::Download { remote, expected } => {
                            run_download(&shared, &remote, &rel, expected, &progress).await
                        }
                        TransferKind::Upload { existing_id, parent_id, name } => {
                            run_upload(&shared, existing_id.as_deref(), &parent_id, &name, &rel, md5, &progress).await
                        }
                    }
                } => r,
            };
            let _ = done_tx.send(Done { path: rel, id, result });
        });
    }
}

/// Whether a file's content on Drive differs from what was last synced.
fn remote_content_changed(s: &SyncedItem, r: &RemoteItem) -> bool {
    match (&s.md5, &r.md5) {
        (Some(a), Some(b)) => a != b,
        _ => s.size != r.size,
    }
}

/// Drive names that could map to a given local name (see `Engine::local_name`).
fn remote_name_candidates(local_name: &str) -> Vec<String> {
    let mut out = vec![local_name.to_owned()];
    if local_name.contains('∕') {
        out.push(local_name.replace('∕', "/"));
    }
    if let Some((stem, ext)) = local_name.rsplit_once('.') {
        if gdoc::is_link_extension(ext) {
            out.push(stem.to_owned());
        }
    }
    out
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn hash(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || local::md5_file(&path)).await?
}

async fn run_download(
    shared: &Shared,
    remote: &RemoteItem,
    rel: &Path,
    expected: Option<LocalStat>,
    progress: &Progress,
) -> Result<()> {
    let tmp = shared.tmp_dir().join(format!("{}.part", remote.id));
    let result = async {
        let md5 = shared.api.download(&remote.id, &tmp, progress).await?;
        if let Some(want) = &remote.md5 {
            if *want != md5 {
                bail!("checksum mismatch after download (file changed on Drive meanwhile?)");
            }
        }
        if let Some(mtime) = remote.mtime {
            let ft = filetime::FileTime::from_unix_time(mtime.timestamp(), mtime.timestamp_subsec_nanos());
            filetime::set_file_mtime(&tmp, ft)?;
        }
        let abs = shared.root.join(rel);
        // The local file may have changed while we downloaded: never clobber it.
        let current = local::stat(&abs);
        let unchanged = match (expected, current) {
            (_, None) => true,
            (Some(e), Some(c)) => e.size == c.size && e.mtime_ns == c.mtime_ns,
            (None, Some(_)) => false,
        };
        if !unchanged {
            shared.move_to_conflict_copy(rel)?;
        }
        std::fs::rename(&tmp, &abs).with_context(|| format!("placing {}", abs.display()))?;
        let l = local::stat(&abs).ok_or_else(|| anyhow!("{} vanished", abs.display()))?;
        shared.record(&remote.id, rel, Some(md5), l)?;
        shared.activity(ActivityKind::Downloaded, rel, None);
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

async fn run_upload(
    shared: &Shared,
    existing_id: Option<&str>,
    parent_id: &str,
    name: &str,
    rel: &Path,
    md5: Option<String>,
    progress: &Progress,
) -> Result<()> {
    let abs = shared.root.join(rel);
    let result = upload_file(shared, existing_id, parent_id, name, rel, &abs, md5, progress).await;
    let result = match result {
        Err(_) if local::stat(&abs).is_none() => Err(Vanished(rel_str(rel)).into()),
        r => r,
    };
    // Keep the session only when the next attempt can pick it up where this one stopped.
    if result.as_ref().err().is_none_or(|e| !ApiError::is_transient(e)) {
        if let Err(e) = shared.db().delete_upload_session(rel) {
            tracing::warn!("{}: forgetting upload session: {e:#}", rel_str(rel));
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn upload_file(
    shared: &Shared,
    existing_id: Option<&str>,
    parent_id: &str,
    name: &str,
    rel: &Path,
    abs: &Path,
    md5: Option<String>,
    progress: &Progress,
) -> Result<()> {
    // Stat before reading: if the file changes mid-upload, the next pass sees a
    // newer mtime than what we record here and uploads again.
    let before = local::stat(abs).ok_or_else(|| Vanished(rel_str(rel)))?;
    let md5 = match md5 {
        Some(m) => m,
        None => hash(abs).await?,
    };
    let file = send_upload(shared, existing_id, parent_id, name, rel, abs, &before, progress).await?;
    shared.db().upsert_remote(&RemoteItem::from(&file))?;
    let recorded_md5 = file.md5_checksum.clone().or(Some(md5));
    shared.record(&file.id, rel, recorded_md5, before)?;
    shared.activity(ActivityKind::Uploaded, rel, None);
    Ok(())
}

/// Sends the file's content, continuing a saved upload session for this exact file and
/// destination when there is one, or starting (and saving) a new session otherwise.
#[allow(clippy::too_many_arguments)]
async fn send_upload(
    shared: &Shared,
    existing_id: Option<&str>,
    parent_id: &str,
    name: &str,
    rel: &Path,
    abs: &Path,
    before: &LocalStat,
    progress: &Progress,
) -> Result<crate::api::DriveFile> {
    let saved = shared.db().get_upload_session(rel)?.filter(|s| {
        s.file_id.as_deref() == existing_id
            && s.parent_id == parent_id
            && s.name == name
            && s.size == before.size
            && s.mtime_ns == before.mtime_ns
            && s.created > upload_session_cutoff()
    });
    if let Some(s) = saved {
        tracing::info!("{}: resuming upload", rel_str(rel));
        match shared.api.upload_to_session(&s.url, abs, None, progress).await {
            Err(e) if e.is::<SessionExpired>() => tracing::info!("{}: upload session expired, starting over", rel_str(rel)),
            r => return r,
        }
    }
    let target = match existing_id {
        Some(id) => UploadTarget::Update { id },
        None => UploadTarget::New { name, parent: parent_id },
    };
    let url = shared.api.start_upload(&target, before.size, before.mtime()).await?;
    shared.db().put_upload_session(&UploadSession {
        rel_path: rel.to_path_buf(),
        url: url.clone(),
        file_id: existing_id.map(str::to_owned),
        parent_id: parent_id.to_owned(),
        name: name.to_owned(),
        size: before.size,
        mtime_ns: before.mtime_ns,
        created: chrono::Utc::now(),
    })?;
    shared.api.upload_to_session(&url, abs, Some(0), progress).await
}
