//! Choosing which Drive folders to sync: finding them in the remote tree by path.
//!
//! Paths here are Drive-relative and spelled the way they appear on disk, so a folder
//! can be picked before anything of it was downloaded.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};

use crate::config::SelectedFolder;
use crate::db::{Db, RemoteItem};
use crate::local;

/// Reads the remote tree to answer questions about Drive folders.
pub struct DriveFolders<'a> {
    db: &'a Db,
    root_id: String,
}

impl<'a> DriveFolders<'a> {
    /// Fails while the Drive has not been listed yet (first sync after signing in).
    pub fn new(db: &'a Db) -> Result<Self> {
        let root_id = db
            .get_meta("root_id")?
            .ok_or_else(|| anyhow!("your Drive hasn't been read yet; sign in, or try again in a moment"))?;
        Ok(Self { db, root_id })
    }

    /// Drive id of the folder at `rel`; the empty path is the Drive root.
    pub fn resolve(&self, rel: &Path) -> Result<String> {
        let mut cur = self.root_id.clone();
        let mut so_far = PathBuf::new();
        for part in rel.components() {
            let name = part.as_os_str().to_string_lossy();
            let mut matches = self.subfolders(&cur)?.into_iter().filter(|f| local::sanitize_name(&f.name) == name);
            let found = matches.next().ok_or_else(|| match so_far.as_os_str().is_empty() {
                true => anyhow!("there is no folder called \"{name}\" in your Drive"),
                false => anyhow!("there is no folder called \"{name}\" in {}", so_far.display()),
            })?;
            if matches.next().is_some() {
                bail!("there are several folders called {} on Drive; rename one of them first", so_far.join(&*name).display());
            }
            so_far.push(&*name);
            cur = found.id;
        }
        Ok(cur)
    }

    /// Where the folder `id` is now, or `None` if it isn't in My Drive any more.
    pub fn path_of(&self, id: &str) -> Result<Option<PathBuf>> {
        let mut parts = Vec::new();
        let mut cur = id.to_owned();
        while cur != self.root_id {
            let Some(item) = self.db.get_remote(&cur)? else { return Ok(None) };
            if parts.len() > 256 {
                return Ok(None);
            }
            parts.push(local::sanitize_name(&item.name));
            let Some(parent) = item.parent_id else { return Ok(None) };
            cur = parent;
        }
        Ok(Some(parts.iter().rev().collect()))
    }

    /// Folders directly inside `parent_id`, by name.
    pub fn subfolders(&self, parent_id: &str) -> Result<Vec<RemoteItem>> {
        let mut out: Vec<RemoteItem> = self.db.remote_children(parent_id)?.into_iter().filter(RemoteItem::is_dir).collect();
        out.sort_by_cached_key(|f| (f.name.to_lowercase(), f.id.clone()));
        Ok(out)
    }

    /// The selection with each folder's path brought up to date.
    pub fn refresh(&self, selected: &[SelectedFolder]) -> Result<Vec<SelectedFolder>> {
        selected
            .iter()
            .map(|f| Ok(SelectedFolder { id: f.id.clone(), path: self.path_of(&f.id)?.unwrap_or_else(|| f.path.clone()) }))
            .collect()
    }
}
