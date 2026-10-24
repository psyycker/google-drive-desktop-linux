//! `gdrive setup`: everything needed to go from a fresh install to syncing, one step at
//! a time. Steps that are already done are shown and skipped, so it can be run again
//! at any point; flags answer the questions for scripted installs.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use gdrive_core::config::Config;
use gdrive_core::ipc::{self, Request, Response};
use gdrive_core::status::{Status, SyncState};
use tokio::sync::mpsc;

#[derive(Args, Default)]
pub struct SetupArgs {
    /// OAuth client ID (from the Google Cloud console).
    #[arg(long, requires = "client_secret")]
    client_id: Option<String>,
    /// OAuth client secret.
    #[arg(long, requires = "client_id")]
    client_secret: Option<String>,
    /// The client JSON downloaded from the Google Cloud console, instead of the ID and secret.
    #[arg(long, conflicts_with = "client_id")]
    client_json: Option<PathBuf>,
    /// Local folder to sync into (default ~/GoogleDrive).
    #[arg(long)]
    local_folder: Option<PathBuf>,
    /// A Drive folder to sync, like `Work/Reports`; repeat for more.
    #[arg(long = "sync", value_name = "FOLDER", conflicts_with = "whole_drive")]
    sync: Vec<PathBuf>,
    /// Sync the whole Drive.
    #[arg(long)]
    whole_drive: bool,
    /// Accept the default answer to every question (needs the client from flags or
    /// already configured).
    #[arg(short, long)]
    yes: bool,
}

const STEPS: usize = 6;
const CLIENT_ID_SUFFIX: &str = ".apps.googleusercontent.com";

/// Questions and answers. Standard input is read on one thread for the whole run, so a
/// question asked while waiting for something else (the browser sign-in) doesn't leave
/// a pending read that would swallow the next answer.
struct Ui {
    lines: mpsc::UnboundedReceiver<String>,
    interactive: bool,
    yes: bool,
}

impl Ui {
    fn new(yes: bool) -> Self {
        let (tx, lines) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { lines, interactive: std::io::stdin().is_terminal(), yes }
    }

    fn step(&self, n: usize, title: &str) {
        println!("\n[{n}/{STEPS}] {title}");
    }

    /// Reads an answer; `None` when there is no one to ask (no terminal, or `--yes`).
    async fn ask(&mut self, question: &str) -> Result<Option<String>> {
        if self.yes || !self.interactive {
            return Ok(None);
        }
        print!("  {question} ");
        std::io::stdout().flush()?;
        let line = self.lines.recv().await.ok_or_else(|| anyhow!("no more input"))?;
        Ok(Some(line.trim().to_owned()))
    }

    /// A yes/no question; Enter (and `--yes`) gives `default`.
    async fn confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        loop {
            let Some(answer) = self.ask(&format!("{question} {hint}")).await? else { return Ok(default) };
            match answer.to_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => println!("  Please answer y or n."),
            }
        }
    }
}

pub async fn run(args: SetupArgs) -> Result<()> {
    let mut ui = Ui::new(args.yes);
    println!("Setting up Google Drive sync. Finished steps are skipped, so this can be run again any time.");

    ui.step(1, "Background service");
    ensure_daemon(&mut ui).await?;

    ui.step(2, "Google Cloud OAuth client");
    let mut config = get_config().await?;
    let fresh = !config.is_complete();
    set_up_client(&mut ui, &args, &mut config).await?;

    ui.step(3, "Local folder");
    choose_local_folder(&mut ui, &args, fresh).await?;

    ui.step(4, "Sign in to Google");
    sign_in(&mut ui).await?;

    ui.step(5, "Drive access");
    check_drive(&mut ui).await?;

    ui.step(6, "Folders to sync");
    choose_folders(&mut ui, &args).await?;

    let st = status().await?;
    if st.selected_folders == Some(0) {
        println!("\nAll set, but nothing syncs until you choose folders:");
        println!("  gdrive folders browse          see what's in your Drive");
        println!("  gdrive folders add <folder>    sync one into {}", st.sync_root);
    } else {
        println!("\nAll set: syncing into {}. It keeps running in the background.", st.sync_root);
        println!("  gdrive status     follow progress");
        println!("  gdrive folders    change which folders sync");
    }
    Ok(())
}

// ------------------------------------------------------------------ 1. service

async fn reachable() -> bool {
    ipc::request(&Request::Status).await.is_ok()
}

fn systemctl(args: &[&str]) -> Result<bool> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running systemctl")?;
    Ok(status.success())
}

/// How to run the daemon: the AppImage itself with `--daemon`, or `gdrived` next to
/// this program or on PATH.
fn daemon_command() -> Result<Vec<String>> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Ok(vec![appimage.to_string_lossy().into_owned(), "--daemon".into()]);
    }
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("gdrived")));
    let on_path = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).map(|d| d.join("gdrived")).find(|p| p.is_file()));
    let exe = beside.filter(|p| p.is_file()).or(on_path).ok_or_else(|| {
        anyhow!("gdrived was not found next to gdrive or on PATH; install the gdrive-linux-cli package, or put gdrived next to gdrive")
    })?;
    Ok(vec![std::path::absolute(exe)?.to_string_lossy().into_owned()])
}

/// A systemd `ExecStart=` word, quoted when needed.
fn unit_quote(word: &str) -> String {
    if word.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        word.to_owned()
    } else {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Writes a user unit for installs that don't ship one (tarball, AppImage).
fn install_unit() -> Result<PathBuf> {
    let exec = daemon_command()?.iter().map(|w| unit_quote(w)).collect::<Vec<_>>().join(" ");
    let dir = gdrive_core::config::config_dir().parent().unwrap_or(Path::new(".")).join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("gdrived.service");
    let unit = format!(
        "[Unit]\n\
         Description=gdrive-linux Google Drive sync daemon\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         KillSignal=SIGTERM\n\
         TimeoutStopSec=20\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    std::fs::write(&path, unit).with_context(|| format!("writing {}", path.display()))?;
    systemctl(&["daemon-reload"])?;
    Ok(path)
}

/// No systemd user session (some containers, WSL): run it in the background for now.
fn spawn_detached() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let cmd = daemon_command()?;
    Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("starting gdrived")?;
    Ok(())
}

async fn wait_until_reachable() -> Result<()> {
    for _ in 0..40 {
        if reachable().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("gdrived didn't start; see `journalctl --user -u gdrived` for why")
}

async fn ensure_daemon(ui: &mut Ui) -> Result<()> {
    let has_systemd = systemctl(&["show-environment"]).unwrap_or(false);
    if !has_systemd {
        if !reachable().await {
            spawn_detached()?;
            wait_until_reachable().await?;
            println!("  Started gdrived. There is no systemd user session here, so it won't start again");
            println!("  by itself after a reboot: run `gdrived &` (or `gdrive setup`) then.");
        } else {
            println!("  ✓ gdrived is running");
        }
        return Ok(());
    }

    if !systemctl(&["cat", "gdrived.service"])? {
        let path = install_unit()?;
        println!("  Installed the service as {}", path.display());
    }
    if systemctl(&["is-enabled", "gdrived.service"])? {
        if !systemctl(&["is-active", "gdrived.service"])? && !reachable().await {
            systemctl(&["start", "gdrived.service"])?;
        }
    } else if ui.confirm("Start syncing automatically when you log in?", true).await? {
        if !systemctl(&["enable", "--now", "gdrived.service"])? {
            bail!("couldn't enable the service; try `systemctl --user enable --now gdrived`");
        }
    } else if !reachable().await {
        systemctl(&["start", "gdrived.service"])?;
    }
    wait_until_reachable().await?;
    println!("  ✓ gdrived is running");

    // Without lingering, the user's services stop when their last session ends: on a
    // server that is as soon as the SSH session closes.
    let headless = std::env::var_os("SSH_CONNECTION").is_some()
        || (std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none());
    if headless && !lingering() && ui.confirm("Keep syncing while you're logged out?", true).await? {
        let ok = Command::new("loginctl").arg("enable-linger").status().is_ok_and(|s| s.success());
        if ok {
            println!("  ✓ Syncing continues after you log out");
        } else {
            println!("  Couldn't turn that on; run: sudo loginctl enable-linger {}", user_name());
        }
    }
    Ok(())
}

fn user_name() -> String {
    std::env::var("USER").unwrap_or_else(|_| "$USER".into())
}

fn lingering() -> bool {
    Command::new("loginctl")
        .args(["show-user", &user_name(), "--property=Linger", "--value"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
}

// ------------------------------------------------------------------ 2. OAuth client

async fn get_config() -> Result<Config> {
    let Response::Config { config } = ipc::call(&Request::GetConfig).await? else { bail!("unexpected reply") };
    Ok(config)
}

async fn status() -> Result<Status> {
    let Response::Status { status } = ipc::call(&Request::Status).await? else { bail!("unexpected reply") };
    Ok(*status)
}

/// The client ID and secret from a client JSON downloaded from the Google Cloud console.
fn read_client_json(path: &Path) -> Result<(String, String)> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("{} is not JSON", path.display()))?;
    let client = json.get("installed").or_else(|| json.get("web")).unwrap_or(&json);
    let field = |k: &str| client.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    match (field("client_id"), field("client_secret")) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        _ => bail!("{} has no client_id and client_secret; download the JSON of a \"Desktop app\" client", path.display()),
    }
}

/// The newest `client_secret_*.json` in ~/Downloads, as the console names them.
fn downloaded_client_json() -> Option<PathBuf> {
    let dir = dirs_downloads()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("client_secret_") && name.ends_with(".json")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

fn dirs_downloads() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    // XDG user dirs may rename it; the default is enough to find a fresh download.
    Some(home.join("Downloads")).filter(|d| d.is_dir())
}

/// A Cloud console page, opened in `project` when it is known (otherwise the console
/// uses whichever project was selected last, which may not be the right one).
fn console(page: &str, project: Option<&str>) -> String {
    let url = format!("https://console.cloud.google.com/{page}");
    match project {
        Some(p) => format!("{url}{}project={p}", if url.contains('?') { '&' } else { '?' }),
        None => url,
    }
}

/// The console's project IDs: 6 to 30 lowercase letters, digits and hyphens.
fn is_project_id(s: &str) -> bool {
    (6..=30).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && !s.ends_with('-')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The project a client belongs to: its ID starts with the project number, which the
/// console accepts in place of the project ID.
fn project_of(client_id: &str) -> Option<&str> {
    let number = client_id.split('-').next()?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some(number)
}

/// Shows one step of the console walkthrough and waits for Enter.
async fn guide_step(ui: &mut Ui, n: usize, title: &str, url: &str, lines: &[&str]) -> Result<()> {
    println!("\n  {n}/5  {title}");
    println!("        {url}");
    for line in lines {
        println!("        {line}");
    }
    ui.ask("Press Enter when done.").await?;
    Ok(())
}

/// Walks through creating the Google Cloud project and its OAuth client.
async fn console_walkthrough(ui: &mut Ui) -> Result<()> {
    println!("  Google only lets apps reach Drive through an OAuth client. You make your own, for free, in");
    println!("  the Google Cloud console; it takes about 5 minutes, once. Open each link signed in with the");
    println!("  Google account whose Drive you want to sync.");
    guide_step(ui, 1, "Create a project", &console("projectcreate", None), &[
        "Project name: anything, e.g. gdrive-linux. Organization: leave as it is. Click Create.",
    ])
    .await?;
    let project = loop {
        let answer = ui.ask("Its project ID (shown on the dashboard, like gdrive-linux-471210), or Enter to skip:").await?;
        let answer = answer.unwrap_or_default();
        if answer.is_empty() || is_project_id(&answer) {
            break (!answer.is_empty()).then_some(answer);
        }
        println!("  Project IDs are 6–30 lowercase letters, digits and hyphens.");
    };
    if project.is_none() {
        println!("  Then make sure the new project is the one selected at the top of each page.");
    }
    let p = project.as_deref();
    guide_step(ui, 2, "Turn on the Google Drive API", &console("apis/library/drive.googleapis.com", p), &[
        "Click Enable.",
    ])
    .await?;
    guide_step(ui, 3, "Set up the consent screen", &console("auth/overview", p), &[
        "Click Get started. App name: gdrive-linux. User support email: your address. Next.",
        "Audience: External (Internal if it's a Google Workspace account; then skip step 4). Next.",
        "Contact email: your address. Next. Agree to the policy, then Create.",
    ])
    .await?;
    guide_step(ui, 4, "Publish the app", &console("auth/audience", p), &[
        "Under Publishing status, click Publish app, then Confirm.",
        "This matters: while it says Testing, Google signs you out every 7 days. It doesn't need",
        "Google's review for your own use; you'll just be warned once when you sign in.",
    ])
    .await?;
    guide_step(ui, 5, "Create the OAuth client", &console("auth/clients/create", p), &[
        "Application type: Desktop app. Name: anything. Create.",
        "In the window that opens, click Download JSON (it goes to ~/Downloads), or keep it open",
        "to copy the Client ID and Client secret.",
    ])
    .await?;
    Ok(())
}

async fn set_up_client(ui: &mut Ui, args: &SetupArgs, config: &mut Config) -> Result<()> {
    let given = match (&args.client_json, &args.client_id, &args.client_secret) {
        (Some(json), _, _) => Some(read_client_json(json)?),
        (None, Some(id), Some(secret)) => Some((id.clone(), secret.clone())),
        _ => None,
    };
    let (id, secret) = match given {
        Some(pair) => pair,
        None if config.is_complete() => {
            println!("  ✓ Using client {}", short_id(&config.client_id));
            return Ok(());
        }
        None => {
            if !ui.interactive || ui.yes {
                bail!("no OAuth client yet: pass --client-json FILE, or --client-id and --client-secret");
            }
            if !ui.confirm("Do you already have an OAuth client of type \"Desktop app\"?", false).await? {
                console_walkthrough(ui).await?;
            }
            ask_client(ui).await?
        }
    };
    if !id.ends_with(CLIENT_ID_SUFFIX) {
        bail!("\"{id}\" doesn't look like an OAuth client ID (they end in {CLIENT_ID_SUFFIX})");
    }
    config.client_id = id;
    config.client_secret = secret;
    ipc::call(&Request::SetConfig { config: config.clone() }).await?;
    println!("  ✓ Saved client {}", short_id(&config.client_id));
    Ok(())
}

async fn ask_client(ui: &mut Ui) -> Result<(String, String)> {
    if let Some(found) = downloaded_client_json() {
        if ui.confirm(&format!("Use {}?", found.display()), true).await? {
            return read_client_json(&found);
        }
    }
    loop {
        let answer = ui.ask("Client ID (or the path of the downloaded JSON):").await?.unwrap_or_default();
        let answer = answer.trim_matches(|c| c == '\'' || c == '"');
        if answer.is_empty() {
            continue;
        }
        let path = PathBuf::from(answer.replacen('~', &std::env::var("HOME").unwrap_or_default(), 1));
        if path.is_file() {
            match read_client_json(&path) {
                Ok(pair) => return Ok(pair),
                Err(e) => {
                    println!("  {e:#}");
                    continue;
                }
            }
        }
        if !answer.ends_with(CLIENT_ID_SUFFIX) {
            println!("  That doesn't look like a client ID; they end in {CLIENT_ID_SUFFIX}.");
            continue;
        }
        let secret = loop {
            let s = ui.ask("Client secret:").await?.unwrap_or_default();
            if !s.is_empty() {
                break s;
            }
        };
        return Ok((answer.to_owned(), secret));
    }
}

fn short_id(id: &str) -> String {
    let head = id.strip_suffix(CLIENT_ID_SUFFIX).unwrap_or(id);
    match head.char_indices().nth(12) {
        Some((i, _)) => format!("{}…", &head[..i]),
        None => head.to_owned(),
    }
}

// ------------------------------------------------------------------ 3. local folder

fn is_signed_in(st: &Status) -> bool {
    !matches!(st.state, SyncState::SetupRequired | SyncState::SignedOut | SyncState::SigningIn)
}

fn expand_home(answer: &str) -> PathBuf {
    match answer.strip_prefix('~') {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest.trim_start_matches('/')),
        None => PathBuf::from(answer),
    }
}

async fn choose_local_folder(ui: &mut Ui, args: &SetupArgs, fresh: bool) -> Result<()> {
    let mut config = get_config().await?;
    let wanted = match &args.local_folder {
        Some(path) => Some(std::path::absolute(path)?),
        // Changing it later starts the sync over, so it is only asked on a first setup.
        None if fresh || !is_signed_in(&status().await?) => {
            let current = config.sync_root.display().to_string();
            match ui.ask(&format!("Keep your Drive files in {current}? [Enter, or type another folder]")).await? {
                Some(answer) if !answer.is_empty() => Some(std::path::absolute(expand_home(&answer))?),
                _ => None,
            }
        }
        None => None,
    };
    if let Some(path) = wanted.filter(|p| *p != config.sync_root) {
        config.sync_root = path;
        ipc::call(&Request::SetConfig { config: config.clone() }).await?;
    }
    println!("  ✓ Files go in {}", config.sync_root.display());
    Ok(())
}

// ------------------------------------------------------------------ 4. sign in

/// Finishes a sign-in whose browser couldn't reach this machine: the user pastes the
/// address the browser was sent back to, and it is requested here instead.
fn deliver_redirect(url: &str) -> Result<()> {
    use std::io::Read;
    let rest = url
        .trim()
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| url.trim().strip_prefix("http://localhost:"))
        .ok_or_else(|| anyhow!("that isn't the http://127.0.0.1:… address the browser was sent to"))?;
    let (port, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let port: u16 = port.parse().map_err(|_| anyhow!("no port in that address"))?;
    let path = if path.is_empty() { "/" } else { path };
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .context("nothing is waiting at that address; restart the sign-in")?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")?;
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    // 400 means it arrived but carried Google's refusal; the daemon's status says why.
    if !matches!(reply.split_whitespace().nth(1), Some("200" | "400")) {
        bail!("the sign-in didn't accept that address; copy the whole address, including everything after ?");
    }
    Ok(())
}

/// What to do about a failed sign-in, from the daemon's message.
fn sign_in_hint(message: &str, project: Option<&str>) -> Option<String> {
    if message.contains("access_denied") {
        return Some(format!(
            "Google refused access, or Allow wasn't clicked. If the page said \"Access blocked\" or \"has not\n  \
             completed the Google verification process\", the app is still in Testing: publish it at\n  {}\n  \
             (or add your account under Test users there).",
            console("auth/audience", project)
        ));
    }
    if message.contains("invalid_client") || message.contains("unauthorized_client") {
        return Some(
            "Google doesn't recognise that client ID and secret. Check them, or download the client's JSON\n  \
             again and run `gdrive setup --client-json <file>`."
                .into(),
        );
    }
    if message.contains("timed out") {
        return Some("The browser didn't come back within 10 minutes.".into());
    }
    None
}

/// One sign-in attempt; Err carries the daemon's reason.
async fn sign_in_once(ui: &mut Ui) -> Result<std::result::Result<(), String>> {
    let Response::LoginUrl { url } = ipc::call(&Request::StartLogin).await? else { bail!("unexpected reply") };
    let remote = std::env::var_os("SSH_CONNECTION").is_some();
    println!("  Open this address in a browser{}:", if remote { "" } else { " (it may have opened already)" });
    println!("\n    {url}\n");
    println!("  Then, in the browser:");
    println!("   - choose the Google account whose Drive you want to sync;");
    println!("   - \"Google hasn't verified this app\": it's the app you just made. Click Advanced, then");
    println!("     \"Go to gdrive-linux (unsafe)\";");
    println!("   - allow access to your Google Drive files (tick the box if there is one), then Continue.");
    println!("  If the browser is on another computer, the last page (http://127.0.0.1:…) won't load:");
    println!("  copy that page's address and paste it here. Otherwise just wait.");
    print!("  > ");
    std::io::stdout().flush()?;

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            line = ui.lines.recv() => {
                let line = line.ok_or_else(|| anyhow!("no more input"))?;
                if line.trim().is_empty() {
                    continue;
                }
                match tokio::task::spawn_blocking(move || deliver_redirect(&line)).await? {
                    Ok(()) => println!("  Finishing sign-in…"),
                    Err(e) => {
                        println!("  {e:#}");
                        print!("  > ");
                        std::io::stdout().flush()?;
                    }
                }
            }
            _ = tick.tick() => {
                let st = status().await?;
                match st.state {
                    SyncState::SigningIn => {}
                    SyncState::SignedOut | SyncState::SetupRequired => {
                        println!();
                        return Ok(Err(st.message.unwrap_or_else(|| "sign-in didn't finish".into())));
                    }
                    _ => {
                        println!();
                        let who = match st.account { Some(a) => format!(" as {}", a.email), None => String::new() };
                        println!("  ✓ Signed in{who}");
                        return Ok(Ok(()));
                    }
                }
            }
        }
    }
}

async fn sign_in(ui: &mut Ui) -> Result<()> {
    let st = status().await?;
    if is_signed_in(&st) {
        match &st.account {
            Some(a) => println!("  ✓ Signed in as {} <{}>", a.display_name, a.email),
            None => println!("  ✓ Signed in"),
        }
        return Ok(());
    }
    if !ui.interactive {
        bail!("signing in needs a browser; run `gdrive setup` in a terminal, or `gdrive login`");
    }
    let client_id = get_config().await?.client_id;
    loop {
        let Err(reason) = sign_in_once(ui).await? else { return Ok(()) };
        println!("  Sign-in failed: {}", reason.trim_start_matches("Sign-in failed: "));
        if let Some(hint) = sign_in_hint(&reason, project_of(&client_id)) {
            println!("  {hint}");
        }
        if !ui.confirm("Try again?", true).await? {
            bail!("not signed in; run `gdrive setup` again when ready");
        }
    }
}

// ------------------------------------------------------------------ Drive access

/// What to do when the Drive can't be read, from the daemon's error.
fn drive_error_hint(message: &str, project: Option<&str>) -> Option<String> {
    if message.contains("has not been used") || message.contains("is disabled") || message.contains("accessNotConfigured") {
        return Some(format!(
            "The Google Drive API is off in your Cloud project. Turn it on (click Enable) at\n  {}\n  \
             It can take a minute to apply.",
            console("apis/library/drive.googleapis.com", project)
        ));
    }
    if message.contains("insufficient") || message.contains("SCOPE") {
        return Some(
            "Access to your Drive files wasn't allowed during sign-in. Sign in again and tick the box:\n  \
             gdrive logout && gdrive setup"
                .into(),
        );
    }
    None
}

/// Waits until the daemon has read the Drive (or reports why it can't), so a broken
/// Cloud project is caught here rather than after setup claims everything is done.
async fn check_drive(ui: &mut Ui) -> Result<()> {
    let project = get_config().await?.client_id;
    let project = project_of(&project).map(str::to_owned);
    let mut said = false;
    loop {
        match list(Path::new("")).await {
            Ok(_) => {
                println!("  ✓ Your Drive can be read");
                return Ok(());
            }
            Err(e) if format!("{e:#}").contains("hasn't been read yet") => {}
            Err(e) => return Err(e),
        }
        let st = status().await?;
        if st.state == SyncState::Error {
            let message = st.message.unwrap_or_default();
            println!("  Can't read your Drive: {message}");
            if let Some(hint) = drive_error_hint(&message, project.as_deref()) {
                println!("  {hint}");
            }
            if !ui.interactive || ui.yes || !ui.confirm("Fixed it? Check again", true).await? {
                bail!("your Drive can't be read yet; run `gdrive setup` again once it's fixed");
            }
            // Any command cuts the daemon's retry wait short.
            ipc::call(&Request::SyncNow).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            said = false;
            continue;
        }
        if !said {
            println!("  Reading your Drive (only names, nothing is downloaded yet)…");
            said = true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ------------------------------------------------------------------ 5. folders

async fn list(path: &Path) -> Result<Vec<PathBuf>> {
    let Response::Listing { folders } = ipc::call(&Request::Browse { path: path.to_owned() }).await? else {
        bail!("unexpected reply")
    };
    Ok(folders.into_iter().map(|f| f.path).collect())
}

async fn choose_folders(ui: &mut Ui, args: &SetupArgs) -> Result<()> {
    let flagged = if args.whole_drive {
        Some(None)
    } else if !args.sync.is_empty() {
        Some(Some(args.sync.clone()))
    } else {
        None
    };
    let current = status().await?.selected_folders;
    if flagged.is_none() {
        let changing = match current {
            None => {
                println!("  ✓ Syncing your whole Drive");
                ui.confirm("Sync only some folders instead?", false).await?
            }
            Some(0) => true,
            Some(n) => {
                println!("  ✓ Syncing {n} chosen folder(s) (`gdrive folders` lists them)");
                ui.confirm("Change them?", false).await?
            }
        };
        if !changing {
            return Ok(());
        }
    }
    let top = list(Path::new("")).await?;
    let choice = match flagged {
        Some(choice) => choice,
        None if !ui.interactive || ui.yes => {
            println!("  No folders chosen yet: pick some later with `gdrive folders add <folder>`.");
            return Ok(());
        }
        None => pick_folders(ui, top).await?,
    };
    let summary = match &choice {
        None => "your whole Drive".to_owned(),
        Some(f) if f.is_empty() => "no folders for now".to_owned(),
        Some(f) => f.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
    };
    ipc::call(&Request::SetFolders { folders: choice }).await?;
    println!("  ✓ Syncing {summary}");
    Ok(())
}

/// A small folder browser: numbers or paths choose, `N/` looks inside, `..` goes back.
/// Returns `None` for the whole Drive.
async fn pick_folders(ui: &mut Ui, top: Vec<PathBuf>) -> Result<Option<Vec<PathBuf>>> {
    let mut chosen: Vec<PathBuf> = Vec::new();
    let mut here = PathBuf::new();
    let mut shown = top;
    println!("  Choose what to keep on this computer; everything stays on Drive either way.");
    println!("  Type numbers or paths (e.g. `2 Work/Reports`) to choose, `2/` to look inside a folder,");
    println!("  `..` to go back, `all` for your whole Drive. Press Enter when done.");
    loop {
        let place = if here.as_os_str().is_empty() { "your Drive".to_owned() } else { here.display().to_string() };
        if shown.is_empty() {
            println!("\n  No folders in {place}.");
        } else {
            println!("\n  Folders in {place}:");
            for (i, f) in shown.iter().enumerate() {
                let mark = if chosen.iter().any(|c| f.starts_with(c)) { "x" } else { " " };
                println!("   [{mark}] {:>2}. {}", i + 1, f.file_name().unwrap_or_default().to_string_lossy());
            }
        }
        if !chosen.is_empty() {
            let names: Vec<String> = chosen.iter().map(|c| c.display().to_string()).collect();
            println!("  Chosen: {}", names.join(", "));
        }
        let answer = ui.ask(">").await?.unwrap_or_default();
        match answer.as_str() {
            "" if chosen.is_empty() => {
                if ui.confirm("Nothing chosen. Leave it for later?", true).await? {
                    return Ok(Some(chosen));
                }
                continue;
            }
            "" => return Ok(Some(chosen)),
            "all" => return Ok(None),
            ".." => {
                here.pop();
                shown = list(&here).await?;
                continue;
            }
            _ => {}
        }
        for token in answer.split_whitespace() {
            let (number, inside) = match token.strip_suffix('/') {
                Some(n) => (n, true),
                None => (token, false),
            };
            let target = match number.parse::<usize>() {
                Ok(n) if (1..=shown.len()).contains(&n) => shown[n - 1].clone(),
                Ok(_) => {
                    println!("  There's no folder {number} here.");
                    continue;
                }
                Err(_) => PathBuf::from(number.trim_matches('/')),
            };
            if inside {
                match list(&target).await {
                    Ok(sub) => {
                        here = target;
                        shown = sub;
                    }
                    Err(e) => println!("  {e:#}"),
                }
            } else if let Some(pos) = chosen.iter().position(|c| *c == target) {
                chosen.remove(pos);
            } else if let Some(parent) = chosen.iter().find(|c| target.starts_with(c)) {
                println!("  {} is already included with {}.", target.display(), parent.display());
            } else if let Err(e) = list(&target).await {
                // Checks the folder exists before choosing it.
                println!("  {e:#}");
            } else {
                chosen.retain(|c| !c.starts_with(&target));
                chosen.push(target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_consoles_client_json() {
        let dir = std::env::temp_dir().join(format!("gdrive-setup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("client_secret_1.json");
        std::fs::write(&file, r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","client_secret":"GOCSPX-x","redirect_uris":["http://localhost"]}}"#).unwrap();
        let (id, secret) = read_client_json(&file).unwrap();
        assert_eq!((id.as_str(), secret.as_str()), ("123-abc.apps.googleusercontent.com", "GOCSPX-x"));
        std::fs::write(&file, r#"{"installed":{"client_id":"x"}}"#).unwrap();
        assert!(read_client_json(&file).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn console_links_point_at_the_project() {
        assert_eq!(console("auth/audience", None), "https://console.cloud.google.com/auth/audience");
        assert_eq!(console("auth/audience", Some("gd-1")), "https://console.cloud.google.com/auth/audience?project=gd-1");
        assert_eq!(project_of("4815-abc.apps.googleusercontent.com"), Some("4815"));
        assert_eq!(project_of("abc.apps.googleusercontent.com"), None);
        assert!(is_project_id("gdrive-linux-471210"));
        assert!(!is_project_id("GDrive") && !is_project_id("short") && !is_project_id("trailing-"));
    }

    #[test]
    fn failures_come_with_a_fix() {
        let off = "Drive API error 403: Google Drive API has not been used in project 4815 before or it is disabled.";
        assert!(drive_error_hint(off, Some("4815")).unwrap().contains("drive.googleapis.com?project=4815"));
        assert!(drive_error_hint("Request had insufficient authentication scopes.", None).unwrap().contains("logout"));
        assert!(drive_error_hint("network unavailable", None).is_none());
        assert!(sign_in_hint("Sign-in failed: sign-in was cancelled: access_denied", None).unwrap().contains("Testing"));
        assert!(sign_in_hint("token endpoint: invalid_client Unauthorized", None).unwrap().contains("--client-json"));
    }

    #[test]
    fn quotes_unit_paths_only_when_needed() {
        assert_eq!(unit_quote("/usr/bin/gdrived"), "/usr/bin/gdrived");
        assert_eq!(unit_quote("/home/a b/gdrive.AppImage"), "\"/home/a b/gdrive.AppImage\"");
        assert_eq!(short_id("123456789012345-abc.apps.googleusercontent.com"), "123456789012…");
    }
}
