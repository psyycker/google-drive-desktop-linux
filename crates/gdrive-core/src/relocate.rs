//! Moving a Drive folder between the sync root and other storage locations.
//!
//! Runs with the engine stopped. Drive never sees anything: paths relative to the
//! Drive stay the same, only where they live on disk changes.

use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{anyhow, bail, Context, Result};

use crate::api::Progress;
use crate::config::{Config, Placement, META_DIR};
use crate::db::{Db, SyncedItem};
use crate::layout::Layout;
use crate::local;

/// Byte counts of a running move, for the status shown to the user.
#[derive(Default)]
pub struct MoveProgress {
    pub done: Progress,
    pub total: Progress,
}

/// Moves `folder` (Drive-relative) to the storage location `to`, or back under the
/// sync root when `to` is `None`.
///
/// `commit` gets the configuration with the new placement once the files are in
/// place and must save it before this returns: only then is the old copy removed,
/// so whatever happens in between, the saved config always points at a full copy.
/// Returns the disk area whose discarded files should be deleted afterwards
/// ([`local::delete_discarded`]).
pub fn relocate(
    config: &Config,
    db: &Db,
    folder: &Path,
    to: Option<&str>,
    progress: &MoveProgress,
    commit: &mut dyn FnMut(&Config) -> Result<()>,
) -> Result<PathBuf> {
    let folder = normalize(folder)?;
    let current = config.placements.iter().find(|p| p.folder == folder).map(|p| p.location.as_str());
    if current == to {
        bail!("{} is already there", folder.display());
    }
    match db.synced_by_path(&folder)? {
        Some(s) if s.is_dir => {}
        Some(_) => bail!("only folders can be moved to another drive"),
        None => bail!("{} is not a synced folder", folder.display()),
    }

    let mut next = config.clone();
    next.placements.retain(|p| p.folder != folder);
    if let Some(name) = to {
        next.placements.push(Placement { folder: folder.clone(), location: name.to_owned() });
    }
    next.validate_storage()?;
    let old = Layout::from_config(config);
    let new = Layout::from_config(&next);
    let src = old.abs(&folder);
    let dst = new.abs(&folder);
    let (src_volume, dst_volume) = (old.volume_of(&folder).to_path_buf(), new.volume_of(&folder).to_path_buf());
    if !dst_volume.is_dir() {
        bail!("{} doesn't exist (is the drive connected?)", dst_volume.display());
    }
    if local::stat(&src).is_none_or(|l| !l.is_dir) {
        bail!("{} is missing", src.display());
    }

    // The destination disk gets the same marker as the sync root, so an unmounted
    // disk is told apart from deleted files.
    std::fs::create_dir_all(dst_volume.join(META_DIR).join("tmp"))?;
    let parent = dst.parent().ok_or_else(|| anyhow!("{} has no parent", dst.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    // A real folder at the destination is the leftover of an earlier, interrupted move.
    if std::fs::symlink_metadata(&dst).is_ok_and(|m| !m.file_type().is_symlink()) {
        tracing::warn!("setting aside {} left over from an earlier move", dst.display());
        local::discard(&dst_volume, &dst)?;
    }

    let staging = local::staging_path(&dst);
    local::remove_any(&staging)?;
    let same_disk = std::fs::metadata(&src)?.dev() == std::fs::metadata(parent)?.dev();
    if same_disk {
        std::fs::rename(&src, &staging).with_context(|| format!("moving {}", src.display()))?;
    } else {
        let total = local::tree_size(&src);
        progress.total.store(total, Ordering::Relaxed);
        let free = free_space(parent)?;
        if free < total {
            bail!(
                "not enough space on {}: {} needed, {} free",
                dst_volume.display(),
                human(total),
                human(free)
            );
        }
        let copied = local::copy_tree(&src, &staging, &mut |n| {
            progress.done.fetch_add(n, Ordering::Relaxed);
        });
        if let Err(e) = copied {
            let _ = local::remove_any(&staging);
            return Err(e.context(format!("copying {} to {}", src.display(), dst_volume.display())));
        }
    }
    // Moving back under the sync root: the placeholder symlink holds the spot.
    local::remove_placeholder(&dst)?;
    std::fs::rename(&staging, &dst).with_context(|| format!("moving into {}", dst.display()))?;
    refresh_inodes(db, &new, &folder)?;

    if let Err(e) = commit(&next) {
        // Put things back as the saved config still describes them.
        let undo = (|| -> Result<()> {
            if same_disk {
                std::fs::rename(&dst, &src)?;
            } else {
                local::remove_any(&dst)?;
            }
            if current.is_some() {
                local::make_placeholder(&old.main().join(&folder), &src, old.main())?;
            }
            refresh_inodes(db, &old, &folder)
        })();
        if let Err(u) = undo {
            tracing::error!("could not undo moving {}: {u:#}", folder.display());
        }
        return Err(e);
    }

    // Point of no return passed: tidy up the old copy and the symlink.
    let link = new.main().join(&folder);
    if !same_disk && std::fs::symlink_metadata(&src).is_ok() && src != link {
        local::discard(&src_volume, &src)?;
    }
    if to.is_some() {
        local::make_placeholder(&link, &dst, new.main())?;
    }
    Ok(src_volume)
}

/// Re-reads the inodes (and stat) of the synced items under `rel` after their files
/// were copied: rename detection matches local items by inode.
pub fn refresh_inodes(db: &Db, layout: &Layout, rel: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(layout.abs(rel)).follow_links(false).into_iter().flatten() {
        let Some(item_rel) = layout.rel(entry.path()) else { continue };
        let Some(l) = local::stat(entry.path()) else { continue };
        if let Some(s) = db.synced_by_path(&item_rel)? {
            if s.inode != l.inode {
                db.upsert_synced(&SyncedItem { inode: l.inode, size: l.size, mtime_ns: l.mtime_ns, ..s })?;
            }
        }
    }
    Ok(())
}

/// A Drive-relative folder path without `.`, `..`, root or trailing parts.
pub fn normalize(folder: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for c in folder.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => bail!("{} is not a folder inside the Drive", folder.display()),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("the Drive itself can't be moved; pick a folder inside it");
    }
    Ok(out)
}

fn free_space(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("checking free space on {}", path.display()));
    }
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StorageLocation;

    struct Setup {
        _tmp: tempfile::TempDir,
        config: Config,
        db: Db,
    }

    /// A sync root with Gaming/a.txt synced, and an empty second disk.
    fn setup() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let ssd = tmp.path().join("ssd");
        std::fs::create_dir_all(main.join(META_DIR)).unwrap();
        std::fs::create_dir_all(main.join("Gaming")).unwrap();
        std::fs::write(main.join("Gaming/a.txt"), "hello").unwrap();
        std::fs::create_dir_all(&ssd).unwrap();
        let config = Config {
            sync_root: main.clone(),
            locations: vec![StorageLocation { name: "SSD".into(), root: ssd }],
            ..Config::default()
        };
        let db = Db::in_memory().unwrap();
        for (id, rel) in [("D", "Gaming"), ("F", "Gaming/a.txt")] {
            let l = local::stat(&main.join(rel)).unwrap();
            db.upsert_synced(&SyncedItem {
                id: id.into(),
                rel_path: rel.into(),
                is_dir: l.is_dir,
                md5: None,
                size: l.size,
                mtime_ns: l.mtime_ns,
                inode: l.inode,
            })
            .unwrap();
        }
        Setup { _tmp: tmp, config, db }
    }

    #[test]
    fn moves_there_and_back() {
        let s = setup();
        let main = s.config.sync_root.clone();
        let ssd = s.config.locations[0].root.clone();
        let mut saved = None;
        relocate(&s.config, &s.db, Path::new("Gaming"), Some("SSD"), &MoveProgress::default(), &mut |c| {
            saved = Some(c.clone());
            Ok(())
        })
        .unwrap();
        let there = saved.unwrap();
        assert_eq!(there.placements, [Placement { folder: "Gaming".into(), location: "SSD".into() }]);
        assert_eq!(std::fs::read_to_string(ssd.join("Gaming/a.txt")).unwrap(), "hello");
        assert_eq!(std::fs::read_link(main.join("Gaming")).unwrap(), ssd.join("Gaming"));
        assert!(ssd.join(META_DIR).is_dir());
        let row = s.db.synced_by_path(Path::new("Gaming/a.txt")).unwrap().unwrap();
        assert_eq!(row.inode, local::stat(&ssd.join("Gaming/a.txt")).unwrap().inode);

        let mut back = None;
        relocate(&there, &s.db, Path::new("Gaming"), None, &MoveProgress::default(), &mut |c| {
            back = Some(c.clone());
            Ok(())
        })
        .unwrap();
        assert!(back.unwrap().placements.is_empty());
        assert!(std::fs::symlink_metadata(main.join("Gaming")).unwrap().is_dir());
        assert_eq!(std::fs::read_to_string(main.join("Gaming/a.txt")).unwrap(), "hello");
        assert!(!ssd.join("Gaming").exists());
    }

    #[test]
    fn failed_commit_keeps_the_old_copy() {
        let s = setup();
        let main = s.config.sync_root.clone();
        let err = relocate(&s.config, &s.db, Path::new("Gaming"), Some("SSD"), &MoveProgress::default(), &mut |_| {
            bail!("disk full")
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read_to_string(main.join("Gaming/a.txt")).unwrap(), "hello");
        assert!(!s.config.locations[0].root.join("Gaming").exists());
        let row = s.db.synced_by_path(Path::new("Gaming/a.txt")).unwrap().unwrap();
        assert_eq!(row.inode, local::stat(&main.join("Gaming/a.txt")).unwrap().inode);
    }

    #[test]
    fn rejects_bad_requests() {
        let s = setup();
        let p = MoveProgress::default();
        let ok = &mut |_: &Config| Ok(());
        assert!(relocate(&s.config, &s.db, Path::new("Gaming"), None, &p, ok).is_err()); // already on main
        assert!(relocate(&s.config, &s.db, Path::new("Gaming/a.txt"), Some("SSD"), &p, ok).is_err()); // a file
        assert!(relocate(&s.config, &s.db, Path::new("Nope"), Some("SSD"), &p, ok).is_err()); // not synced
        assert!(relocate(&s.config, &s.db, Path::new("Gaming"), Some("HDD2"), &p, ok).is_err()); // no such location
        assert!(relocate(&s.config, &s.db, Path::new("../x"), Some("SSD"), &p, ok).is_err());
    }
}
