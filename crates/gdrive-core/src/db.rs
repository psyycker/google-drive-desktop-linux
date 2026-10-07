//! SQLite persistence for the remote tree and the synced tree.
//!
//! Local paths are stored as raw bytes (BLOB) because Linux file names need not be
//! UTF-8. Byte-wise BLOB ordering also makes subtree queries simple range scans:
//! every descendant of `a/b` sorts between `a/b/` and `a/b0` ('0' is '/' + 1).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::api::{DriveFile, FOLDER_MIME};

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS remote (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    md5 TEXT,
    size INTEGER NOT NULL,
    mtime TEXT,
    link TEXT
);
CREATE INDEX IF NOT EXISTS remote_parent ON remote(parent_id, name);
CREATE TABLE IF NOT EXISTS synced (
    id TEXT PRIMARY KEY,
    rel_path BLOB NOT NULL UNIQUE,
    is_dir INTEGER NOT NULL,
    md5 TEXT,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    inode INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS synced_inode ON synced(inode);
CREATE TABLE IF NOT EXISTS upload_sessions (
    rel_path BLOB PRIMARY KEY,
    url TEXT NOT NULL,
    file_id TEXT,
    parent_id TEXT NOT NULL,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    created INTEGER NOT NULL
);
";

/// A resumable upload in progress, kept so an interrupted upload continues where it
/// stopped. It only applies while the local file and its destination are unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadSession {
    pub rel_path: PathBuf,
    pub url: String,
    /// The Drive file being replaced, or `None` for a new file.
    pub file_id: Option<String>,
    pub parent_id: String,
    pub name: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub created: DateTime<Utc>,
}

/// A file or folder as last reported by Drive.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteItem {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub mime: String,
    pub md5: Option<String>,
    pub size: u64,
    pub mtime: Option<DateTime<Utc>>,
    pub link: Option<String>,
}

impl RemoteItem {
    pub fn is_dir(&self) -> bool {
        self.mime == FOLDER_MIME
    }

    pub fn is_native(&self) -> bool {
        self.mime.starts_with("application/vnd.google-apps.") && !self.is_dir()
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            parent_id: row.get(1)?,
            name: row.get(2)?,
            mime: row.get(3)?,
            md5: row.get(4)?,
            size: row.get::<_, i64>(5)? as u64,
            mtime: row.get::<_, Option<String>>(6)?.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(Into::into),
            link: row.get(7)?,
        })
    }
}

impl From<&DriveFile> for RemoteItem {
    fn from(f: &DriveFile) -> Self {
        Self {
            id: f.id.clone(),
            parent_id: f.parents.first().cloned(),
            name: f.name.clone(),
            mime: f.mime_type.clone(),
            md5: f.md5_checksum.clone(),
            size: f.size(),
            mtime: f.modified_time,
            link: f.web_view_link.clone(),
        }
    }
}

/// Read access to the remote tree: the database, or a snapshot of it.
pub trait RemoteLookup {
    fn get_remote(&self, id: &str) -> Result<Option<RemoteItem>>;
    /// Children of `parent_id` called exactly `name`, ordered by id.
    fn remote_named(&self, parent_id: &str, name: &str) -> Result<Vec<RemoteItem>>;
}

/// The whole remote tree in memory, for resolving every item's path at once
/// without a query per item and per ancestor.
pub struct RemoteSnapshot {
    items: HashMap<String, RemoteItem>,
    /// parent id → name → ids, ordered by id.
    by_name: HashMap<String, HashMap<String, Vec<String>>>,
}

impl RemoteSnapshot {
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.items.keys().map(String::as_str)
    }

    pub fn get(&self, id: &str) -> Option<&RemoteItem> {
        self.items.get(id)
    }
}

impl RemoteLookup for RemoteSnapshot {
    fn get_remote(&self, id: &str) -> Result<Option<RemoteItem>> {
        Ok(self.items.get(id).cloned())
    }

    fn remote_named(&self, parent_id: &str, name: &str) -> Result<Vec<RemoteItem>> {
        let ids = self.by_name.get(parent_id).and_then(|m| m.get(name));
        Ok(ids.into_iter().flatten().filter_map(|id| self.items.get(id).cloned()).collect())
    }
}

/// An item whose local and remote versions were last known to agree.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncedItem {
    pub id: String,
    pub rel_path: PathBuf,
    pub is_dir: bool,
    /// MD5 of the content at sync time (None for folders and native Google files).
    pub md5: Option<String>,
    pub size: u64,
    pub mtime_ns: i64,
    pub inode: u64,
}

impl SyncedItem {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            rel_path: bytes_to_path(row.get::<_, Vec<u8>>(1)?),
            is_dir: row.get(2)?,
            md5: row.get(3)?,
            size: row.get::<_, i64>(4)? as u64,
            mtime_ns: row.get(5)?,
            inode: row.get::<_, i64>(6)? as u64,
        })
    }
}

fn path_bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_bytes().to_vec()
}

fn bytes_to_path(b: Vec<u8>) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&b))
}

/// Bounds of the BLOB range containing every strict descendant of `dir`.
fn subtree_range(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut lo = path_bytes(dir);
    let mut hi = lo.clone();
    lo.push(b'/');
    hi.push(b'0');
    (lo, hi)
}

const SYNCED_COLS: &str = "id, rel_path, is_dir, md5, size, mtime_ns, inode";
const REMOTE_COLS: &str = "id, parent_id, name, mime, md5, size, mtime, link";

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Drops all sync state (used on sign-out or when the sync folder changes).
    pub fn reset(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM meta; DELETE FROM remote; DELETE FROM synced; DELETE FROM upload_sessions;")?;
        Ok(())
    }

    pub fn transaction<T>(&mut self, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN")?;
        match f(self) {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    // ----- meta -----

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    // ----- remote tree -----

    pub fn upsert_remote(&self, item: &RemoteItem) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT INTO remote ({REMOTE_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET parent_id = excluded.parent_id, name = excluded.name,
                   mime = excluded.mime, md5 = excluded.md5, size = excluded.size,
                   mtime = excluded.mtime, link = excluded.link"
            ),
            params![
                item.id,
                item.parent_id,
                item.name,
                item.mime,
                item.md5,
                item.size as i64,
                item.mtime.map(|t| t.to_rfc3339()),
                item.link
            ],
        )?;
        Ok(())
    }

    pub fn delete_remote(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM remote WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear_remote(&self) -> Result<()> {
        self.conn.execute("DELETE FROM remote", [])?;
        Ok(())
    }

    pub fn get_remote(&self, id: &str) -> Result<Option<RemoteItem>> {
        let mut stmt = self.conn.prepare_cached(&format!("SELECT {REMOTE_COLS} FROM remote WHERE id = ?1"))?;
        Ok(stmt.query_row([id], RemoteItem::from_row).optional()?)
    }

    pub fn remote_snapshot(&self) -> Result<RemoteSnapshot> {
        let mut stmt = self.conn.prepare(&format!("SELECT {REMOTE_COLS} FROM remote ORDER BY id"))?;
        let mut snap = RemoteSnapshot { items: HashMap::new(), by_name: HashMap::new() };
        for item in stmt.query_map([], RemoteItem::from_row)? {
            let item = item?;
            if let Some(parent) = &item.parent_id {
                let siblings = snap.by_name.entry(parent.clone()).or_default();
                siblings.entry(item.name.clone()).or_default().push(item.id.clone());
            }
            snap.items.insert(item.id.clone(), item);
        }
        Ok(snap)
    }

    pub fn remote_children(&self, parent_id: &str) -> Result<Vec<RemoteItem>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {REMOTE_COLS} FROM remote WHERE parent_id = ?1 ORDER BY id"))?;
        let rows = stmt.query_map([parent_id], RemoteItem::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Children of `parent_id` called exactly `name`, ordered by id.
    pub fn remote_named(&self, parent_id: &str, name: &str) -> Result<Vec<RemoteItem>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {REMOTE_COLS} FROM remote WHERE parent_id = ?1 AND name = ?2 ORDER BY id"
        ))?;
        let rows = stmt.query_map([parent_id, name], RemoteItem::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn all_remote_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM remote")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ----- synced tree -----

    pub fn upsert_synced(&self, item: &SyncedItem) -> Result<()> {
        // A stale row may still claim this path (e.g. an item replaced in place);
        // the path belongs to the item being written now.
        self.conn
            .execute("DELETE FROM synced WHERE rel_path = ?1 AND id != ?2", params![path_bytes(&item.rel_path), item.id])?;
        self.conn.execute(
            &format!(
                "INSERT INTO synced ({SYNCED_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET rel_path = excluded.rel_path, is_dir = excluded.is_dir,
                   md5 = excluded.md5, size = excluded.size, mtime_ns = excluded.mtime_ns, inode = excluded.inode"
            ),
            params![
                item.id,
                path_bytes(&item.rel_path),
                item.is_dir,
                item.md5,
                item.size as i64,
                item.mtime_ns,
                item.inode as i64
            ],
        )?;
        Ok(())
    }

    pub fn get_synced(&self, id: &str) -> Result<Option<SyncedItem>> {
        let mut stmt = self.conn.prepare_cached(&format!("SELECT {SYNCED_COLS} FROM synced WHERE id = ?1"))?;
        Ok(stmt.query_row([id], SyncedItem::from_row).optional()?)
    }

    pub fn synced_by_path(&self, rel: &Path) -> Result<Option<SyncedItem>> {
        let mut stmt = self.conn.prepare_cached(&format!("SELECT {SYNCED_COLS} FROM synced WHERE rel_path = ?1"))?;
        Ok(stmt.query_row([path_bytes(rel)], SyncedItem::from_row).optional()?)
    }

    pub fn synced_by_inode(&self, inode: u64) -> Result<Vec<SyncedItem>> {
        let mut stmt = self.conn.prepare_cached(&format!("SELECT {SYNCED_COLS} FROM synced WHERE inode = ?1"))?;
        let rows = stmt.query_map([inode as i64], SyncedItem::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Strict descendants of `dir`, shallowest first.
    pub fn synced_descendants(&self, dir: &Path) -> Result<Vec<SyncedItem>> {
        let (lo, hi) = subtree_range(dir);
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {SYNCED_COLS} FROM synced WHERE rel_path > ?1 AND rel_path < ?2 ORDER BY rel_path"
        ))?;
        let rows = stmt.query_map(params![lo, hi], SyncedItem::from_row)?;
        let mut items: Vec<SyncedItem> = rows.collect::<rusqlite::Result<_>>()?;
        items.sort_by_key(|i| i.rel_path.components().count());
        Ok(items)
    }

    pub fn all_synced(&self) -> Result<Vec<SyncedItem>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {SYNCED_COLS} FROM synced"))?;
        let rows = stmt.query_map([], SyncedItem::from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn synced_count(&self) -> Result<usize> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM synced", [], |r| r.get::<_, i64>(0))? as usize)
    }

    pub fn delete_synced(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM synced WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Removes `dir` and everything below it from the synced tree.
    pub fn delete_synced_subtree(&self, dir: &Path) -> Result<()> {
        let (lo, hi) = subtree_range(dir);
        self.conn.execute(
            "DELETE FROM synced WHERE rel_path = ?1 OR (rel_path > ?2 AND rel_path < ?3)",
            params![path_bytes(dir), lo, hi],
        )?;
        Ok(())
    }

    /// Re-roots `old` and all its descendants at `new` after a move.
    pub fn move_synced_subtree(&mut self, old: &Path, new: &Path) -> Result<()> {
        self.transaction(|db| {
            let descendants = db.synced_descendants(old)?;
            if let Some(mut item) = db.synced_by_path(old)? {
                item.rel_path = new.to_path_buf();
                db.upsert_synced(&item)?;
            }
            for mut item in descendants {
                let tail = item.rel_path.strip_prefix(old).expect("descendant").to_path_buf();
                item.rel_path = new.join(tail);
                db.upsert_synced(&item)?;
            }
            Ok(())
        })
    }

    // ----- upload sessions -----

    pub fn get_upload_session(&self, rel: &Path) -> Result<Option<UploadSession>> {
        Ok(self
            .conn
            .query_row(
                "SELECT rel_path, url, file_id, parent_id, name, size, mtime_ns, created FROM upload_sessions WHERE rel_path = ?1",
                [path_bytes(rel)],
                |row| {
                    Ok(UploadSession {
                        rel_path: bytes_to_path(row.get(0)?),
                        url: row.get(1)?,
                        file_id: row.get(2)?,
                        parent_id: row.get(3)?,
                        name: row.get(4)?,
                        size: row.get::<_, i64>(5)? as u64,
                        mtime_ns: row.get(6)?,
                        created: DateTime::from_timestamp(row.get(7)?, 0).unwrap_or_default(),
                    })
                },
            )
            .optional()?)
    }

    pub fn put_upload_session(&self, s: &UploadSession) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO upload_sessions (rel_path, url, file_id, parent_id, name, size, mtime_ns, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![path_bytes(&s.rel_path), s.url, s.file_id, s.parent_id, s.name, s.size as i64, s.mtime_ns, s.created.timestamp()],
        )?;
        Ok(())
    }

    pub fn delete_upload_session(&self, rel: &Path) -> Result<()> {
        self.conn.execute("DELETE FROM upload_sessions WHERE rel_path = ?1", [path_bytes(rel)])?;
        Ok(())
    }

    /// Forgets sessions started before `cutoff` (Drive has expired them by now).
    pub fn prune_upload_sessions(&self, cutoff: DateTime<Utc>) -> Result<()> {
        self.conn.execute("DELETE FROM upload_sessions WHERE created < ?1", [cutoff.timestamp()])?;
        Ok(())
    }
}

impl RemoteLookup for Db {
    fn get_remote(&self, id: &str) -> Result<Option<RemoteItem>> {
        Db::get_remote(self, id)
    }

    fn remote_named(&self, parent_id: &str, name: &str) -> Result<Vec<RemoteItem>> {
        Db::remote_named(self, parent_id, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, path: &str) -> SyncedItem {
        SyncedItem { id: id.into(), rel_path: path.into(), is_dir: false, md5: None, size: 0, mtime_ns: 0, inode: 1 }
    }

    #[test]
    fn subtree_queries_do_not_match_siblings_with_common_prefix() {
        let db = Db::in_memory().unwrap();
        for (id, p) in [("1", "a"), ("2", "a/x"), ("3", "a/x/y"), ("4", "ab"), ("5", "a.txt"), ("6", "a0")] {
            db.upsert_synced(&item(id, p)).unwrap();
        }
        let ids: Vec<_> = db.synced_descendants(Path::new("a")).unwrap().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["2", "3"]);
        db.delete_synced_subtree(Path::new("a")).unwrap();
        assert_eq!(db.synced_count().unwrap(), 3);
    }

    #[test]
    fn upload_sessions_round_trip_and_prune() {
        let db = Db::in_memory().unwrap();
        let created = Utc::now() - chrono::Duration::days(3);
        let s = UploadSession {
            rel_path: "dir/big.bin".into(),
            url: "https://upload.example/s1".into(),
            file_id: None,
            parent_id: "p".into(),
            name: "big.bin".into(),
            size: 42,
            mtime_ns: 7,
            created,
        };
        db.put_upload_session(&s).unwrap();
        let got = db.get_upload_session(Path::new("dir/big.bin")).unwrap().unwrap();
        assert_eq!(got.url, s.url);
        assert_eq!(got.created.timestamp(), created.timestamp());
        db.prune_upload_sessions(Utc::now() - chrono::Duration::days(4)).unwrap();
        assert!(db.get_upload_session(Path::new("dir/big.bin")).unwrap().is_some());
        db.prune_upload_sessions(Utc::now() - chrono::Duration::days(2)).unwrap();
        assert!(db.get_upload_session(Path::new("dir/big.bin")).unwrap().is_none());
    }

    #[test]
    fn move_subtree_rewrites_descendant_paths() {
        let mut db = Db::in_memory().unwrap();
        for (id, p) in [("1", "a"), ("2", "a/x"), ("3", "a/x/y")] {
            db.upsert_synced(&item(id, p)).unwrap();
        }
        db.move_synced_subtree(Path::new("a"), Path::new("b/c")).unwrap();
        assert_eq!(db.get_synced("3").unwrap().unwrap().rel_path, Path::new("b/c/x/y"));
        assert_eq!(db.get_synced("1").unwrap().unwrap().rel_path, Path::new("b/c"));
    }
}
