use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// User configuration, stored at `~/.config/gdrive-linux/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// OAuth client ID of a Google Cloud "Desktop app" credential.
    pub client_id: String,
    /// OAuth client secret of the same credential (not actually secret for desktop apps).
    pub client_secret: String,
    /// Local folder that mirrors the whole of My Drive.
    pub sync_root: PathBuf,
    /// How often to poll the Drive Changes API, in seconds.
    pub poll_interval_secs: u64,
    /// Number of uploads/downloads that may run at the same time.
    pub max_concurrent_transfers: usize,
    /// Extra glob patterns (matched against file names) that are never synced.
    pub ignore: Vec<String>,
    /// When a file is deleted on Drive, move the local copy to the desktop trash
    /// instead of deleting it permanently.
    pub use_local_trash: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            sync_root: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("GoogleDrive"),
            poll_interval_secs: 15,
            max_concurrent_transfers: 4,
            ignore: Vec::new(),
            use_local_trash: true,
        }
    }
}

impl Config {
    pub fn is_complete(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }

    pub fn load() -> Result<Self> {
        let path = config_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_file();
        std::fs::create_dir_all(path.parent().unwrap())?;
        write_private(&path, toml::to_string_pretty(self)?.as_bytes())
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("gdrive-linux")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("gdrive-linux")
}

pub fn token_file() -> PathBuf {
    data_dir().join("token.json")
}

pub fn db_file() -> PathBuf {
    data_dir().join("state.db")
}

pub fn socket_path() -> PathBuf {
    match dirs::runtime_dir() {
        Some(dir) => dir.join("gdrive-linux.sock"),
        None => {
            let uid = std::env::var("UID").unwrap_or_else(|_| "user".into());
            PathBuf::from(format!("/tmp/gdrive-linux-{uid}.sock"))
        }
    }
}

/// Name of the hidden bookkeeping directory inside the sync root. It holds partial
/// downloads (same filesystem, so the final rename is atomic) and acts as a marker
/// that the sync root is really mounted.
pub const META_DIR: &str = ".gdrive-linux";

/// Writes a file readable only by the current user.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(contents)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
