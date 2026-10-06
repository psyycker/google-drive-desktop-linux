//! Background self-update. Checks GitHub Releases periodically; for AppImage installs,
//! downloads, verifies and installs a newer version once syncing has been idle for a
//! while, then restarts the daemon into it.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use gdrive_core::config;
use gdrive_core::status::{SyncState, UpdateInfo, UpdatePhase};
use gdrive_core::update::{self, Updater, Version};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::Shared;

const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How long syncing must have been idle before an update is installed.
const IDLE_FOR: Duration = Duration::from_secs(120);
const IDLE_POLL: Duration = Duration::from_secs(15);

pub async fn run(daemon: Shared, http: reqwest::Client, check_now: Arc<Notify>) {
    // GDRIVE_UPDATE_API points the checker at another server (for testing).
    let api = std::env::var("GDRIVE_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".into());
    let updater = Updater::with_api_base(http, &api);
    tokio::select! {
        _ = tokio::time::sleep(FIRST_CHECK) => {}
        _ = check_now.notified() => {}
    }
    loop {
        if let Err(e) = cycle(&daemon, &updater).await {
            tracing::warn!("update failed: {e:#}");
            let status = daemon.lock().await.status.clone();
            let mut st = status.write().unwrap();
            if let Some(info) = st.update.as_mut() {
                info.phase = UpdatePhase::Failed;
                info.message = Some(format!("{e:#}"));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(CHECK_EVERY) => {}
            _ = check_now.notified() => {}
        }
    }
}

async fn cycle(daemon: &Shared, updater: &Updater) -> Result<()> {
    let release = updater.latest().await?;
    let Some(latest) = release.version() else { bail!("unrecognised release tag {:?}", release.tag_name) };
    let (status, auto_enabled) = {
        let d = daemon.lock().await;
        (d.status.clone(), d.config.auto_update)
    };
    if latest <= Version::current() {
        tracing::info!("up to date (latest release is {latest})");
        status.write().unwrap().update = None;
        return Ok(());
    }

    let appimage = update::running_appimage();
    let automatic =
        auto_enabled && release.appimage().is_some() && appimage.as_deref().is_some_and(update::can_replace);
    let set = |f: &dyn Fn(&mut UpdateInfo)| {
        if let Some(info) = status.write().unwrap().update.as_mut() {
            f(info);
        }
    };
    status.write().unwrap().update = Some(UpdateInfo {
        latest: latest.to_string(),
        phase: UpdatePhase::Available,
        automatic,
        release_url: release.html_url.clone(),
        message: automatic.then(|| "Will install when syncing is idle".to_owned()),
        bytes_done: 0,
        bytes_total: release.appimage().map_or(0, |a| a.size),
    });
    let Some(appimage) = appimage.filter(|_| automatic) else {
        tracing::info!("version {latest} is available: {}", release.html_url);
        return Ok(());
    };

    tracing::info!("version {latest} is available; installing once syncing is idle");
    if !wait_for_idle(daemon).await {
        set(&|i| {
            i.automatic = false;
            i.message = None;
        });
        return Ok(());
    }

    set(&|i| {
        i.phase = UpdatePhase::Downloading;
        i.message = None;
    });
    let file_name = appimage.file_name().unwrap_or_default().to_string_lossy();
    let staged = appimage.with_file_name(format!(".{file_name}.update"));
    let progress = Arc::new(AtomicU64::new(0));
    let download = updater.download_appimage(&release, &staged, &progress);
    tokio::pin!(download);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            result = &mut download => { result?; break; }
            _ = tick.tick() => {
                let done = progress.load(Ordering::Relaxed);
                set(&|i| i.bytes_done = done);
            }
        }
    }

    if let Err(e) = update::sanity_check(&staged, &latest).await {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    set(&|i| {
        i.phase = UpdatePhase::Installing;
        i.message = Some("Restarting into the new version…".into());
    });
    update::install(&staged, &appimage)?;
    tracing::info!("installed {latest} at {}", appimage.display());
    restart(daemon, &appimage).await
}

/// Waits until syncing has been idle for `IDLE_FOR`. Returns false if automatic
/// updates get turned off meanwhile.
async fn wait_for_idle(daemon: &Shared) -> bool {
    let mut idle_since: Option<Instant> = None;
    loop {
        {
            let d = daemon.lock().await;
            if !d.config.auto_update {
                return false;
            }
            let st = d.status.read().unwrap();
            // Paused, offline or failing states are not "idle": a restart would undo
            // a pause, and there's no point updating while things are broken.
            let quiet = matches!(st.state, SyncState::Idle | SyncState::SignedOut | SyncState::SetupRequired)
                && st.pending == 0
                && st.transfers.is_empty();
            if quiet {
                if idle_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_FOR {
                    return true;
                }
            } else {
                idle_since = None;
            }
        }
        tokio::time::sleep(IDLE_POLL).await;
    }
}

/// Stops syncing cleanly and replaces this process with the newly installed version.
async fn restart(daemon: &Shared, appimage: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut d = daemon.lock().await;
    d.stop_engine().await;
    if let Some(login) = d.login.take() {
        login.abort();
    }
    let _ = std::fs::remove_file(config::socket_path());
    // A fresh `--daemon` invocation gets its own AppImage mount of the new file.
    let mut cmd = std::process::Command::new(appimage);
    update::clean_appimage_launch(&mut cmd);
    cmd.arg("--daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .context("starting the updated daemon")?;
    tracing::info!("restarting into the new version");
    std::process::exit(0);
}
