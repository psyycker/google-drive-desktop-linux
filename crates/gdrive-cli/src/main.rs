//! `gdrive` — command-line control for the gdrive-linux daemon.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gdrive_core::config::{Config, StorageLocation};
use gdrive_core::gdoc;
use gdrive_core::ipc::{self, Coverage, Request, Response};
use gdrive_core::status::{human_bytes, Direction, SyncState, UpdatePhase};

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
    /// Delete everything in the sync folder and download the whole Drive again.
    /// Local changes that were not uploaded yet are lost.
    Redownload {
        /// Confirm that the local files may be deleted.
        #[arg(long)]
        yes: bool,
    },
    /// Check for a new version now (AppImage installs update themselves when idle).
    Update,
    /// Sign in to a Google account (opens the browser).
    Login,
    /// Sign out and forget sync state (local files are kept).
    Logout,
    /// Show or change settings.
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },
    /// Choose which Drive folders are synced (new setups start with none).
    Folders {
        #[command(subcommand)]
        action: Option<FoldersCmd>,
    },
    /// Open a .gdoc/.gsheet/… link file in the browser.
    Open { file: PathBuf },
    /// Move a Drive folder to another drive (see `gdrive config location`), or back.
    Move {
        /// The folder: a path inside the Drive folder, or relative to the Drive root.
        folder: PathBuf,
        /// Name of the storage location, or `main` for the main Drive folder.
        to: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Set the OAuth client ID and secret of a Google Cloud "Desktop app" credential.
    Client { id: String, secret: String },
    /// Set the local folder that mirrors My Drive.
    Folder { path: PathBuf },
    /// Set the Drive polling interval in seconds.
    Poll { seconds: u64 },
    /// Limit total transfer speed in MB/s (0 = unlimited). Applies immediately.
    Limit {
        /// Download limit in MB/s.
        #[arg(long)]
        down: Option<f64>,
        /// Upload limit in MB/s.
        #[arg(long)]
        up: Option<f64>,
    },
    /// Manage other drives that can hold some Drive folders.
    Location {
        #[command(subcommand)]
        action: LocationCmd,
    },
}

#[derive(Subcommand)]
enum FoldersCmd {
    /// List the folders on Drive inside a folder (the Drive root by default) and
    /// whether they are synced.
    Browse {
        /// A path from the Drive root, like `Work/Reports`, or a local path inside the Drive folder.
        folder: Option<PathBuf>,
    },
    /// Start syncing these folders, with everything in them.
    Add {
        /// Paths from the Drive root, like `Work/Reports`, or local paths inside the Drive folder.
        #[arg(required = true)]
        folders: Vec<PathBuf>,
    },
    /// Stop syncing these folders. Their local copies are removed; they stay on Drive.
    Remove {
        /// Paths from the Drive root, like `Work/Reports`, or local paths inside the Drive folder.
        #[arg(required = true)]
        folders: Vec<PathBuf>,
    },
    /// Sync only these folders, replacing the current choice.
    Only {
        /// Paths from the Drive root, like `Work/Reports`, or local paths inside the Drive folder.
        #[arg(required = true)]
        folders: Vec<PathBuf>,
        /// Confirm, when the whole Drive is synced now, that local copies of
        /// everything else may be removed.
        #[arg(long)]
        yes: bool,
    },
    /// Sync the whole Drive.
    All,
}

#[derive(Subcommand)]
enum LocationCmd {
    /// Add a folder on another drive, e.g. `gdrive config location add SSD /mnt/ssd/GDrive`.
    Add { name: String, path: PathBuf },
    /// Remove a storage location (its folders must be moved back first).
    Remove { name: String },
    /// List storage locations and the folders on them.
    List,
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
    match st.selected_folders {
        None => println!("Syncing:  the whole Drive"),
        Some(0) => println!("Syncing:  no folders yet (pick some: `gdrive folders browse`, `gdrive folders add <folder>`)"),
        Some(n) => println!("Syncing:  {n} selected folder(s) (`gdrive folders` lists them)"),
    }
    if !st.version.is_empty() {
        println!("Version:  {}", st.version);
    }
    if let Some(u) = &st.update {
        let what = match u.phase {
            UpdatePhase::Available if u.automatic => "available; installs when syncing is idle".to_owned(),
            UpdatePhase::Available => format!("available: {}", u.release_url),
            UpdatePhase::Downloading => format!("downloading ({} of {})", human_bytes(u.bytes_done), human_bytes(u.bytes_total)),
            UpdatePhase::Installing => "installing".to_owned(),
            UpdatePhase::Failed => format!("failed: {}", u.message.as_deref().unwrap_or("unknown error")),
        };
        println!("Update:   {} {what}", u.latest);
    }
    if let Some(r) = &st.relocation {
        let to = r.to.as_deref().unwrap_or("main drive");
        let pct = (r.bytes_done * 100).checked_div(r.bytes_total).map(|p| format!(" {p}%")).unwrap_or_default();
        println!("Moving:   {} → {to}{pct}", r.folder);
    }
    if st.pending > 0 {
        println!("Pending:  {} item(s)", st.pending);
    }
    if let Some(speed) = st.speed_text() {
        println!("Speed:    {speed}");
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

/// The selected folders' paths, or `None` when the whole Drive is synced.
async fn get_folders() -> Result<Option<Vec<PathBuf>>> {
    let Response::Folders { folders } = ipc::call(&Request::GetFolders).await? else { bail!("unexpected reply") };
    Ok(folders.map(|sel| sel.into_iter().map(|f| f.path).collect()))
}

async fn set_folders(folders: Option<Vec<PathBuf>>) -> Result<()> {
    let what = match &folders {
        None => "the whole Drive".to_owned(),
        Some(f) if f.is_empty() => "no folders".to_owned(),
        Some(f) => format!("{} folder(s)", f.len()),
    };
    ipc::call(&Request::SetFolders { folders }).await?;
    println!("Now syncing {what}; follow along with `gdrive status`.");
    Ok(())
}

/// Folder arguments as paths from the Drive root: a local path inside the Drive folder
/// is converted, anything else is taken as a Drive path already.
async fn drive_paths(folders: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let Response::Config { config } = ipc::call(&Request::GetConfig).await? else { bail!("unexpected reply") };
    folders
        .into_iter()
        .map(|f| {
            let abs = std::path::absolute(&f)?;
            let rel = match abs.strip_prefix(&config.sync_root) {
                Ok(rel) if abs.exists() => rel.to_path_buf(),
                _ => f,
            };
            // Same spelling as the daemon reports, so they compare equal.
            Ok(rel.components().filter(|c| !matches!(c, std::path::Component::CurDir)).collect())
        })
        .collect()
}

async fn folders(action: Option<FoldersCmd>) -> Result<()> {
    let whole_drive = "the whole Drive is synced; to sync only some folders, run `gdrive folders only <folder>…`";
    match action {
        None => match get_folders().await? {
            None => println!("Syncing the whole Drive. To sync only some folders: `gdrive folders only <folder>…`"),
            Some(sel) if sel.is_empty() => {
                println!("No folders are synced yet. See what's on Drive with `gdrive folders browse`,");
                println!("then pick folders with `gdrive folders add <folder>…`.");
            }
            Some(sel) => {
                println!("Syncing these Drive folders:");
                for f in sel {
                    println!("  {}", f.display());
                }
            }
        },
        Some(FoldersCmd::Browse { folder }) => {
            let path = match folder {
                Some(f) => drive_paths(vec![f]).await?.remove(0),
                None => PathBuf::new(),
            };
            let Response::Listing { folders } = ipc::call(&Request::Browse { path: path.clone() }).await? else {
                bail!("unexpected reply")
            };
            let place = if path.as_os_str().is_empty() { "your Drive".to_owned() } else { path.display().to_string() };
            if folders.is_empty() {
                println!("No folders in {place}.");
                return Ok(());
            }
            println!("Folders in {place}  ([x] synced, [~] partly synced):");
            for f in folders {
                let mark = match f.coverage {
                    Coverage::Whole => "[x]",
                    Coverage::Partly => "[~]",
                    Coverage::Unsynced => "[ ]",
                };
                println!("  {mark} {}", f.path.display());
            }
        }
        Some(FoldersCmd::Add { folders }) => {
            let Some(mut sel) = get_folders().await? else { bail!(whole_drive) };
            for f in drive_paths(folders).await? {
                if let Some(parent) = sel.iter().find(|s| f.starts_with(s)) {
                    println!("{} is already synced (as part of {}).", f.display(), parent.display());
                    continue;
                }
                // Folders inside it come along anyway.
                sel.retain(|s| !s.starts_with(&f));
                sel.push(f);
            }
            set_folders(Some(sel)).await?
        }
        Some(FoldersCmd::Remove { folders }) => {
            let Some(mut sel) = get_folders().await? else { bail!(whole_drive) };
            for f in drive_paths(folders).await? {
                if let Some(pos) = sel.iter().position(|s| *s == f) {
                    sel.remove(pos);
                } else if let Some(parent) = sel.iter().find(|s| f.starts_with(s)) {
                    bail!(
                        "{} is synced as part of {}; only whole selected folders can be removed. \
                         Remove {} and add the folders inside it that you want instead.",
                        f.display(),
                        parent.display(),
                        parent.display()
                    );
                } else {
                    bail!("{} isn't synced", f.display());
                }
            }
            set_folders(Some(sel)).await?
        }
        Some(FoldersCmd::Only { folders, yes }) => {
            if get_folders().await?.is_none() && !yes {
                bail!(
                    "the whole Drive is synced now: this removes the local copies of everything else \
                     (it stays on Drive). Run again with --yes to confirm."
                );
            }
            set_folders(Some(drive_paths(folders).await?)).await?
        }
        Some(FoldersCmd::All) => set_folders(None).await?,
    }
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
        Cmd::Redownload { yes } => {
            if !yes {
                bail!("this permanently deletes everything in your sync folder; run `gdrive redownload --yes` to confirm");
            }
            ipc::call(&Request::Redownload).await?;
            println!("Sync folder cleared; downloading everything again.");
        }
        Cmd::Update => {
            ipc::call(&Request::CheckForUpdates).await?;
            println!("Checking for updates… run `gdrive status` in a few seconds to see the result.");
        }
        Cmd::Login => {
            if let Response::LoginUrl { url } = ipc::call(&Request::StartLogin).await? {
                println!("Opening your browser to sign in. If it didn't open, visit:\n\n  {url}\n");
                println!("Signing in from a browser on another computer (e.g. over SSH)? After you allow access,");
                println!("it fails to load a http://127.0.0.1:… page. Copy that page's address and run, on this computer:");
                println!("  curl '<address>'\n");
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
        Cmd::Config { action: Some(ConfigCmd::Limit { down, up }) } => {
            if down.is_none() && up.is_none() {
                bail!("pass --down and/or --up (MB/s, 0 = unlimited)");
            }
            update_config(|c| {
                if let Some(d) = down {
                    c.max_download_mb_per_sec = d;
                }
                if let Some(u) = up {
                    c.max_upload_mb_per_sec = u;
                }
            })
            .await?
        }
        Cmd::Open { file } => open_link(&file)?,
        Cmd::Folders { action } => folders(action).await?,
        Cmd::Config { action: Some(ConfigCmd::Location { action }) } => match action {
            LocationCmd::Add { name, path } => {
                let root = std::path::absolute(path)?;
                update_config(|c| c.locations.push(StorageLocation { name, root })).await?
            }
            LocationCmd::Remove { name } => update_config(|c| c.locations.retain(|l| l.name != name)).await?,
            LocationCmd::List => {
                let Response::Config { config } = ipc::call(&Request::GetConfig).await? else { bail!("unexpected reply") };
                print!("{}", locations_display(&config));
            }
        },
        Cmd::Move { folder, to } => {
            // A path that exists locally is taken as one; anything else is Drive-relative.
            let folder = if folder.exists() { std::path::absolute(folder)? } else { folder };
            let location = (to != "main").then_some(to);
            ipc::call(&Request::Relocate { folder, location }).await?;
            println!("Moving; follow along with `gdrive status`.");
        }
    }
    Ok(())
}

fn locations_display(c: &Config) -> String {
    if c.locations.is_empty() {
        return "No storage locations. Add one with `gdrive config location add <name> <path>`.\n".into();
    }
    let mut out = String::new();
    for l in &c.locations {
        out.push_str(&format!("{}  {}\n", l.name, l.root.display()));
        for p in c.placements.iter().filter(|p| p.location == l.name) {
            out.push_str(&format!("  {}\n", p.folder.display()));
        }
    }
    out
}

fn toml_display(c: &Config) -> String {
    let mut out = format!(
        "client_id = {:?}\nclient_secret = {:?}\nsync_root = {:?}\npoll_interval_secs = {}\nmax_concurrent_transfers = {}\nmax_download_mb_per_sec = {}\nmax_upload_mb_per_sec = {}\nuse_local_trash = {}\nignore = {:?}\n",
        c.client_id,
        c.client_secret,
        c.sync_root,
        c.poll_interval_secs,
        c.max_concurrent_transfers,
        c.max_download_mb_per_sec,
        c.max_upload_mb_per_sec,
        c.use_local_trash,
        c.ignore
    );
    if !c.locations.is_empty() {
        out.push_str("\n[storage locations]\n");
        out.push_str(&locations_display(c));
    }
    out
}
