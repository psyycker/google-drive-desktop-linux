//! `gdrive` — command-line control for the gdrive-linux daemon.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gdrive_core::config::Config;
use gdrive_core::gdoc;
use gdrive_core::ipc::{self, Request, Response};
use gdrive_core::status::{Direction, SyncState};

#[derive(Parser)]
#[command(name = "gdrive", version, about = "Control the gdrive-linux sync daemon")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show sync status, transfers and recent activity.
    Status {
        /// Number of recent activity lines to show.
        #[arg(short, long, default_value_t = 10)]
        recent: usize,
    },
    /// Pause syncing.
    Pause,
    /// Resume syncing.
    Resume,
    /// Check Drive for changes now.
    Sync,
    /// Re-read the whole Drive and rescan the sync folder.
    Resync,
    /// Sign in to a Google account (opens the browser).
    Login,
    /// Sign out and forget sync state (local files are kept).
    Logout,
    /// Show or change settings.
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },
    /// Open a .gdoc/.gsheet/… link file in the browser.
    Open { file: PathBuf },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Set the OAuth client ID and secret of a Google Cloud "Desktop app" credential.
    Client { id: String, secret: String },
    /// Set the local folder that mirrors My Drive.
    Folder { path: PathBuf },
    /// Set the Drive polling interval in seconds.
    Poll { seconds: u64 },
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

async fn status(recent: usize) -> Result<()> {
    let Response::Status { status: st } = ipc::call(&Request::Status).await? else { bail!("unexpected reply") };
    let state = match st.state {
        SyncState::SetupRequired => "Setup required (run `gdrive config client <id> <secret>`)",
        SyncState::SignedOut => "Signed out (run `gdrive login`)",
        SyncState::SigningIn => "Waiting for browser sign-in",
        SyncState::Starting => "Starting",
        SyncState::Idle => "Up to date",
        SyncState::Syncing => "Syncing",
        SyncState::Paused => "Paused",
        SyncState::Offline => "Offline",
        SyncState::Error => "Error",
    };
    println!("Status:   {state}");
    if let Some(msg) = &st.message {
        println!("          {msg}");
    }
    if let Some(acc) = &st.account {
        println!("Account:  {} <{}>", acc.display_name, acc.email);
    }
    if let Some(q) = &st.quota {
        match q.limit {
            Some(limit) => println!("Storage:  {} of {} used", human_bytes(q.used), human_bytes(limit)),
            None => println!("Storage:  {} used", human_bytes(q.used)),
        }
    }
    println!("Folder:   {}", st.sync_root);
    if st.pending > 0 {
        println!("Pending:  {} item(s)", st.pending);
    }
    if let Some(t) = st.last_synced {
        println!("Synced:   {}", t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S"));
    }
    if !st.transfers.is_empty() {
        println!("\nTransfers:");
        for t in &st.transfers {
            let arrow = if t.direction == Direction::Upload { "↑" } else { "↓" };
            let pct = (t.bytes_done * 100).checked_div(t.bytes_total).unwrap_or(100);
            println!("  {arrow} {:>3}%  {}", pct, t.path);
        }
    }
    if !st.errors.is_empty() {
        println!("\nErrors:");
        for e in st.errors.iter().take(10) {
            println!("  {}  {}", if e.path.is_empty() { "-" } else { &e.path }, e.message);
        }
    }
    if recent > 0 && !st.recent.is_empty() {
        println!("\nRecent activity:");
        for a in st.recent.iter().take(recent) {
            let when = a.time.with_timezone(&chrono::Local).format("%H:%M:%S");
            let detail = a.detail.as_deref().map(|d| format!("  ({d})")).unwrap_or_default();
            println!("  {when}  {:<22} {}{detail}", format!("{:?}", a.kind), a.path);
        }
    }
    Ok(())
}

async fn update_config(f: impl FnOnce(&mut Config)) -> Result<()> {
    let Response::Config { mut config } = ipc::call(&Request::GetConfig).await? else { bail!("unexpected reply") };
    f(&mut config);
    ipc::call(&Request::SetConfig { config }).await?;
    println!("Saved.");
    Ok(())
}

fn open_link(file: &Path) -> Result<()> {
    let link = gdoc::read(file).with_context(|| format!("{} is not a Google Drive link file", file.display()))?;
    std::process::Command::new("xdg-open").arg(&link.url).spawn()?.wait()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Status { recent } => status(recent).await?,
        Cmd::Pause => drop(ipc::call(&Request::Pause).await?),
        Cmd::Resume => drop(ipc::call(&Request::Resume).await?),
        Cmd::Sync => drop(ipc::call(&Request::SyncNow).await?),
        Cmd::Resync => drop(ipc::call(&Request::FullResync).await?),
        Cmd::Login => {
            if let Response::LoginUrl { url } = ipc::call(&Request::StartLogin).await? {
                println!("Opening your browser to sign in. If it didn't open, visit:\n\n  {url}\n");
                println!("Run `gdrive status` to follow progress.");
            }
        }
        Cmd::Logout => {
            ipc::call(&Request::SignOut).await?;
            println!("Signed out. Your local files were left in place.");
        }
        Cmd::Config { action: None } => {
            let Response::Config { config } = ipc::call(&Request::GetConfig).await? else { bail!("unexpected reply") };
            let mut shown = config.clone();
            if !shown.client_secret.is_empty() {
                shown.client_secret = "********".into();
            }
            print!("{}", toml_display(&shown));
        }
        Cmd::Config { action: Some(ConfigCmd::Client { id, secret }) } => {
            update_config(|c| {
                c.client_id = id;
                c.client_secret = secret;
            })
            .await?
        }
        Cmd::Config { action: Some(ConfigCmd::Folder { path }) } => {
            let path = std::path::absolute(path)?;
            update_config(|c| c.sync_root = path).await?
        }
        Cmd::Config { action: Some(ConfigCmd::Poll { seconds }) } => {
            update_config(|c| c.poll_interval_secs = seconds).await?
        }
        Cmd::Open { file } => open_link(&file)?,
    }
    Ok(())
}

fn toml_display(c: &Config) -> String {
    format!(
        "client_id = {:?}\nclient_secret = {:?}\nsync_root = {:?}\npoll_interval_secs = {}\nmax_concurrent_transfers = {}\nuse_local_trash = {}\nignore = {:?}\n",
        c.client_id, c.client_secret, c.sync_root, c.poll_interval_secs, c.max_concurrent_transfers, c.use_local_trash, c.ignore
    )
}
