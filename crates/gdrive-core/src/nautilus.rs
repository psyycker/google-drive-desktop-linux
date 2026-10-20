//! Right-click menu items in GNOME Files, through a nautilus-python extension.

use std::path::{Path, PathBuf};

use anyhow::Result;

const EXTENSION: &str = include_str!("../../../packaging/nautilus/gdrive_linux.py");
const FILE_NAME: &str = "gdrive_linux.py";

/// Where distributions put nautilus-python's loader; the extension does nothing without it.
const LOADERS: &[&str] = &[
    "/usr/lib64/nautilus/extensions-4/libnautilus-python.so",
    "/usr/lib/nautilus/extensions-4/libnautilus-python.so",
    "/usr/lib/x86_64-linux-gnu/nautilus/extensions-4/libnautilus-python.so",
    "/usr/lib/aarch64-linux-gnu/nautilus/extensions-4/libnautilus-python.so",
];

fn extension_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("nautilus-python/extensions").join(FILE_NAME))
}

/// Installs or updates the extension when nautilus-python is available. Returns
/// whether the file changed (Files only loads extensions when it starts).
pub fn install_extension() -> Result<bool> {
    if !LOADERS.iter().any(|p| Path::new(p).exists()) {
        return Ok(false);
    }
    let Some(path) = extension_path() else { return Ok(false) };
    if std::fs::read_to_string(&path).is_ok_and(|current| current == EXTENSION) {
        return Ok(false);
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, EXTENSION)?;
    Ok(true)
}

/// The Drive web page of an item, for when Drive didn't send a `webViewLink`.
pub fn fallback_link(id: &str, is_dir: bool) -> String {
    if is_dir {
        format!("https://drive.google.com/drive/folders/{id}")
    } else {
        format!("https://drive.google.com/open?id={id}")
    }
}
