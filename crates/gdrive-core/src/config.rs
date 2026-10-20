use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
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
    /// Total download speed limit in MB/s across all transfers; 0 = unlimited.
    pub max_download_mb_per_sec: f64,
    /// Total upload speed limit in MB/s across all transfers; 0 = unlimited.
    pub max_upload_mb_per_sec: f64,
    /// Install new versions automatically when syncing is idle (AppImage only).
    pub auto_update: bool,
    /// Other disks that can hold some of the Drive's folders.
    pub locations: Vec<StorageLocation>,
    /// Drive folders kept on one of `locations` instead of under `sync_root`. Only the
    /// daemon changes these, as it moves the files.
    pub placements: Vec<Placement>,
}

/// A folder on another disk, e.g. an SSD, that mirrors part of the Drive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageLocation {
    pub name: String,
    pub root: PathBuf,
}

/// A Drive folder (and everything in it) kept at `<location root>/<folder>`, with a
/// symlink to it left at `<sync_root>/<folder>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Placement {
    /// Path relative to the Drive root, e.g. `Gaming`.
    pub folder: PathBuf,
    /// Name of the [`StorageLocation`].
    pub location: String,
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
            max_download_mb_per_sec: 0.0,
            max_upload_mb_per_sec: 0.0,
            auto_update: true,
            locations: Vec::new(),
            placements: Vec::new(),
        }
    }
}

impl Config {
    pub fn is_complete(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()
    }

    pub fn location(&self, name: &str) -> Option<&StorageLocation> {
        self.locations.iter().find(|l| l.name == name)
    }

    /// Checks that storage locations and placements make sense together.
    pub fn validate_storage(&self) -> Result<()> {
        for (i, loc) in self.locations.iter().enumerate() {
            if loc.name.trim().is_empty() {
                bail!("every storage location needs a name");
            }
            if !loc.root.is_absolute() {
                bail!("the folder of storage location \"{}\" must be an absolute path", loc.name);
            }
            if overlaps(&loc.root, &self.sync_root) {
                bail!("storage location \"{}\" can't be inside or around the Google Drive folder", loc.name);
            }
            for other in &self.locations[..i] {
                if other.name == loc.name {
                    bail!("two storage locations are called \"{}\"", loc.name);
                }
                if overlaps(&other.root, &loc.root) {
                    bail!("storage locations \"{}\" and \"{}\" overlap", other.name, loc.name);
                }
            }
        }
        for (i, p) in self.placements.iter().enumerate() {
            if p.folder.as_os_str().is_empty() || !p.folder.is_relative() {
                bail!("{} is not a folder inside the Drive", p.folder.display());
            }
            if self.location(&p.location).is_none() {
                bail!("{} is kept on \"{}\", which is not a storage location", p.folder.display(), p.location);
            }
            if let Some(other) = self.placements[..i].iter().find(|o| overlaps(&o.folder, &p.folder)) {
                bail!("{} and {} can't both be on other drives: one is inside the other", other.folder.display(), p.folder.display());
            }
        }
        Ok(())
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

/// Whether one path is the other or inside it.
pub fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
