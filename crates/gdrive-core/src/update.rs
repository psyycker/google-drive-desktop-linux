//! Update checks against GitHub Releases, and self-replacement for AppImage installs.
//!
//! Only an AppImage can safely update itself: it's one user-owned file, and running
//! processes keep their already-mounted copy while the file is atomically replaced.
//! Package-manager installs (.deb/.rpm) and source builds only get notified.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::api::Progress;

pub const REPO: &str = "psyycker/google-drive-desktop-linux";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_API: &str = "https://api.github.com";

/// A `major.minor.patch` version; pre-release/build suffixes are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u64>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().unwrap_or(Ok(0)).ok()?;
        let patch = parts.next().unwrap_or(Ok(0)).ok()?;
        Some(Self(major, minor, patch))
    }

    pub fn current() -> Self {
        Self::parse(CURRENT_VERSION).expect("crate version is valid")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn version(&self) -> Option<Version> {
        Version::parse(&self.tag_name)
    }

    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    /// The AppImage built for this machine's architecture, if the release has one.
    pub fn appimage(&self) -> Option<&Asset> {
        let version = self.version()?;
        self.asset(&appimage_asset_name(&version))
    }
}

pub fn appimage_asset_name(version: &Version) -> String {
    format!("gdrive-linux-{version}-{}.AppImage", std::env::consts::ARCH)
}

/// The AppImage file this process was started from, if any.
pub fn running_appimage() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
    path.is_file().then_some(path)
}

/// Prepares a command that launches an AppImage from inside another AppImage.
///
/// The AppImage launcher `cd`s into its mount and prepends mount paths to PATH,
/// LD_LIBRARY_PATH, XDG_DATA_DIRS and many GTK/GIO variables. A child that inherits
/// that working directory keeps our mount busy forever (it can't unmount after we
/// exit), and inherited variables pile up stale paths on every relaunch. So: start
/// from the home folder, drop every `/tmp/.mount_*` entry, and let the new instance
/// set up its own environment.
pub fn clean_appimage_launch<C: CommandEnv>(cmd: &mut C) {
    const OWN_VARS: [&str; 5] = ["APPDIR", "APPIMAGE", "ARGV0", "OWD", "PWD"];
    let is_mount = |s: &str| s.contains("/.mount_");
    for (key, value) in std::env::vars_os() {
        let (Some(k), Some(v)) = (key.to_str(), value.to_str()) else { continue };
        if OWN_VARS.contains(&k) {
            cmd.remove_env(k);
        } else if is_mount(v) {
            let kept: Vec<&str> = v.split(':').filter(|e| !e.is_empty() && !is_mount(e)).collect();
            if kept.is_empty() {
                cmd.remove_env(k);
            } else {
                cmd.set_env(k, &kept.join(":"));
            }
        }
    }
    cmd.set_dir(&dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
}

/// Marks every inherited file descriptor (fd ≥ 3) close-on-exec.
///
/// The AppImage runtime hands its process an inheritable descriptor on the mount
/// point. Without this, every process we spawn would inherit it and keep our mount
/// alive after we exit — and the chain repeats with each update or relaunch.
/// Call once, early in `main`.
pub fn mark_inherited_fds_cloexec() {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else { return };
    let fds: Vec<i32> = entries.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).filter(|&fd| fd > 2).collect();
    for fd in fds {
        // SAFETY: fcntl on a descriptor number only reads/sets its flags; a stale
        // number (e.g. the read_dir handle, now closed) just fails with EBADF.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

/// The bits of `std::process::Command` / `tokio::process::Command` used above.
pub trait CommandEnv {
    fn remove_env(&mut self, key: &str);
    fn set_env(&mut self, key: &str, value: &str);
    fn set_dir(&mut self, dir: &Path);
}

impl CommandEnv for std::process::Command {
    fn remove_env(&mut self, key: &str) {
        self.env_remove(key);
    }
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
    fn set_dir(&mut self, dir: &Path) {
        self.current_dir(dir);
    }
}

impl CommandEnv for tokio::process::Command {
    fn remove_env(&mut self, key: &str) {
        self.env_remove(key);
    }
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
    fn set_dir(&mut self, dir: &Path) {
        self.current_dir(dir);
    }
}

/// Whether we may replace `appimage` (its folder must be writable by us).
pub fn can_replace(appimage: &Path) -> bool {
    let Some(dir) = appimage.parent() else { return false };
    let probe = dir.join(format!(".gdrive-linux-write-test-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub struct Updater {
    http: reqwest::Client,
    api_base: String,
}

impl Updater {
    pub fn new(http: reqwest::Client) -> Self {
        Self::with_api_base(http, GITHUB_API)
    }

    /// Like [`Updater::new`] against another API host (used by tests).
    pub fn with_api_base(http: reqwest::Client, api_base: &str) -> Self {
        Self { http, api_base: api_base.trim_end_matches('/').to_owned() }
    }

    /// The newest published, non-prerelease release.
    pub async fn latest(&self) -> Result<Release> {
        let url = format!("{}/repos/{REPO}/releases/latest", self.api_base);
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()
            .context("checking GitHub for updates")?;
        Ok(resp.json().await?)
    }

    /// Downloads the release's AppImage to `dest`, verifying its size and SHA-256
    /// against the release's `SHA256SUMS`, and marks it executable.
    pub async fn download_appimage(&self, release: &Release, dest: &Path, progress: &Progress) -> Result<()> {
        let asset = release.appimage().ok_or_else(|| anyhow!("release has no AppImage for this architecture"))?;
        let sums = release.asset("SHA256SUMS").ok_or_else(|| anyhow!("release has no SHA256SUMS"))?;
        let sums = self.http.get(&sums.browser_download_url).send().await?.error_for_status()?.text().await?;
        let expected = sums
            .lines()
            .find_map(|l| {
                let (hash, name) = l.split_once(char::is_whitespace)?;
                (name.trim().trim_start_matches('*') == asset.name).then(|| hash.to_ascii_lowercase())
            })
            .ok_or_else(|| anyhow!("{} is not listed in SHA256SUMS", asset.name))?;

        let resp = self.http.get(&asset.browser_download_url).send().await?.error_for_status()?;
        let result = async {
            let mut file = tokio::fs::File::create(dest).await?;
            let mut hasher = Sha256::new();
            let mut written = 0u64;
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                hasher.update(&chunk);
                file.write_all(&chunk).await?;
                written += chunk.len() as u64;
                progress.store(written, Ordering::Relaxed);
            }
            file.sync_all().await?;
            if written != asset.size {
                bail!("download is {written} bytes, expected {}", asset.size);
            }
            let actual = hex::encode(hasher.finalize());
            if actual != expected {
                bail!("checksum mismatch for {} (got {actual}, expected {expected})", asset.name);
            }
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755)).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(dest).await;
        }
        result
    }
}

/// Runs the downloaded AppImage once (`--cli --version`) to make sure it starts and
/// is the version we expect, before it replaces the working one.
pub async fn sanity_check(appimage: &Path, expected: &Version) -> Result<()> {
    let mut cmd = tokio::process::Command::new(appimage);
    clean_appimage_launch(&mut cmd);
    let run = cmd
        .args(["--cli", "--version"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(60), run)
        .await
        .map_err(|_| anyhow!("the new version did not start within 60 s"))??;
    let text = String::from_utf8_lossy(&out.stdout);
    let reported = text.split_whitespace().nth(1).and_then(Version::parse);
    if !out.status.success() || reported != Some(*expected) {
        bail!(
            "the new version failed its self-test (exit {}, output {:?})",
            out.status,
            format!("{}{}", text.trim(), String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    Ok(())
}

/// Atomically replaces `target` with `new` (both in the same folder).
pub fn install(new: &Path, target: &Path) -> Result<()> {
    std::fs::rename(new, target).with_context(|| format!("replacing {}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders_versions() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.10.0-rc.1"), Some(Version(0, 10, 0)));
        assert_eq!(Version::parse("2"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("vx.1"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
    }

    #[test]
    fn asset_name_matches_release_workflow() {
        let name = appimage_asset_name(&Version(0, 2, 0));
        assert_eq!(name, format!("gdrive-linux-0.2.0-{}.AppImage", std::env::consts::ARCH));
    }
}
