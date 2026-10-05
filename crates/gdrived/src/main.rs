//! gdrived — the gdrive-linux sync daemon.
//!
//! Owns the sync engine's lifecycle (sign-in, sign-out, config changes) and
//! serves the JSON IPC protocol on a Unix socket for the tray app and CLI.

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use gdrive_core::auth::{self, Authenticator, Token};
use gdrive_core::config::{self, Config};
use gdrive_core::db::Db;
use gdrive_core::engine::{self, Command, EngineHandle};
use gdrive_core::ipc::{Request, Response};
use gdrive_core::status::{Status, SyncState};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

struct Daemon {
    config: Config,
    http: reqwest::Client,
    status: Arc<RwLock<Status>>,
    engine: Option<EngineHandle>,
    login: Option<tokio::task::JoinHandle<()>>,
}

type Shared = Arc<Mutex<Daemon>>;

impl Daemon {
    fn set_state(&self, state: SyncState, message: Option<String>) {
        let mut st = self.status.write().unwrap();
        st.state = state;
        st.message = message;
    }

    /// Starts the engine if configured and signed in; otherwise reflects why not.
    fn start_engine(&mut self) -> Result<()> {
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
        Request::SetConfig { config } => {
            apply_config(&mut d, config).await?;
            Response::Ok
        }
        Request::StartLogin => {
            let url = start_login(daemon, &mut d).await?;
            Response::LoginUrl { url }
        }
        Request::SignOut => {
            d.stop_engine().await;
            if let Some(login) = d.login.take() {
                login.abort();
            }
            Token::delete()?;
            Db::open(&config::db_file())?.reset()?;
            let root = d.config.sync_root.display().to_string();
            *d.status.write().unwrap() = Status::new(SyncState::SignedOut, root);
            tracing::info!("signed out");
            Response::Ok
        }
    })
}

async fn apply_config(d: &mut Daemon, mut new: Config) -> Result<()> {
    new.client_id = new.client_id.trim().to_owned();
    new.client_secret = new.client_secret.trim().to_owned();
    if !new.sync_root.is_absolute() {
        bail!("the sync folder must be an absolute path");
    }
    if new == d.config {
        return Ok(());
    }
    let old = std::mem::replace(&mut d.config, new);
    d.config.save()?;
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
    let daemon: Shared = Arc::new(Mutex::new(Daemon { config, http, status, engine: None, login: None }));
    {
        let mut d = daemon.lock().await;
        if let Err(e) = d.start_engine() {
            tracing::error!("cannot start sync: {e:#}");
            d.set_state(SyncState::Error, Some(format!("{e:#}")));
        }
    }
    tracing::info!("listening on {}", config::socket_path().display());

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
        }
    }
    tracing::info!("shutting down");
    daemon.lock().await.stop_engine().await;
    let _ = std::fs::remove_file(config::socket_path());
    Ok(())
}
