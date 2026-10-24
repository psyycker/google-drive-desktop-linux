//! `gdrive update`: finds the newest release on GitHub, picks the file that matches
//! how this copy was installed (desktop or terminal-only package, deb or rpm, the
//! tarball, the AppImage) and installs it.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use gdrive_core::ipc::{self, Request, Response};
use gdrive_core::status::{human_bytes, UpdatePhase};
use gdrive_core::update::{self, PackageFormat, Release, Updater, Version};

/// How this copy of gdrive was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Install {
    AppImage,
    Package { name: String, format: PackageFormat },
    /// The `-cli.tar.gz` binaries, unpacked into `dir`.
    Tarball { dir: PathBuf },
    /// Built from source (`cargo build`): updated with git, not here.
    Source,
}

impl Install {
    fn describe(&self) -> String {
        match self {
            Install::AppImage => "AppImage".into(),
            Install::Package { name, format: PackageFormat::Deb } => format!("{name} (.deb)"),
            Install::Package { name, format: PackageFormat::Rpm } => format!("{name} (.rpm)"),
            Install::Tarball { dir } => format!("terminal-only bundle in {}", dir.display()),
            Install::Source => "source build".into(),
        }
    }

    /// The release file this install updates from.
    fn asset(&self, version: &Version) -> Option<String> {
        let arch = std::env::consts::ARCH;
        match self {
            Install::AppImage => Some(update::appimage_asset_name(version)),
            Install::Package { name, format } => update::package_asset_name(name, *format, version, arch),
            Install::Tarball { .. } => Some(update::cli_tarball_name(version, arch)),
            Install::Source => None,
        }
    }
}

fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Which package owns `exe`: rpm's database first, then dpkg's.
fn owning_package(exe: &Path) -> Option<(String, PackageFormat)> {
    let path = exe.to_string_lossy();
    if let Some(name) = output_of("rpm", &["-qf", "--queryformat", "%{NAME}", &path]) {
        return Some((name, PackageFormat::Rpm));
    }
    // "gdrive-linux-cli: /usr/bin/gdrive"
    let line = output_of("dpkg-query", &["-S", &path])?;
    let name = line.split(':').next()?.trim().to_owned();
    (!name.is_empty()).then_some((name, PackageFormat::Deb))
}

fn detect() -> Result<Install> {
    if std::env::var_os("APPIMAGE").is_some() {
        return Ok(Install::AppImage);
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    if let Some((name, format)) = owning_package(&exe) {
        return Ok(Install::Package { name, format });
    }
    if exe.components().any(|c| c.as_os_str() == "target") {
        return Ok(Install::Source);
    }
    let dir = exe.parent().ok_or_else(|| anyhow!("can't tell where gdrive is installed"))?.to_owned();
    Ok(Install::Tarball { dir })
}

pub async fn run(check_only: bool, yes: bool) -> Result<()> {
    // Taken now: once the file is replaced, the kernel reports it as "… (deleted)".
    let exe = std::env::current_exe()?;
    let install = detect()?;
    let current = Version::current();
    println!("Installed: {current}, {}", install.describe());

    if install == Install::AppImage {
        return appimage_update().await;
    }

    let updater = Updater::standalone()?;
    let release = updater.latest().await.context("checking GitHub for the newest version")?;
    let latest = release.version().ok_or_else(|| anyhow!("the newest release ({}) has no version number", release.tag_name))?;
    println!("Newest:    {latest} ({})", release.html_url);

    if latest <= current {
        println!("You have the newest version.");
        return restart_stale_daemon(&current).await;
    }
    let name = match install.asset(&latest) {
        Some(name) => name,
        None if install == Install::Source => {
            println!("This is a build from source: update with `git pull` and build again.");
            return Ok(());
        }
        None => bail!(
            "{} isn't one of this project's packages on {}; update it the way it was installed",
            install.describe(),
            std::env::consts::ARCH
        ),
    };
    if release.asset(&name).is_none() {
        bail!("release {latest} has no {name}; download what fits from {}", release.html_url);
    }
    if check_only {
        println!("Run `gdrive update` to install {name}.");
        return Ok(());
    }
    if !yes && !confirm(&format!("Install {latest} ({name})?"))? {
        return Ok(());
    }

    let dir = download_dir()?;
    let file = dir.join(&name);
    download(&updater, &release, &name, &file).await?;
    let installed = match &install {
        Install::Package { format, .. } => install_package(*format, &file),
        Install::Tarball { dir } => install_tarball(&file, dir),
        Install::AppImage | Install::Source => unreachable!(),
    };
    let _ = std::fs::remove_file(&file);
    installed?;

    // The new binary, not this (old) process.
    let reported = output_of(&exe.to_string_lossy(), &["--version"])
        .and_then(|out| out.split_whitespace().nth(1).and_then(Version::parse));
    if reported != Some(latest) {
        bail!("installed, but {} still reports {:?}; check the output above", exe.display(), reported);
    }
    println!("✓ Updated to {latest}");
    if let Install::Package { name, .. } = &install {
        if name == update::DESKTOP_PACKAGE {
            println!("  Quit and reopen the Google Drive app to use the new version of the window.");
        }
    }
    restart_stale_daemon(&latest).await
}

fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not asking without a terminal; run `gdrive update --yes` to install");
    }
    print!("{question} [Y/n] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes"))
}

fn download_dir() -> Result<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| anyhow!("no home folder to download into"))?;
    let dir = cache.join("gdrive-linux").join("updates");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

async fn download(updater: &Updater, release: &Release, name: &str, dest: &Path) -> Result<()> {
    let total = release.asset(name).map_or(0, |a| a.size);
    let progress = Arc::new(AtomicU64::new(0));
    let shown = progress.clone();
    let terminal = std::io::stdout().is_terminal();
    let ticker = tokio::spawn(async move {
        if !terminal {
            return;
        }
        loop {
            let done = human_bytes(shown.load(Ordering::Relaxed));
            match total {
                0 => print!("\rDownloading… {done}   "),
                t => print!("\rDownloading… {done} of {}   ", human_bytes(t)),
            }
            let _ = std::io::stdout().flush();
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });
    let result = updater.download(release, name, dest, &progress).await;
    ticker.abort();
    println!("\rDownloaded {name} ({}), checksum verified", human_bytes(progress.load(Ordering::Relaxed)));
    result
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// Runs a package manager on the downloaded file, through sudo unless already root.
fn install_package(format: PackageFormat, file: &Path) -> Result<()> {
    let file = file.to_string_lossy().into_owned();
    let mut args: Vec<String> = match format {
        // Given a file path, apt installs it with its dependencies, like dnf.
        PackageFormat::Deb if on_path("apt-get") => vec!["apt-get".into(), "install".into(), "-y".into(), file],
        PackageFormat::Deb => vec!["dpkg".into(), "-i".into(), file],
        PackageFormat::Rpm if on_path("dnf") => vec!["dnf".into(), "install".into(), "-y".into(), file],
        PackageFormat::Rpm if on_path("zypper") => {
            vec!["zypper".into(), "--non-interactive".into(), "install".into(), file]
        }
        PackageFormat::Rpm => vec!["rpm".into(), "-U".into(), file],
    };
    if !is_root() {
        args.insert(0, "sudo".into());
    }
    println!("Running: {}", args.join(" "));
    let status = Command::new(&args[0]).args(&args[1..]).status().with_context(|| format!("running {}", args[0]))?;
    if !status.success() {
        bail!("{} failed ({status})", args[0]);
    }
    Ok(())
}

/// Replaces `gdrive` and `gdrived` in `dir` with the ones from the tarball. Each is
/// swapped with a rename, so running copies keep working until restarted.
fn install_tarball(tarball: &Path, dir: &Path) -> Result<()> {
    let unpacked = tarball.with_extension("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked)?;
    let ok = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(&unpacked)
        .status()
        .context("running tar")?
        .success();
    let result = (|| {
        if !ok {
            bail!("couldn't unpack {}", tarball.display());
        }
        // The tarball holds one folder, gdrive-linux-<version>-<arch>-cli/.
        let inner = std::fs::read_dir(&unpacked)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir())
            .ok_or_else(|| anyhow!("unexpected tarball layout"))?;
        for tool in ["gdrived", "gdrive"] {
            let target = dir.join(tool);
            if tool == "gdrived" && !target.exists() {
                println!("  (no gdrived in {}; installing it there too)", dir.display());
            }
            let staged = dir.join(format!(".{tool}.update"));
            std::fs::copy(inner.join(tool), &staged).with_context(|| {
                format!("can't write to {}; run `sudo gdrive update` or reinstall there", dir.display())
            })?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
            std::fs::rename(&staged, &target).with_context(|| format!("replacing {}", target.display()))?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&unpacked);
    result
}

/// The running daemon keeps its old code until it restarts.
async fn restart_stale_daemon(installed: &Version) -> Result<()> {
    let Ok(Response::Status { status }) = ipc::request(&Request::Status).await else { return Ok(()) };
    if Version::parse(&status.version) == Some(*installed) {
        return Ok(());
    }
    let under_systemd = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "gdrived"])
        .status()
        .is_ok_and(|s| s.success());
    if under_systemd {
        let restarted = Command::new("systemctl").args(["--user", "restart", "gdrived"]).status().is_ok_and(|s| s.success());
        if restarted {
            println!("✓ Restarted gdrived");
            return Ok(());
        }
    }
    println!(
        "The background service still runs {}: restart it (`systemctl --user restart gdrived`, or quit and reopen the app).",
        status.version
    );
    Ok(())
}

/// AppImages update themselves through the daemon, which waits until syncing is idle.
async fn appimage_update() -> Result<()> {
    ipc::call(&Request::CheckForUpdates).await?;
    print!("Checking…");
    std::io::stdout().flush()?;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Response::Status { status } = ipc::call(&Request::Status).await? else { continue };
        let Some(u) = status.update else { continue };
        println!();
        match u.phase {
            UpdatePhase::Available if u.automatic => println!("{} found; it installs itself when syncing is idle.", u.latest),
            UpdatePhase::Available => println!("{} is out: {}", u.latest, u.release_url),
            UpdatePhase::Downloading | UpdatePhase::Installing => println!("Installing {}…", u.latest),
            UpdatePhase::Failed => bail!("updating to {} failed: {}", u.latest, u.message.unwrap_or_default()),
        }
        return Ok(());
    }
    println!("\nYou have the newest version.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_install_picks_its_own_file() {
        let v = Version(0, 5, 1);
        let pkg = |name: &str, format| Install::Package { name: name.into(), format };
        let arch = std::env::consts::ARCH;
        let deb_arch = if arch == "x86_64" { "amd64" } else { "arm64" };
        assert_eq!(pkg("gdrive-linux-cli", PackageFormat::Rpm).asset(&v).unwrap(), format!("gdrive-linux-cli-0.5.1-1.{arch}.rpm"));
        assert_eq!(pkg("gdrive-linux", PackageFormat::Rpm).asset(&v).unwrap(), format!("gdrive-linux-0.5.1-1.{arch}.rpm"));
        assert_eq!(pkg("gdrive-linux-cli", PackageFormat::Deb).asset(&v).unwrap(), format!("gdrive-linux-cli_0.5.1-1_{deb_arch}.deb"));
        assert_eq!(pkg("gdrive-linux", PackageFormat::Deb).asset(&v).unwrap(), format!("gdrive-linux_0.5.1_{deb_arch}.deb"));
        assert_eq!(Install::Tarball { dir: "/x".into() }.asset(&v).unwrap(), format!("gdrive-linux-0.5.1-{arch}-cli.tar.gz"));
        // A distribution's own package of the same files isn't ours to replace.
        assert_eq!(pkg("gdrive", PackageFormat::Deb).asset(&v), None);
        assert_eq!(Install::Source.asset(&v), None);
    }
}
