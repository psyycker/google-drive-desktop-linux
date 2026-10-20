//! Drive folders kept on another disk ("storage locations").

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gdrive_core::config::{Config, Placement, StorageLocation};
use gdrive_core::db::Db;
use gdrive_core::relocate::{relocate, MoveProgress};
use gdrive_core::status::SyncState;
use support::{Harness, ROOT};

const T: u64 = 20;

/// A second "disk" next to the harness, and a config that knows about it.
struct Disk2 {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Disk2 {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Disk2");
        std::fs::create_dir_all(&root).unwrap();
        Self { _tmp: tmp, root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn config(&self, h: &Harness, placed: &[&str]) -> Config {
        Config {
            locations: vec![StorageLocation { name: "SSD".into(), root: self.root.clone() }],
            placements: placed.iter().map(|f| Placement { folder: f.into(), location: "SSD".into() }).collect(),
            ..h.config()
        }
    }
}

fn read(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}

/// Runs a move with the engine stopped, the way the daemon does, and returns the
/// config it saved.
fn move_folder(h: &Harness, config: &Config, folder: &str, to: Option<&str>) -> Config {
    let db = Db::open(&h.db_path).unwrap();
    let mut saved = None;
    relocate(config, &db, Path::new(folder), to, &MoveProgress::default(), &mut |c| {
        saved = Some(c.clone());
        Ok(())
    })
    .unwrap();
    saved.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn placed_folder_syncs_on_the_other_disk() {
    support::init_logs();
    let mut h = Harness::new().await;
    let disk = Disk2::new();
    let d = h.drive.clone();
    let gaming = d.add_folder("Gaming", ROOT);
    let a = d.add_file("a.txt", &gaming, b"a1");
    let docs = d.add_folder("Docs", ROOT);
    d.add_file("b.txt", &docs, b"b");
    let config = disk.config(&h, &["Gaming"]);
    h.start_with(config.clone());

    h.wait("download to disk 2", T, |_| read(&disk.path("Gaming/a.txt")).as_deref() == Some(b"a1")).await;
    h.wait("rest under the root", T, |h| h.read("Docs/b.txt").is_some()).await;
    h.settle().await;
    assert_eq!(std::fs::read_link(h.path("Gaming")).unwrap(), disk.path("Gaming"));
    assert_eq!(d.counters().uploads, 0);
    assert_eq!(d.counters().trashes, 0);

    // Local edits on the other disk, directly and through the placeholder link.
    std::fs::write(disk.path("Gaming/a.txt"), b"a2").unwrap();
    h.wait("upload edit", T, |h| h.drive.content("Gaming/a.txt").as_deref() == Some(b"a2")).await;
    std::fs::write(h.path("Gaming/via-link.txt"), b"v").unwrap();
    h.wait("upload via link", T, |h| h.drive.content("Gaming/via-link.txt").as_deref() == Some(b"v")).await;
    assert!(disk.path("Gaming/via-link.txt").exists());

    // Remote changes land on the other disk.
    d.edit(&a, b"a3");
    d.add_file("c.txt", &gaming, b"c");
    h.wait("remote edit", T, |_| read(&disk.path("Gaming/a.txt")).as_deref() == Some(b"a3")).await;
    h.wait("remote add", T, |_| read(&disk.path("Gaming/c.txt")).as_deref() == Some(b"c")).await;

    // Local delete propagates.
    std::fs::remove_file(disk.path("Gaming/c.txt")).unwrap();
    h.wait("trash on Drive", T, |h| h.drive.find("Gaming/c.txt").is_none_or(|f| f.trashed)).await;
    h.settle().await;

    // A restart with the placeholder in place changes nothing on Drive.
    let before = d.counters();
    h.stop().await;
    h.start_with(config);
    h.settle().await;
    let after = d.counters();
    assert_eq!((after.uploads, after.trashes, after.writes), (before.uploads, before.trashes, before.writes));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_a_synced_folder_never_touches_drive() {
    support::init_logs();
    let mut h = Harness::new().await;
    let disk = Disk2::new();
    let d = h.drive.clone();
    let gaming = d.add_folder("Gaming", ROOT);
    let sub = d.add_folder("Saves", &gaming);
    d.add_file("a.txt", &gaming, b"a");
    d.add_file("s.dat", &sub, b"s");
    let config = disk.config(&h, &[]);
    h.start_with(config.clone());
    h.wait("initial", T, |h| h.read("Gaming/Saves/s.dat").is_some()).await;
    h.settle().await;
    let before = d.counters();

    h.stop().await;
    let there = move_folder(&h, &config, "Gaming", Some("SSD"));
    h.start_with(there.clone());
    h.settle().await;
    assert_eq!(read(&disk.path("Gaming/Saves/s.dat")).as_deref(), Some(&b"s"[..]));
    assert_eq!(std::fs::read_link(h.path("Gaming")).unwrap(), disk.path("Gaming"));
    let mid = d.counters();
    assert_eq!((mid.uploads, mid.trashes, mid.writes), (before.uploads, before.trashes, before.writes));

    // Still synced from its new home.
    std::fs::write(disk.path("Gaming/new.txt"), b"n").unwrap();
    h.wait("upload from disk 2", T, |h| h.drive.content("Gaming/new.txt").as_deref() == Some(b"n")).await;
    h.settle().await;
    let before = d.counters();

    h.stop().await;
    let back = move_folder(&h, &there, "Gaming", None);
    assert!(back.placements.is_empty());
    h.start_with(back);
    h.settle().await;
    assert!(std::fs::symlink_metadata(h.path("Gaming")).unwrap().is_dir());
    assert_eq!(h.read("Gaming/new.txt").as_deref(), Some(&b"n"[..]));
    assert!(!disk.path("Gaming").exists());
    let after = d.counters();
    assert_eq!((after.uploads, after.trashes, after.writes), (before.uploads, before.trashes, before.writes));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_second_disk_never_deletes_remote_files() {
    support::init_logs();
    let mut h = Harness::new().await;
    let disk = Disk2::new();
    let d = h.drive.clone();
    let gaming = d.add_folder("Gaming", ROOT);
    d.add_file("a.txt", &gaming, b"a");
    let config = disk.config(&h, &["Gaming"]);
    h.start_with(config.clone());
    h.wait("initial", T, |_| disk.path("Gaming/a.txt").exists()).await;
    h.settle().await;
    h.stop().await;

    // The disk is unplugged: its folder (and marker) are gone.
    std::fs::remove_dir_all(&disk.root).unwrap();
    std::fs::create_dir_all(&disk.root).unwrap();
    h.start_with(config);
    h.wait("error state", T, |h| h.state() == SyncState::Error).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(d.counters().trashes, 0);
    assert_eq!(d.counters().writes, 0);
    assert!(h.status().message.unwrap_or_default().contains("missing or not mounted"));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_move_into_a_placed_folder_lands_on_the_other_disk() {
    support::init_logs();
    let mut h = Harness::new().await;
    let disk = Disk2::new();
    let d = h.drive.clone();
    let gaming = d.add_folder("Gaming", ROOT);
    let x = d.add_file("x.txt", ROOT, b"x");
    h.start_with(disk.config(&h, &["Gaming"]));
    h.wait("initial", T, |h| h.read("x.txt").is_some() && disk.path("Gaming").is_dir()).await;
    h.settle().await;

    d.move_to(&x, &gaming);
    h.wait("moved to disk 2", T, |_| read(&disk.path("Gaming/x.txt")).as_deref() == Some(b"x")).await;
    h.settle().await;
    assert!(h.read("x.txt").is_none());
    assert_eq!(d.counters().uploads, 0);
    assert_eq!(d.counters().trashes, 0);
    h.stop().await;
}
