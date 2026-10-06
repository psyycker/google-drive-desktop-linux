//! Local filesystem helpers: stat snapshots, hashing, ignore rules, naming.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use md5::{Digest, Md5};

use crate::config::META_DIR;

/// The parts of `stat(2)` used to notice local changes cheaply, without hashing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStat {
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ns: i64,
    pub inode: u64,
}

impl LocalStat {
    pub fn from_meta(meta: &std::fs::Metadata) -> Self {
        Self {
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            inode: meta.ino(),
        }
    }

    pub fn mtime(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_nanos(self.mtime_ns)
    }
}

/// Stats a path without following symlinks. Returns `None` for anything that
/// doesn't exist or isn't a regular file or directory (symlinks, sockets, …).
pub fn stat(path: &Path) -> Option<LocalStat> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.is_file() || meta.is_dir() {
        Some(LocalStat::from_meta(&meta))
    } else {
        None
    }
}

pub fn md5_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Decides which local names are never synced: editor swap files, partial
/// downloads, lock files, and the client's own bookkeeping directory.
pub struct IgnoreRules {
    globs: GlobSet,
}

const BUILTIN_IGNORES: &[&str] = &[
    ".~lock.*#",
    "~$*",
    "*~",
    ".*.swp",
    ".*.swx",
    ".*.swo",
    "4913",
    ".#*",
    "#*#",
    ".goutputstream-*",
    "*.part",
    "*.crdownload",
    ".Trash-*",
    ".directory",
];

impl IgnoreRules {
    pub fn new(extra: &[String]) -> Self {
        let mut builder = GlobSetBuilder::new();
        for pat in BUILTIN_IGNORES.iter().copied().chain(extra.iter().map(String::as_str)) {
            match Glob::new(pat) {
                Ok(g) => {
                    builder.add(g);
                }
                Err(e) => tracing::warn!("ignoring invalid ignore pattern {pat:?}: {e}"),
            }
        }
        Self { globs: builder.build().unwrap_or_else(|_| GlobSet::empty()) }
    }

    /// `rel` is relative to the sync root.
    pub fn is_ignored(&self, rel: &Path) -> bool {
        if rel.components().next().is_some_and(|c| c.as_os_str() == META_DIR) {
            return true;
        }
        // Anything inside an ignored folder is ignored too.
        rel.components().any(|c| self.globs.is_match(c.as_os_str()))
    }
}

/// Turns a Drive name into a valid, safe local file name.
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' => '∕', // U+2215 DIVISION SLASH looks like a slash but is legal in file names
            '\0' => '_',
            c => c,
        })
        .collect();
    match cleaned.as_str() {
        "" => "_".into(),
        "." => "_.".into(),
        ".." => "_..".into(),
        _ => cleaned,
    }
}

/// Inserts `suffix` before the extension: `report.pdf` + ` (2)` → `report (2).pdf`.
pub fn with_suffix(name: &str, suffix: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{}{}{}", &name[..dot], suffix, &name[dot..]),
        _ => format!("{name}{suffix}"),
    }
}

/// Finds a free "conflicted copy" name next to `path`.
pub fn conflict_path(path: &Path) -> PathBuf {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let date = chrono::Local::now().format("%Y-%m-%d");
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(""));
    for i in 1.. {
        let suffix = if i == 1 {
            format!(" (conflicted copy {date} {host})")
        } else {
            format!(" (conflicted copy {date} {host} {i})")
        };
        let candidate = dir.join(with_suffix(&name, &suffix));
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    unreachable!()
}

/// Depth of a relative path (`a` = 1, `a/b` = 2).
pub fn depth(rel: &Path) -> usize {
    rel.components().count()
}

pub fn rel_str(rel: &Path) -> String {
    rel.to_string_lossy().into_owned()
}

/// Prefix of the folders inside [`META_DIR`] that hold discarded local files
/// waiting to be deleted (see [`discard_contents`]).
const DISCARDED_PREFIX: &str = "discarded-";

/// Refuses to wipe a folder that does not look like a sync root: `/`, the home
/// folder or one of its parents, or a non-empty folder without our [`META_DIR`].
pub fn check_discardable(root: &Path) -> Result<()> {
    if !root.is_absolute() || root.parent().is_none() {
        bail!("refusing to delete everything in {}", root.display());
    }
    if dirs::home_dir().is_some_and(|home| home.starts_with(root)) {
        bail!("refusing to delete everything in {}: it contains your home folder", root.display());
    }
    if root.is_dir() && !root.join(META_DIR).is_dir() && std::fs::read_dir(root)?.next().is_some() {
        bail!("{} does not look like a Google Drive sync folder; not deleting it", root.display());
    }
    Ok(())
}

/// Moves everything in `root` (except [`META_DIR`]) into a fresh folder inside
/// [`META_DIR`], which sync ignores, and returns that folder. Renames are instant even
/// for huge trees, so the slow delete can happen later with [`delete_discarded`].
/// Entries that cannot be renamed (e.g. another mount) are deleted on the spot.
pub fn discard_contents(root: &Path) -> Result<Option<PathBuf>> {
    check_discardable(root)?;
    if !root.is_dir() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let target = root.join(META_DIR).join(format!("{DISCARDED_PREFIX}{}", stamp.as_nanos()));
    std::fs::create_dir_all(&target)?;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name() == META_DIR {
            continue;
        }
        if std::fs::rename(entry.path(), target.join(entry.file_name())).is_err() {
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(Some(target))
}

/// Permanently deletes every folder left by [`discard_contents`], including ones from
/// an earlier run that was interrupted.
pub fn delete_discarded(root: &Path) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(root.join(META_DIR)) else { return Ok(()) };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(DISCARDED_PREFIX) {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_goes_before_extension() {
        assert_eq!(with_suffix("report.pdf", " (2)"), "report (2).pdf");
        assert_eq!(with_suffix("Makefile", " (2)"), "Makefile (2)");
        assert_eq!(with_suffix(".bashrc", " (2)"), ".bashrc (2)");
    }

    #[test]
    fn discards_everything_but_the_meta_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Drive");
        std::fs::create_dir_all(root.join(META_DIR).join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("docs/sub")).unwrap();
        std::fs::write(root.join("docs/sub/a.txt"), "a").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();

        let discarded = discard_contents(&root).unwrap().unwrap();
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, [META_DIR]);
        assert!(discarded.join("docs/sub/a.txt").is_file());

        delete_discarded(&root).unwrap();
        assert!(!discarded.exists());
        assert!(root.join(META_DIR).join("tmp").is_dir());
    }

    #[test]
    fn refuses_to_discard_foreign_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious.txt"), "x").unwrap();
        assert!(discard_contents(dir.path()).is_err());
        assert!(dir.path().join("precious.txt").exists());
        assert!(check_discardable(Path::new("/")).is_err());
        if let Some(home) = dirs::home_dir() {
            assert!(check_discardable(&home).is_err());
        }
        // A missing or empty folder is fine: there is nothing to lose.
        assert!(discard_contents(&dir.path().join("missing")).unwrap().is_none());
    }

    #[test]
    fn sanitizes_slashes_and_dots() {
        assert_eq!(sanitize_name("a/b"), "a∕b");
        assert_eq!(sanitize_name(".."), "_..");
    }

    #[test]
    fn ignores_temp_files_and_meta_dir() {
        let rules = IgnoreRules::new(&["*.tmp".into()]);
        assert!(rules.is_ignored(Path::new(".gdrive-linux/tmp/x")));
        assert!(rules.is_ignored(Path::new("docs/.~lock.report.odt#")));
        assert!(rules.is_ignored(Path::new("docs/.report.txt.swp")));
        assert!(rules.is_ignored(Path::new("a/b.tmp")));
        assert!(!rules.is_ignored(Path::new("docs/report.odt")));
    }
}
