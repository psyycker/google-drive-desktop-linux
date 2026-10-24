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
use gdrive_core::status::{human_bytes, Status, SyncState};

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
    /// Mirror (download the whole Drive) or stream (only the folders you choose).
    #[arg(long, value_enum, conflicts_with = "whole_drive")]
    mode: Option<Mode>,
    /// Stream, starting with this Drive folder, like `Work/Reports`; repeat for more.
    #[arg(long = "sync", value_name = "FOLDER", conflicts_with_all = ["whole_drive", "mode"])]
    sync: Vec<PathBuf>,
    /// Same as `--mode mirror` (kept for scripts written for 0.5.1).
    #[arg(long, hide = true)]
    whole_drive: bool,
    /// Run by `gdrive update` with the new version: only the steps that version added.
    #[arg(long, hide = true)]
    after_update: bool,
    /// Accept the default answer to every question (needs the client from flags or
    /// already configured).
    #[arg(short, long)]
    yes: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Download everything on the Drive and keep it all in sync.
    Mirror,
    /// Download only the folders you choose afterwards.
    Stream,
}

/// Setup steps whose answer can't be told from the daemon's state, recorded once done
/// so they aren't asked again (by a later `gdrive setup`, or after an update).
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Done {
    #[serde(default)]
    steps: Vec<String>,
}

const MODE_STEP: &str = "sync-mode";

fn done_file() -> PathBuf {
    gdrive_core::config::config_dir().join("setup-done.json")
}

impl Done {
    fn load() -> Self {
        std::fs::read_to_string(done_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn has(&self, step: &str) -> bool {
        self.steps.iter().any(|s| s == step)
    }

    fn mark(step: &str) -> Result<()> {
        let mut done = Self::load();
        if !done.has(step) {
            done.steps.push(step.to_owned());
            std::fs::create_dir_all(gdrive_core::config::config_dir())?;
            std::fs::write(done_file(), serde_json::to_string_pretty(&done)?)?;
        }
        Ok(())
    }
}

const STEPS: usize = 6;
const CLIENT_ID_SUFFIX: &str = ".apps.googleusercontent.com";

/// Questions and answers. Standard input is only read when an answer is wanted (never
/// in the background), so the folder picker can take over the keyboard.
struct Ui {
    interactive: bool,
    yes: bool,
}

fn read_line() -> Result<String> {
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        bail!("no more input");
    }
    Ok(line.trim().to_owned())
}

/// A line typed within `timeout`, if any. In a terminal's line mode, input becomes
/// readable only once Enter is pressed, so this never stops halfway through a line.
fn line_within(timeout: Duration) -> Result<Option<String>> {
    let mut fd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd, and its count.
    let ready = unsafe { libc::poll(&mut fd, 1, timeout.as_millis() as libc::c_int) };
    if ready <= 0 {
        return Ok(None);
    }
    read_line().map(Some)
}

impl Ui {
    fn new(yes: bool) -> Self {
        Self { interactive: std::io::stdin().is_terminal(), yes }
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
        Ok(Some(tokio::task::spawn_blocking(read_line).await??))
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
    if args.after_update {
        return after_update(&mut ui).await;
    }
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

    ui.step(6, "Mirror or stream");
    choose_mode(&mut ui, &args).await?;

    let st = status().await?;
    if st.selected_folders == Some(0) {
        println!("\nAll set. Now choose the folders to stream to {}:", st.sync_root);
        println!("  gdrive folders browse");
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
async fn sign_in_once() -> Result<std::result::Result<(), String>> {
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

    loop {
        // Waits up to a second for a pasted address, then looks at the sign-in.
        if let Some(line) = tokio::task::spawn_blocking(|| line_within(Duration::from_secs(1))).await?? {
            if !line.is_empty() {
                match tokio::task::spawn_blocking(move || deliver_redirect(&line)).await? {
                    Ok(()) => println!("  Finishing sign-in…"),
                    Err(e) => {
                        println!("  {e:#}");
                        print!("  > ");
                        std::io::stdout().flush()?;
                    }
                }
            }
        }
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
        let Err(reason) = sign_in_once().await? else { return Ok(()) };
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

/// Mirror (`None`) or stream, as the folder selection says, when that answers it:
/// a whole-Drive selection or chosen folders mean the question was settled.
fn mode_of(selected: Option<usize>, done: &Done) -> Option<Mode> {
    match selected {
        None => Some(Mode::Mirror),
        Some(0) if !done.has(MODE_STEP) => None,
        Some(_) => Some(Mode::Stream),
    }
}

async fn choose_mode(ui: &mut Ui, args: &SetupArgs) -> Result<()> {
    let st = status().await?;
    let flagged = match (args.mode, args.whole_drive, args.sync.is_empty()) {
        (Some(m), _, _) => Some(m),
        (None, true, _) => Some(Mode::Mirror),
        (None, false, false) => Some(Mode::Stream),
        (None, false, true) => None,
    };
    let mode = match (flagged, mode_of(st.selected_folders, &Done::load())) {
        (None, Some(Mode::Mirror)) => {
            println!("  ✓ Mirroring your whole Drive (stream only some folders with `gdrive folders browse`)");
            return Ok(());
        }
        (None, Some(Mode::Stream)) => {
            match st.selected_folders.unwrap_or(0) {
                0 => println!("  ✓ Streaming; no folders chosen yet"),
                n => println!("  ✓ Streaming {n} chosen folder(s) (`gdrive folders browse` changes them)"),
            }
            return Ok(());
        }
        (Some(m), _) => m,
        (None, None) if !ui.interactive || ui.yes => {
            println!("  Streaming for now: nothing downloads until you choose folders with `gdrive folders browse`.");
            println!("  To download everything instead: `gdrive folders all`.");
            return Ok(());
        }
        (None, None) => ask_mode(ui, &st).await?,
    };
    match mode {
        Mode::Mirror => {
            ipc::call(&Request::SetFolders { folders: None }).await?;
            println!("  ✓ Mirroring your whole Drive");
        }
        Mode::Stream if !args.sync.is_empty() => {
            ipc::call(&Request::SetFolders { folders: Some(args.sync.clone()) }).await?;
            let names: Vec<String> = args.sync.iter().map(|p| p.display().to_string()).collect();
            println!("  ✓ Streaming {}", names.join(", "));
        }
        Mode::Stream => {
            // Switching from a mirror would remove every local copy; that's the
            // picker's job, with its confirmation.
            if st.selected_folders.is_none() {
                println!("  You're mirroring now: choose what to keep with `gdrive folders browse`.");
                return Ok(());
            }
            println!("  ✓ Streaming: choose the folders once setup is done");
        }
    }
    Done::mark(MODE_STEP)
}

async fn ask_mode(ui: &mut Ui, st: &Status) -> Result<Mode> {
    let size = st
        .quota
        .as_ref()
        .map(|q| format!(" (your Google storage holds {} in all)", human_bytes(q.used)))
        .unwrap_or_default();
    println!("  How should this computer keep your Drive?");
    println!("   1. Mirror: download everything and keep it all in sync{size}.");
    println!("   2. Stream: download only the folders you choose; the rest stays online.");
    loop {
        let answer = ui.ask("Mirror or stream? [1/2]").await?.unwrap_or_default();
        match answer.to_lowercase().as_str() {
            "1" | "m" | "mirror" => return Ok(Mode::Mirror),
            "2" | "s" | "stream" => return Ok(Mode::Stream),
            _ => println!("  Please answer 1 or 2."),
        }
    }
}

/// After an update: the steps this version added that haven't been done, and nothing
/// when there are none. Setup that was never finished is left to `gdrive setup`.
async fn after_update(ui: &mut Ui) -> Result<()> {
    let Ok(st) = status().await else { return Ok(()) };
    if !get_config().await?.is_complete() || !is_signed_in(&st) {
        println!("Finish setting up with `gdrive setup`.");
        return Ok(());
    }
    if mode_of(st.selected_folders, &Done::load()).is_none() {
        println!("\nNew in this version: choose to mirror or stream your Drive.");
        let args = SetupArgs::default();
        choose_mode(ui, &args).await?;
        if status().await?.selected_folders == Some(0) {
            println!("  Choose the folders to stream with `gdrive folders browse`.");
        }
    }
    Ok(())
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
    fn the_mode_question_is_asked_once() {
        let none = Done::default();
        let asked = Done { steps: vec![MODE_STEP.into()] };
        // Configs from before streaming mirror the whole Drive: settled.
        assert!(mode_of(None, &none) == Some(Mode::Mirror));
        assert!(mode_of(Some(2), &none) == Some(Mode::Stream));
        // Nothing chosen yet: a fresh setup, unless streaming was the answer.
        assert!(mode_of(Some(0), &none).is_none());
        assert!(mode_of(Some(0), &asked) == Some(Mode::Stream));
    }

    #[test]
    fn quotes_unit_paths_only_when_needed() {
        assert_eq!(unit_quote("/usr/bin/gdrived"), "/usr/bin/gdrived");
        assert_eq!(unit_quote("/home/a b/gdrive.AppImage"), "\"/home/a b/gdrive.AppImage\"");
        assert_eq!(short_id("123456789012345-abc.apps.googleusercontent.com"), "123456789012…");
    }
}
