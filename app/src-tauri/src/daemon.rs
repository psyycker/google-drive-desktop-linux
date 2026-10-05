//! Talking to (and, if needed, starting) the `gdrived` daemon.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use gdrive_core::config::{self, Config};
use gdrive_core::ipc::{self, Request, Response};
use gdrive_core::status::Status;

/// Delay before relaunching the daemon after a launch attempt. It doubles with every
/// consecutive attempt (so a daemon that crashes on start isn't relaunched every
/// poll) and resets as soon as the daemon answers.
const RESPAWN_MIN: Duration = Duration::from_secs(3);
const RESPAWN_MAX: Duration = Duration::from_secs(60);

struct SpawnState {
    last: Instant,
    attempts: u32,
}

static SPAWN: Mutex<Option<SpawnState>> = Mutex::new(None);

/// Sends one request; `Response::Error` becomes `Err`.
pub async fn call(req: Request) -> Result<Response> {
    ipc::call(&req).await
}

pub async fn status() -> Result<Status> {
    match call(Request::Status).await? {
        Response::Status { status } => Ok(*status),
        other => Err(anyhow!("unexpected reply from gdrived: {other:?}")),
    }
}

/// Fetches the status; if the daemon is unreachable, tries to launch it (subject to a
/// backoff) and waits a few seconds for its socket to come up.
pub async fn status_or_spawn() -> Result<Status> {
    let err = match status().await {
        Ok(s) => {
            *SPAWN.lock().unwrap() = None;
            return Ok(s);
        }
        Err(e) => e,
    };
    if !try_spawn() {
        return Err(err);
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        match status().await {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => {}
        }
    }
}

/// Launches `gdrived` detached. Returns whether a process was started.
fn try_spawn() -> bool {
    {
        let mut state = SPAWN.lock().unwrap();
        let attempts = state.as_ref().map_or(0, |s| s.attempts);
        if let Some(s) = state.as_ref() {
            let wait = (RESPAWN_MIN * 2u32.saturating_pow(s.attempts.saturating_sub(1))).min(RESPAWN_MAX);
            if s.last.elapsed() < wait {
                return false;
            }
        }
        *state = Some(SpawnState { last: Instant::now(), attempts: attempts + 1 });
    }
    let mut cmd = if let Some(appimage) = std::env::var_os("APPIMAGE") {
        // Never run the daemon straight out of our AppImage mount: it disappears when
        // the app quits. A separate `--daemon` invocation gets a mount of its own.
        tracing::info!("starting daemon via {}", appimage.to_string_lossy());
        let mut cmd = Command::new(appimage);
        cmd.arg("--daemon");
        cmd
    } else {
        let Some(exe) = find_daemon() else {
            tracing::warn!("gdrived is not running and was not found next to the app or on PATH");
            return false;
        };
        tracing::info!("starting {}", exe.display());
        Command::new(exe)
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        // Own process group: not affected by signals sent to the app's group.
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            // Reap it if it exits while we are still running (no zombie). If the app
            // exits first the daemon is simply re-parented and keeps going.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(e) => {
            tracing::warn!("failed to start the daemon: {e}");
            false
        }
    }
}

fn find_daemon() -> Option<PathBuf> {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
        let candidate = dir.join("gdrived");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("gdrived"))
        .find(|p| p.is_file())
}

/// Best-effort guess, without the daemon, whether onboarding is needed.
pub fn local_setup_needed() -> bool {
    let configured = Config::load().map(|c| c.is_complete()).unwrap_or(false);
    !configured || !config::token_file().exists()
}
