//! "Start on login" via an XDG autostart entry.

use std::path::PathBuf;

use anyhow::{Context, Result};

fn entry_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("autostart")
        .join("gdrive-linux.desktop")
}

pub fn is_enabled() -> bool {
    std::fs::read_to_string(entry_path())
        .map(|text| !text.lines().any(|l| l.trim() == "Hidden=true"))
        .unwrap_or(false)
}

/// Quotes a path for a desktop entry `Exec=` line.
fn exec_quote(path: &str) -> String {
    if path.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        return path.to_string();
    }
    let mut out = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // In desktop files a literal backslash must itself be escaped once more.
    out.replace('\\', "\\\\")
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    let path = entry_path();
    if !enabled {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
    // Inside an AppImage, current_exe() points into a temporary mount; $APPIMAGE is the
    // .AppImage file itself.
    let exe = match std::env::var_os("APPIMAGE") {
        Some(appimage) => PathBuf::from(appimage),
        None => std::env::current_exe().context("cannot determine the app's own path")?,
    };
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Google Drive\n\
         Comment=Keep your Google Drive in sync\n\
         Exec={}\n\
         Icon=gdrive-app\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_quote(&exe.to_string_lossy())
    );
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, entry).with_context(|| format!("writing {}", path.display()))
}
