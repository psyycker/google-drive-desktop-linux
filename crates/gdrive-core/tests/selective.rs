//! Syncing only some Drive folders ("selected folders").

mod support;

use std::path::Path;

use gdrive_core::config::{Config, SelectedFolder};
use gdrive_core::db::Db;
use gdrive_core::selection::DriveFolders;
use support::{Harness, ROOT};

const T: u64 = 20;

/// The harness config, syncing only the given `(id, path)` folders.
fn selecting(h: &Harness, folders: &[(&str, &str)]) -> Config {
    let selected = folders.iter().map(|(id, path)| SelectedFolder { id: id.to_string(), path: path.into() }).collect();
    Config { selected_folders: Some(selected), ..h.config() }
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_synced_until_folders_are_selected() {
    support::init_logs();
    let mut h = Harness::new().await;
    let docs = h.drive.add_folder("Docs", ROOT);
    h.drive.add_file("a.txt", &docs, b"a");
    h.drive.add_file("root.txt", ROOT, b"r");
    h.write("mine.txt", b"local");

    h.start_with(Config { selected_folders: Some(Vec::new()), ..h.config() });
    h.settle().await;
    assert_eq!(h.local_tree(), ["mine.txt"]);
    assert_eq!(h.drive.counters().downloads, 0);
    assert!(h.drive.find("mine.txt").is_none(), "nothing outside the selection is uploaded");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_selected_folders_sync_both_ways() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = &h.drive;
    let work = d.add_folder("Work", ROOT);
    let proj = d.add_folder("Proj", &work);
    d.add_file("b.txt", &proj, b"b");
    d.add_file("other.txt", &work, b"o");
    let docs = d.add_folder("Docs", ROOT);
    d.add_file("a.txt", &docs, b"a");
    d.add_file("root.txt", ROOT, b"r");

    h.start_with(selecting(&h, &[(&proj, "Work/Proj")]));
    h.wait("selected folder downloaded", T, |h| h.read("Work/Proj/b.txt").as_deref() == Some(b"b")).await;
    h.settle().await;
    // The folders on the way to a selected one exist, but nothing else in them.
    assert_eq!(h.local_tree(), ["Work", "Work/Proj", "Work/Proj/b.txt"]);

    {
        let db = Db::open(&h.db_path).unwrap();
        let drive = DriveFolders::new(&db).unwrap();
        assert_eq!(drive.resolve(Path::new("Work/Proj")).unwrap(), proj);
        assert_eq!(drive.path_of(&proj).unwrap().unwrap(), Path::new("Work/Proj"));
        assert!(drive.resolve(Path::new("Work/Nope")).is_err());
        let names: Vec<String> = drive.subfolders(ROOT).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["Docs", "Work"]);
    }

    // Local changes upload only inside the selected folder.
    h.write("Work/Proj/new.txt", b"n");
    h.write("Work/side.txt", b"s");
    h.write("loose.txt", b"l");
    std::fs::create_dir(h.path("Fresh")).unwrap();
    h.wait("new file uploaded", T, |h| h.drive.content("Work/Proj/new.txt").as_deref() == Some(b"n")).await;
    h.settle().await;
    assert!(h.drive.find("Work/side.txt").is_none());
    assert!(h.drive.find("loose.txt").is_none());
    assert!(h.drive.find("Fresh").is_none());

    // Drive changes download only inside it.
    h.drive.add_file("c.txt", &proj, b"c");
    h.drive.add_file("d.txt", &docs, b"d");
    h.drive.add_file("e.txt", &work, b"e");
    h.wait("remote file downloaded", T, |h| h.read("Work/Proj/c.txt").as_deref() == Some(b"c")).await;
    h.settle().await;
    assert!(!h.path("Docs").exists());
    assert!(!h.path("Work/e.txt").exists());
    assert_eq!(h.drive.counters().trashes, 0);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unselecting_removes_local_copies_but_keeps_drive_and_local_edits() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = &h.drive;
    let work = d.add_folder("Work", ROOT);
    d.add_file("w.txt", &work, b"w");
    let docs = d.add_folder("Docs", ROOT);
    d.add_file("a.txt", &docs, b"a");
    let old = d.add_folder("Old", &docs);
    d.add_file("x.txt", &old, b"x");
    d.add_file("root.txt", ROOT, b"r");

    h.start();
    h.wait("everything downloaded", T, |h| h.read("Docs/Old/x.txt").is_some() && h.read("root.txt").is_some()).await;
    h.settle().await;
    h.stop().await;
    // Changed while not syncing, so never uploaded.
    h.write("Docs/a.txt", b"edited here");

    h.start_with(selecting(&h, &[(&work, "Work")]));
    h.wait("unselected copies removed", T, |h| !h.path("Docs/Old").exists() && !h.path("root.txt").exists()).await;
    h.settle().await;
    assert_eq!(h.local_tree(), ["Docs", "Docs/a.txt", "Work", "Work/w.txt"]);
    assert_eq!(h.read("Docs/a.txt").unwrap(), b"edited here");
    assert_eq!(h.drive.content("Docs/a.txt").unwrap(), b"a", "the local edit is not uploaded");
    assert_eq!(h.drive.content("Docs/Old/x.txt").unwrap(), b"x");
    assert_eq!(h.drive.content("root.txt").unwrap(), b"r");
    assert_eq!(h.drive.counters().trashes, 0);
    h.stop().await;

    // Selecting it again brings Drive's version back and keeps the edit beside it.
    h.start_with(selecting(&h, &[(&work, "Work"), (&docs, "Docs")]));
    h.wait("folder downloaded again", T, |h| h.read("Docs/Old/x.txt").is_some()).await;
    h.settle().await;
    assert_eq!(h.read("Docs/a.txt").unwrap(), b"a");
    let copies: Vec<String> = h.local_tree().into_iter().filter(|p| p.starts_with("Docs/a (conflicted copy")).collect();
    assert_eq!(copies.len(), 1, "{:?}", h.local_tree());
    assert_eq!(h.read(&copies[0]).unwrap(), b"edited here");
    assert_eq!(h.drive.counters().trashes, 0);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn selection_follows_folders_renamed_or_moved_on_either_side() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = &h.drive;
    let work = d.add_folder("Work", ROOT);
    let file = d.add_file("w.txt", &work, b"w");
    d.add_file("keep.txt", &work, b"k");
    let archive = d.add_folder("Archive", ROOT);

    h.start_with(selecting(&h, &[(&work, "Work")]));
    h.wait("selected folder downloaded", T, |h| h.read("Work/w.txt").is_some()).await;
    h.settle().await;
    let downloads = h.drive.counters().downloads;

    // Renamed on Drive: it moves locally and stays synced.
    h.drive.rename(&work, "Job");
    h.wait("rename mirrored", T, |h| h.read("Job/w.txt").is_some() && !h.path("Work").exists()).await;
    h.settle().await;
    h.drive.add_file("new.txt", &work, b"n");
    h.wait("still synced", T, |h| h.read("Job/new.txt").is_some()).await;

    // Renamed locally: Drive follows, and it stays synced.
    std::fs::rename(h.path("Job"), h.path("Gig")).unwrap();
    h.wait("rename uploaded", T, |h| h.drive.get(&work).unwrap().name == "Gig").await;
    h.settle().await;
    h.write("Gig/later.txt", b"l");
    h.wait("still uploading", T, |h| h.drive.content("Gig/later.txt").is_some()).await;
    assert_eq!(h.drive.counters().downloads, downloads + 1, "only new.txt was downloaded");

    // A file moved on Drive out of the selection is removed here, not trashed there.
    h.drive.move_to(&file, &archive);
    h.wait("moved-out file removed", T, |h| !h.path("Gig/w.txt").exists()).await;
    h.settle().await;
    assert_eq!(h.drive.content("Archive/w.txt").unwrap(), b"w");

    // A file moved locally out of the selection moves on Drive too and stays here.
    std::fs::rename(h.path("Gig/keep.txt"), h.path("keep.txt")).unwrap();
    h.wait("move uploaded", T, |h| h.drive.find("keep.txt").is_some()).await;
    h.settle().await;
    assert_eq!(h.read("keep.txt").unwrap(), b"k");
    h.write("keep.txt", b"changed");
    h.settle().await;
    assert_eq!(h.drive.content("keep.txt").unwrap(), b"k", "no longer synced");
    assert_eq!(h.drive.counters().trashes, 0);
    h.stop().await;
}
