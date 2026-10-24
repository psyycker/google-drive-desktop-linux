//! gdrived — the gdrive-linux sync daemon.
//!
//! Owns the sync engine's lifecycle (sign-in, sign-out, config changes) and
//! serves the JSON IPC protocol on a Unix socket for the tray app and CLI.

mod updater;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, bail, Context, Result};
use gdrive_core::auth::{self, Authenticator, Token};
use gdrive_core::bandwidth::Bandwidth;
use gdrive_core::config::{self, Config, SelectedFolder};
use gdrive_core::db::Db;
use gdrive_core::engine::{self, Command, EngineHandle};
use gdrive_core::ipc::{Coverage, FolderEntry, Request, Response};
use gdrive_core::layout::Layout;
use gdrive_core::local;
use gdrive_core::relocate::{self, MoveProgress};
use gdrive_core::selection::DriveFolders;
use gdrive_core::status::{ActivityKind, Relocation, Status, SyncState};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

struct Daemon {
    config: Config,
    http: reqwest::Client,
    status: Arc<RwLock<Status>>,
    engine: Option<EngineHandle>,
    /// Throttles of the running engine's Drive client, for live limit changes.
    bandwidth: Option<Arc<Bandwidth>>,
    login: Option<tokio::task::JoinHandle<()>>,
    /// Wakes the updater for an immediate check.
    check_updates: Arc<tokio::sync::Notify>,
    /// Ends the main loop, as SIGTERM does.
    shutdown: Arc<tokio::sync::Notify>,
    /// A folder is being moved between drives; the engine stays stopped until done.
    relocating: bool,
}

type Shared = Arc<Mutex<Daemon>>;

impl Daemon {
    fn set_state(&self, state: SyncState, message: Option<String>) {
        let mut st = self.status.write().unwrap();
        st.state = state;
        st.message = message;
    }

    fn publish_selection(&self) {
        self.status.write().unwrap().selected_folders = self.config.selected_folders.as_ref().map(Vec::len);
    }

    /// Starts the engine if configured and signed in; otherwise reflects why not.
    fn start_engine(&mut self) -> Result<()> {
        self.publish_selection();
        if !self.config.is_complete() {
            self.set_state(SyncState::SetupRequired, None);
            return Ok(());
        }
        let Some(token) = Token::load()? else {
            self.set_state(SyncState::SignedOut, None);
            return Ok(());
        };
        let auth = Authenticator::new(self.http.clone(), &self.config, token);
        let api = gdrive_core::api::DriveClient::new(self.http.clone(), auth);
        self.bandwidth = Some(api.bandwidth().clone());
        let db = Db::open(&config::db_file())?;
        self.status.write().unwrap().sync_root = self.config.sync_root.display().to_string();
        self.engine = Some(engine::start(&self.config, api, db, self.status.clone()));
        tracing::info!("sync engine started for {}", self.config.sync_root.display());
        Ok(())
    }

    async fn stop_engine(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.stop().await;
        }
        self.bandwidth = None;
    }

    /// Drops the handle of an engine that exited on its own (e.g. revoked sign-in).
    fn reap_engine(&mut self) {
        if self.engine.as_ref().is_some_and(EngineHandle::is_finished) {
            self.engine = None;
        }
    }

    fn send(&mut self, cmd: Command) -> Response {
        self.reap_engine();
        match &self.engine {
            Some(engine) => {
                engine.send(cmd);
                Response::Ok
            }
            None => Response::Error { message: "not syncing — sign in first".into() },
        }
    }
}

async fn handle(daemon: &Shared, req: Request) -> Result<Response> {
    let mut d = daemon.lock().await;
    d.reap_engine();
    Ok(match req {
        Request::Status => Response::Status { status: Box::new(d.status.read().unwrap().clone()) },
        Request::Pause => d.send(Command::Pause),
        Request::Resume => d.send(Command::Resume),
        Request::SyncNow => d.send(Command::SyncNow),
        Request::FullResync => d.send(Command::FullResync),
        Request::GetConfig => Response::Config { config: d.config.clone() },
        Request::CheckForUpdates => {
            d.check_updates.notify_one();
            Response::Ok
        }
        Request::Lookup { path } => lookup(&d, &path)?,
        Request::Relocate { folder, location } => {
            start_relocation(daemon, &mut d, folder, location).await?;
            Response::Ok
        }
        Request::GetFolders => Response::Folders { folders: current_folders(&d)? },
        Request::Browse { path } => browse(&d, path)?,
        Request::SetFolders { folders } => {
            set_folders(&mut d, folders).await?;
            Response::Ok
        }
        Request::Shutdown => {
            tracing::info!("shutdown requested");
            d.shutdown.notify_one();
            Response::Ok
        }
        Request::SetConfig { config } => {
            apply_config(&mut d, config).await?;
            Response::Ok
        }
        Request::StartLogin => {
            let url = start_login(daemon, &mut d).await?;
            Response::LoginUrl { url }
        }
        Request::Redownload => {
            redownload(&mut d).await?;
            Response::Ok
        }
        Request::SignOut => {
            d.stop_engine().await;
            if let Some(login) = d.login.take() {
                login.abort();
            }
            Token::delete()?;
            Db::open(&config::db_file())?.reset()?;
            let root = d.config.sync_root.display().to_string();
            let update = d.status.read().unwrap().update.clone();
            let mut fresh = Status::new(SyncState::SignedOut, root);
            fresh.update = update;
            *d.status.write().unwrap() = fresh;
            d.publish_selection();
            tracing::info!("signed out");
            Response::Ok
        }
    })
}

/// The Drive item synced at `path`, for the file manager's right-click menu.
fn lookup(d: &Daemon, path: &std::path::Path) -> Result<Response> {
    let layout = Layout::from_config(&d.config);
    let Some(rel) = layout.rel(path).filter(|r| !r.as_os_str().is_empty()) else {
        bail!("{} is not inside the Drive folder", path.display());
    };
    let db = Db::open(&config::db_file())?;
    let Some(synced) = db.synced_by_path(&rel)? else {
        bail!("{} is not synced yet", rel.display());
    };
    let link = db
        .get_remote(&synced.id)?
        .and_then(|r| r.link)
        .unwrap_or_else(|| gdrive_core::nautilus::fallback_link(&synced.id, synced.is_dir));
    let placement = d.config.placements.iter().find(|p| rel.starts_with(&p.folder));
    Ok(Response::Item {
        id: synced.id,
        link,
        is_dir: synced.is_dir,
        placed_in: placement.map(|p| p.location.clone()),
        placed_root: placement.is_some_and(|p| p.folder == rel),
        locations: d.config.locations.iter().map(|l| l.name.clone()).collect(),
        path: rel,
    })
}

/// Starts moving a folder between drives in the background: the engine is stopped
/// for the duration, the files are copied, and the new placement is saved before
/// the old copy is removed.
async fn start_relocation(daemon: &Shared, d: &mut Daemon, folder: PathBuf, location: Option<String>) -> Result<()> {
    if d.relocating {
        bail!("another folder is being moved; wait for it to finish");
    }
    if let Some(name) = &location {
        if d.config.location(name).is_none() {
            bail!("there is no storage location called \"{name}\"");
        }
    }
    let folder = relocate::normalize(&drive_rel(&d.config, folder)?)?;
    if location.is_some() && !d.config.is_selected(&folder) {
        bail!("{} isn't synced; add it with `gdrive folders add` first", folder.display());
    }

    d.relocating = true;
    d.stop_engine().await;
    let label = location.clone().unwrap_or_else(|| "the main drive".into());
    tracing::info!("moving {} to {label}", folder.display());
    let relocation = Relocation { folder: local::rel_str(&folder), to: location.clone(), bytes_done: 0, bytes_total: 0 };
    {
        let mut st = d.status.write().unwrap();
        st.relocation = Some(relocation.clone());
        st.state = SyncState::Syncing;
        st.message = Some(format!("Moving {} to {label}…", folder.display()));
    }

    let progress = Arc::new(MoveProgress::default());
    let ticker = {
        let (status, progress) = (d.status.clone(), progress.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Some(r) = status.write().unwrap().relocation.as_mut() {
                    r.bytes_done = progress.done.load(Ordering::Relaxed);
                    r.bytes_total = progress.total.load(Ordering::Relaxed);
                }
            }
        })
    };
    let config = d.config.clone();
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let job = tokio::task::spawn_blocking(move || -> Result<(Config, PathBuf)> {
            let db = Db::open(&config::db_file())?;
            let mut saved = None;
            let volume = relocate::relocate(&config, &db, &folder, location.as_deref(), &progress, &mut |next| {
                next.save()?;
                saved = Some(next.clone());
                Ok(())
            })?;
            Ok((saved.expect("relocate commits before returning Ok"), volume))
        });
        let result = job.await.map_err(anyhow::Error::from).and_then(|r| r);
        ticker.abort();
        let mut d = daemon.lock().await;
        d.relocating = false;
        d.status.write().unwrap().relocation = None;
        match result {
            Ok((next, volume)) => {
                d.config = next;
                tracing::info!("moved {} to {label}", relocation.folder);
                d.status.write().unwrap().push_activity(ActivityKind::MovedLocal, relocation.folder.clone(), Some(format!("to {label}")));
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = local::delete_discarded(&volume) {
                        tracing::warn!("cannot delete the old copy: {e:#}");
                    }
                });
            }
            Err(e) => {
                tracing::error!("moving {} failed: {e:#}", relocation.folder);
                d.status.write().unwrap().push_error(relocation.folder.clone(), format!("Could not move it to {label}: {e:#}"));
            }
        }
        if let Err(e) = d.start_engine() {
            d.set_state(SyncState::Error, Some(format!("{e:#}")));
        }
    });
    Ok(())
}

/// A folder given as an absolute local path or a Drive-relative one, as Drive-relative.
fn drive_rel(config: &Config, path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Ok(path);
    }
    Layout::from_config(config).rel(&path).ok_or_else(|| anyhow!("{} is not inside the Drive folder", path.display()))
}

/// Like [`drive_rel`], without `.` or trailing parts; `..` is refused. The Drive root
/// itself is allowed only when `root_ok`.
fn drive_folder(config: &Config, path: PathBuf, root_ok: bool) -> Result<PathBuf> {
    let rel = drive_rel(config, path)?;
    let mut out = PathBuf::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            _ => bail!("{} is not a folder inside the Drive", rel.display()),
        }
    }
    if out.as_os_str().is_empty() && !root_ok {
        bail!("pick a folder inside the Drive (to sync all of it, use `gdrive folders all`)");
    }
    Ok(out)
}

/// The selected folders with their current paths; `None` when the whole Drive syncs.
fn current_folders(d: &Daemon) -> Result<Option<Vec<SelectedFolder>>> {
    let Some(selected) = &d.config.selected_folders else { return Ok(None) };
    let db = Db::open(&config::db_file())?;
    match DriveFolders::new(&db) {
        Ok(drive) => Ok(Some(drive.refresh(selected)?)),
        // Not listed yet: the paths saved with the selection are all there is.
        Err(_) => Ok(Some(selected.clone())),
    }
}

fn browse(d: &Daemon, path: PathBuf) -> Result<Response> {
    let rel = drive_folder(&d.config, path, true)?;
    let selected = current_folders(d)?;
    let db = Db::open(&config::db_file())?;
    let drive = DriveFolders::new(&db)?;
    let id = drive.resolve(&rel)?;
    let folders = drive
        .subfolders(&id)?
        .into_iter()
        .map(|f| {
            let path = rel.join(local::sanitize_name(&f.name));
            let coverage = match &selected {
                None => Coverage::Whole,
                Some(sel) if sel.iter().any(|s| path.starts_with(&s.path)) => Coverage::Whole,
                Some(sel) if sel.iter().any(|s| s.path.starts_with(&path)) => Coverage::Partly,
                Some(_) => Coverage::Unsynced,
            };
            FolderEntry { name: f.name, path, coverage }
        })
        .collect();
    Ok(Response::Listing { folders })
}

/// Changes which Drive folders are synced and restarts the engine, which downloads the
/// newly selected ones and removes local copies of the rest.
async fn set_folders(d: &mut Daemon, folders: Option<Vec<PathBuf>>) -> Result<()> {
    if d.relocating {
        bail!("a folder is being moved; wait for it to finish");
    }
    let mut new = d.config.clone();
    new.selected_folders = match folders {
        None => None,
        Some(paths) => {
            let current = current_folders(d)?.unwrap_or_default();
            let db = Db::open(&config::db_file())?;
            let drive = DriveFolders::new(&db);
            let mut next: Vec<SelectedFolder> = Vec::new();
            for path in paths {
                let rel = drive_folder(&d.config, path, false)?;
                let folder = match current.iter().find(|f| f.path == rel) {
                    Some(f) => f.clone(),
                    None => {
                        let drive = drive.as_ref().map_err(|e| anyhow!("{e:#}"))?;
                        SelectedFolder { id: drive.resolve(&rel)?, path: rel }
                    }
                };
                if !next.iter().any(|f| f.id == folder.id) {
                    next.push(folder);
                }
            }
            Some(next)
        }
    };
    if let Some(p) = new.placements.iter().find(|p| !new.is_selected(&p.folder)) {
        bail!(
            "{0} is kept on \"{1}\"; move it back first with `gdrive move \"{0}\" main`",
            p.folder.display(),
            p.location
        );
    }
    if new == d.config {
        return Ok(());
    }
    new.save()?;
    d.stop_engine().await;
    d.config = new;
    match &d.config.selected_folders {
        None => tracing::info!("syncing the whole Drive"),
        Some(sel) => tracing::info!("syncing {} selected folder(s)", sel.len()),
    }
    d.start_engine()
}

/// Wipes the sync folder and the sync state, then restarts the engine, which sees an
/// empty folder and a never-synced Drive and downloads everything again.
async fn redownload(d: &mut Daemon) -> Result<()> {
    if d.relocating {
        bail!("a folder is being moved; wait for it to finish");
    }
    let root = d.config.sync_root.clone();
    local::check_discardable(&root)?;
    d.stop_engine().await;
    // Forget the state first: an engine started with the old state and an empty folder
    // would take every missing file for a local delete and trash it on Drive.
    Db::open(&config::db_file())?.reset()?;
    let update = d.status.read().unwrap().update.clone();
    let mut fresh = Status::new(SyncState::Starting, root.display().to_string());
    fresh.update = update;
    *d.status.write().unwrap() = fresh;
    if let Err(e) = local::discard_contents(&root) {
        d.set_state(SyncState::Error, Some(format!("Could not clear the sync folder: {e:#}")));
        return Err(e);
    }
    let layout = Layout::from_config(&d.config);
    for folder in layout.placed_folders() {
        let abs = layout.abs(folder);
        if abs.exists() {
            if let Err(e) = local::discard(layout.volume_of(folder), &abs) {
                d.set_state(SyncState::Error, Some(format!("Could not clear {}: {e:#}", abs.display())));
                return Err(e);
            }
        }
    }
    tracing::info!("cleared {}; downloading everything again", root.display());
    let volumes: Vec<PathBuf> = layout.volumes().into_iter().map(PathBuf::from).collect();
    tokio::task::spawn_blocking(move || {
        for volume in volumes {
            if let Err(e) = local::delete_discarded(&volume) {
                tracing::warn!("cannot delete discarded files: {e:#}");
            }
        }
    });
    d.start_engine()
}

async fn apply_config(d: &mut Daemon, mut new: Config) -> Result<()> {
    new.client_id = new.client_id.trim().to_owned();
    new.client_secret = new.client_secret.trim().to_owned();
    if !new.sync_root.is_absolute() {
        bail!("the sync folder must be an absolute path");
    }
    for limit in [&mut new.max_download_mb_per_sec, &mut new.max_upload_mb_per_sec] {
        if !limit.is_finite() || *limit < 0.0 {
            *limit = 0.0;
        }
    }
    // Placements only change by moving files (Relocate), and the selection through
    // SetFolders, never through settings.
    new.placements = d.config.placements.clone();
    new.selected_folders = d.config.selected_folders.clone();
    for loc in &mut new.locations {
        loc.name = loc.name.trim().to_owned();
    }
    for p in &d.config.placements {
        let before = d.config.location(&p.location).map(|l| &l.root);
        if new.location(&p.location).map(|l| &l.root) != before {
            bail!("\"{}\" holds {}; move it back to the main drive before changing or removing that drive", p.location, p.folder.display());
        }
    }
    new.validate_storage()?;
    if new == d.config {
        return Ok(());
    }
    if d.relocating {
        bail!("a folder is being moved; wait for it to finish");
    }
    let old = std::mem::replace(&mut d.config, new);
    d.config.save()?;

    // Speed limits apply live; everything else restarts the engine.
    let limits_only = Config {
        max_download_mb_per_sec: d.config.max_download_mb_per_sec,
        max_upload_mb_per_sec: d.config.max_upload_mb_per_sec,
        ..old.clone()
    } == d.config;
    if let Some(bandwidth) = &d.bandwidth {
        bandwidth.set_limits(d.config.max_download_mb_per_sec, d.config.max_upload_mb_per_sec);
        if limits_only {
            tracing::info!(
                "speed limits now {} MB/s down, {} MB/s up (0 = unlimited)",
                d.config.max_download_mb_per_sec,
                d.config.max_upload_mb_per_sec
            );
            return Ok(());
        }
    }
    d.stop_engine().await;
    if old.client_id != d.config.client_id || old.client_secret != d.config.client_secret {
        // Refresh tokens are bound to the OAuth client that issued them.
        Token::delete()?;
        Db::open(&config::db_file())?.reset()?;
    } else if old.sync_root != d.config.sync_root {
        // A new folder starts from scratch; the old one is left untouched.
        Db::open(&config::db_file())?.reset()?;
    }
    d.start_engine()
}

async fn start_login(daemon: &Shared, d: &mut Daemon) -> Result<String> {
    if let Some(prev) = d.login.take() {
        prev.abort();
    }
    let pending = auth::begin_login(&d.config).await?;
    let url = pending.url.clone();
    d.set_state(SyncState::SigningIn, Some("Finish signing in in your browser".into()));
    let _ = std::process::Command::new("xdg-open")
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    let daemon = daemon.clone();
    let http = d.http.clone();
    d.login = Some(tokio::spawn(async move {
        let result = tokio::time::timeout(std::time::Duration::from_secs(600), pending.finish(&http)).await;
        let mut d = daemon.lock().await;
        d.login = None;
        match result {
            Ok(Ok(_)) => {
                tracing::info!("signed in");
                d.stop_engine().await;
                if let Err(e) = d.start_engine() {
                    d.set_state(SyncState::Error, Some(format!("{e:#}")));
                }
            }
            Ok(Err(e)) => d.set_state(SyncState::SignedOut, Some(format!("Sign-in failed: {e:#}"))),
            Err(_) => d.set_state(SyncState::SignedOut, Some("Sign-in timed out".into())),
        }
    }));
    Ok(url)
}

async fn serve_client(daemon: Shared, stream: UnixStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await? {
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(&daemon, req).await.unwrap_or_else(|e| Response::Error { message: format!("{e:#}") }),
            Err(e) => Response::Error { message: format!("bad request: {e}") },
        };
        let mut out = serde_json::to_string(&resp)?;
        out.push('\n');
        write.write_all(out.as_bytes()).await?;
    }
    Ok(())
}

fn bind_socket() -> Result<UnixListener> {
    let path = config::socket_path();
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            bail!("gdrived is already running ({})", path.display());
        }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[tokio::main]
async fn main() -> Result<()> {
    gdrive_core::update::mark_inherited_fds_cloexec();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gdrive_core=info,gdrived=info".into()),
        )
        .with_target(false)
        .init();

    let listener = bind_socket()?;
    let config = Config::load()?;
    let status = Arc::new(RwLock::new(Status::new(SyncState::Starting, config.sync_root.display().to_string())));
    let http = reqwest::Client::builder()
        .user_agent(concat!("gdrive-linux/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(120))
        .build()?;
    let check_updates = Arc::new(tokio::sync::Notify::new());
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let daemon: Shared = Arc::new(Mutex::new(Daemon {
        config,
        http: http.clone(),
        status,
        engine: None,
        bandwidth: None,
        login: None,
        check_updates: check_updates.clone(),
        shutdown: shutdown.clone(),
        relocating: false,
    }));
    tokio::spawn(updater::run(daemon.clone(), http, check_updates));
    {
        let mut d = daemon.lock().await;
        if let Err(e) = d.start_engine() {
            tracing::error!("cannot start sync: {e:#}");
            d.set_state(SyncState::Error, Some(format!("{e:#}")));
        }
    }
    tracing::info!("listening on {}", config::socket_path().display());
    match gdrive_core::nautilus::install_extension() {
        Ok(true) => tracing::info!("installed the Files right-click menu; it shows once Files restarts (nautilus -q)"),
        Ok(false) => {}
        Err(e) => tracing::warn!("cannot install the Files right-click menu: {e:#}"),
    }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_client(daemon, stream).await {
                        tracing::debug!("client error: {e:#}");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
            _ = shutdown.notified() => break,
        }
    }
    tracing::info!("shutting down");
    daemon.lock().await.stop_engine().await;
    let _ = std::fs::remove_file(config::socket_path());
    Ok(())
}
