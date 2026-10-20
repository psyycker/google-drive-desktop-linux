//! Where Drive-relative paths live on disk.
//!
//! Most of the Drive sits under the sync root; folders placed on another storage
//! location live at `<location root>/<same relative path>`. Everything that touches
//! the disk goes through [`Layout`], so the rest of the engine keeps working with
//! Drive-relative paths only.

use std::path::{Path, PathBuf};

use crate::config::{Config, META_DIR};

#[derive(Debug, Clone)]
pub struct Layout {
    main: PathBuf,
    /// Placed folders (Drive-relative) and the root of the location holding them.
    placed: Vec<(PathBuf, PathBuf)>,
}

impl Layout {
    pub fn from_config(config: &Config) -> Self {
        let placed = config
            .placements
            .iter()
            .filter_map(|p| Some((p.folder.clone(), config.location(&p.location)?.root.clone())))
            .collect();
        Self { main: config.sync_root.clone(), placed }
    }

    /// The sync root.
    pub fn main(&self) -> &Path {
        &self.main
    }

    /// The location root holding `rel`, or `None` when it lives under the sync root.
    fn location_of(&self, rel: &Path) -> Option<&Path> {
        self.placed.iter().find(|(folder, _)| rel.starts_with(folder)).map(|(_, root)| root.as_path())
    }

    /// Root of the disk area `rel` lives in.
    pub fn volume_of(&self, rel: &Path) -> &Path {
        self.location_of(rel).unwrap_or(&self.main)
    }

    pub fn abs(&self, rel: &Path) -> PathBuf {
        self.volume_of(rel).join(rel)
    }

    /// The Drive-relative path of `abs`, whether it is given under the sync root
    /// (including through a placeholder symlink) or under a location root.
    pub fn rel(&self, abs: &Path) -> Option<PathBuf> {
        if let Ok(rel) = abs.strip_prefix(&self.main) {
            return Some(rel.to_path_buf());
        }
        self.placed.iter().find_map(|(folder, root)| {
            let rel = abs.strip_prefix(root).ok()?;
            rel.starts_with(folder).then(|| rel.to_path_buf())
        })
    }

    /// Partial downloads for `rel`: on the same disk, so the final rename is atomic.
    pub fn tmp_dir(&self, rel: &Path) -> PathBuf {
        self.volume_of(rel).join(META_DIR).join("tmp")
    }

    /// The sync root and every location root that holds a placed folder.
    pub fn volumes(&self) -> Vec<&Path> {
        let mut out = vec![self.main.as_path()];
        for (_, root) in &self.placed {
            if !out.contains(&root.as_path()) {
                out.push(root);
            }
        }
        out
    }

    /// Placed folders, Drive-relative.
    pub fn placed_folders(&self) -> impl Iterator<Item = &Path> {
        self.placed.iter().map(|(folder, _)| folder.as_path())
    }

    /// Whether `rel` is exactly a placed folder (its sync-root spot holds a symlink).
    pub fn is_placed_folder(&self, rel: &Path) -> bool {
        self.placed.iter().any(|(folder, _)| folder == rel)
    }

    /// Whether `rel` lives under the given volume root.
    pub fn on_volume(&self, rel: &Path, volume: &Path) -> bool {
        self.volume_of(rel) == volume
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Placement, StorageLocation};

    fn layout() -> Layout {
        let config = Config {
            sync_root: "/hdd/Drive".into(),
            locations: vec![StorageLocation { name: "SSD".into(), root: "/ssd/Drive".into() }],
            placements: vec![Placement { folder: "Gaming".into(), location: "SSD".into() }],
            ..Config::default()
        };
        Layout::from_config(&config)
    }

    #[test]
    fn resolves_placed_folders_to_their_location() {
        let l = layout();
        assert_eq!(l.abs(Path::new("Gaming")), Path::new("/ssd/Drive/Gaming"));
        assert_eq!(l.abs(Path::new("Gaming/a/b.txt")), Path::new("/ssd/Drive/Gaming/a/b.txt"));
        assert_eq!(l.abs(Path::new("Gaming2/x")), Path::new("/hdd/Drive/Gaming2/x"));
        assert_eq!(l.abs(Path::new("Docs/x")), Path::new("/hdd/Drive/Docs/x"));
        assert_eq!(l.tmp_dir(Path::new("Gaming/x")), Path::new("/ssd/Drive/.gdrive-linux/tmp"));
        assert_eq!(l.tmp_dir(Path::new("Docs/x")), Path::new("/hdd/Drive/.gdrive-linux/tmp"));
    }

    #[test]
    fn maps_both_spellings_back() {
        let l = layout();
        assert_eq!(l.rel(Path::new("/ssd/Drive/Gaming/a")).unwrap(), Path::new("Gaming/a"));
        assert_eq!(l.rel(Path::new("/hdd/Drive/Gaming/a")).unwrap(), Path::new("Gaming/a"));
        assert_eq!(l.rel(Path::new("/hdd/Drive/Docs")).unwrap(), Path::new("Docs"));
        // Elsewhere on the location disk is not part of the Drive.
        assert!(l.rel(Path::new("/ssd/Drive/Other/a")).is_none());
        assert!(l.rel(Path::new("/etc/hosts")).is_none());
        assert_eq!(l.volumes(), [Path::new("/hdd/Drive"), Path::new("/ssd/Drive")]);
    }

    #[test]
    fn validation_rejects_overlaps() {
        let mut c = Config {
            sync_root: "/hdd/Drive".into(),
            locations: vec![StorageLocation { name: "SSD".into(), root: "/ssd/Drive".into() }],
            placements: vec![Placement { folder: "Gaming".into(), location: "SSD".into() }],
            ..Config::default()
        };
        c.validate_storage().unwrap();
        c.placements.push(Placement { folder: "Gaming/Sub".into(), location: "SSD".into() });
        assert!(c.validate_storage().is_err());
        c.placements.pop();
        c.locations.push(StorageLocation { name: "Inner".into(), root: "/hdd/Drive/x".into() });
        assert!(c.validate_storage().is_err());
        c.locations.pop();
        c.placements.push(Placement { folder: "Music".into(), location: "Nope".into() });
        assert!(c.validate_storage().is_err());
    }
}
