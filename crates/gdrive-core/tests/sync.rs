//! End-to-end tests: the real engine against a fake Drive, on a temp folder.

mod support;

use std::time::Duration;

use gdrive_core::engine::Command;
use gdrive_core::status::SyncState;
use support::{mtime, Harness, ROOT};

const T: u64 = 20;

#[tokio::test(flavor = "multi_thread")]
async fn initial_sync_mirrors_remote_tree_and_adopts_local_files() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = &h.drive;
    let docs = d.add_folder("Docs", ROOT);
    d.add_file("a.txt", &docs, b"hello");
    let sub = d.add_folder("Sub", &docs);
    let bin: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
    d.add_file("b.bin", &sub, &bin);
    let plan = d.add_doc("Plan", &docs);
    d.add_file("dup.txt", ROOT, b"first");
    d.add_file("dup.txt", ROOT, b"second");
    d.add_file("x/y.txt", ROOT, b"slash");
    d.add_file("same.txt", ROOT, b"identical");
    d.add_file("diff.txt", ROOT, b"remote version");
    d.add_folder("Empty", ROOT);

    h.write("same.txt", b"identical");
    h.write("diff.txt", b"local version");
    h.write("Docs/local-only.txt", b"mine");

    h.start();
    h.wait("remote tree downloaded", T, |h| h.read("Docs/Sub/b.bin").as_deref() == Some(&bin[..])).await;
    h.settle().await;

    assert_eq!(h.read("Docs/a.txt").unwrap(), b"hello");
    let m = mtime(&h.path("Docs/a.txt"));
    assert_eq!(m.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), "2024-03-01T12:34:56.789Z");
    let link = String::from_utf8(h.read("Docs/Plan.gdoc").unwrap()).unwrap();
    assert!(link.contains(&plan), "{link}");
    assert_eq!(h.read("dup.txt").unwrap(), b"first");
    assert_eq!(h.read("dup (2).txt").unwrap(), b"second");
    assert_eq!(h.read("x∕y.txt").unwrap(), b"slash");
    assert!(h.path("Empty").is_dir());
    assert_eq!(h.read("same.txt").unwrap(), b"identical");
    assert_eq!(h.read("diff.txt").unwrap(), b"remote version");

    // The differing local file was kept as a conflicted copy and uploaded.
    let copies: Vec<String> = h.local_tree().into_iter().filter(|p| p.starts_with("diff (conflicted copy")).collect();
    assert_eq!(copies.len(), 1, "{:?}", h.local_tree());
    assert_eq!(h.read(&copies[0]).unwrap(), b"local version");
    assert_eq!(h.drive.content(&copies[0]).unwrap(), b"local version");
    assert_eq!(h.drive.content("Docs/local-only.txt").unwrap(), b"mine");
    // Only the conflicted copy and the local-only file were uploaded; same.txt was adopted.
    assert_eq!(h.drive.counters().uploads, 2);
    assert_eq!(h.drive.counters().trashes, 0);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_changes_propagate_to_drive() {
    support::init_logs();
    let mut h = Harness::new().await;
    h.start();
    h.settle().await;

    // New file.
    h.write("new.txt", b"v1");
    h.wait("upload new.txt", T, |h| h.drive.content("new.txt").as_deref() == Some(b"v1")).await;
    let id = h.drive.find("new.txt").unwrap().id;

    // Nested folder moved in from outside the sync root in one rename.
    let outside = h.root.parent().unwrap().join("outside");
    std::fs::create_dir_all(outside.join("tree/a/b")).unwrap();
    std::fs::write(outside.join("tree/a/b/deep.txt"), b"deep").unwrap();
    std::fs::write(outside.join("tree/top.txt"), b"top").unwrap();
    std::fs::rename(outside.join("tree"), h.path("tree")).unwrap();
    h.wait("upload tree", T, |h| {
        h.drive.content("tree/a/b/deep.txt").as_deref() == Some(b"deep") && h.drive.content("tree/top.txt").is_some()
    })
    .await;

    // Folders that arrived via a move are watched too.
    h.write("tree/a/b/deep.txt", b"deeper");
    h.wait("upload deep edit", T, |h| h.drive.content("tree/a/b/deep.txt").as_deref() == Some(b"deeper")).await;

    // Edit keeps the same Drive file.
    h.write("new.txt", b"version two");
    h.wait("upload edit", T, |h| h.drive.content("new.txt").as_deref() == Some(b"version two")).await;
    assert_eq!(h.drive.find("new.txt").unwrap().id, id);
    h.settle().await;
    let uploads = h.drive.counters().uploads;

    // Rename and move are metadata-only.
    std::fs::rename(h.path("new.txt"), h.path("renamed.txt")).unwrap();
    h.wait("remote rename", T, |h| h.drive.find("renamed.txt").is_some_and(|f| f.id == id)).await;
    std::fs::rename(h.path("renamed.txt"), h.path("tree/a/renamed.txt")).unwrap();
    h.wait("remote move", T, |h| h.drive.find("tree/a/renamed.txt").is_some_and(|f| f.id == id)).await;
    let tree_id = h.drive.find("tree").unwrap().id;
    std::fs::rename(h.path("tree"), h.path("forest")).unwrap();
    h.wait("remote folder rename", T, |h| h.drive.find("forest").is_some_and(|f| f.id == tree_id)).await;
    assert_eq!(h.drive.find("forest/a/renamed.txt").unwrap().id, id);
    h.settle().await;
    assert_eq!(h.drive.counters().uploads, uploads, "renames must not re-upload");
    assert_eq!(h.drive.counters().trashes, 0, "renames must not trash");

    // Deletes go to the Drive trash.
    std::fs::remove_file(h.path("forest/a/renamed.txt")).unwrap();
    h.wait("remote trash file", T, |h| h.drive.get(&id).unwrap().trashed).await;
    std::fs::remove_dir_all(h.path("forest")).unwrap();
    h.wait("remote trash folder", T, |h| h.drive.get(&tree_id).unwrap().trashed).await;
    h.settle().await;
    assert_eq!(h.drive.children(ROOT), Vec::<String>::new());
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_changes_propagate_locally() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let f = d.add_file("note.txt", ROOT, b"one");
    let folder = d.add_folder("Folder", ROOT);
    let inner = d.add_file("inner.txt", &folder, b"inner");
    h.start();
    h.wait("initial", T, |h| h.read("Folder/inner.txt").is_some()).await;
    h.settle().await;

    // Folders the engine created are watched: local edits inside get uploaded.
    h.write("Folder/inner.txt", b"edited here");
    h.wait("upload inside downloaded folder", T, |h| h.drive.get(&inner).unwrap().content == b"edited here").await;
    h.settle().await;
    let writes_before_remote = h.drive.counters().writes;

    d.edit(&f, b"two");
    h.wait("remote edit", T, |h| h.read("note.txt").as_deref() == Some(b"two")).await;
    d.rename(&f, "renamed.txt");
    h.wait("remote rename", T, |h| h.read("renamed.txt").is_some() && h.read("note.txt").is_none()).await;
    d.move_to(&f, &folder);
    h.wait("remote move", T, |h| h.read("Folder/renamed.txt").as_deref() == Some(b"two")).await;
    d.rename(&folder, "Moved");
    h.wait("remote folder rename", T, |h| h.read("Moved/inner.txt").is_some() && !h.path("Folder").exists()).await;
    let new_doc = d.add_doc("Sheet thing", &folder);
    h.wait("new doc", T, |h| h.read("Moved/Sheet thing.gdoc").is_some()).await;
    d.trash(&inner);
    h.wait("remote trash", T, |h| h.read("Moved/inner.txt").is_none()).await;
    d.trash(&folder);
    h.wait("remote folder trash", T, |h| !h.path("Moved").exists()).await;
    h.settle().await;
    assert_eq!(h.local_tree(), Vec::<String>::new());
    let c = h.drive.counters();
    assert_eq!(c.writes, writes_before_remote, "mirroring remote changes must not write to Drive: {c:?}");
    let _ = new_doc;
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicts_and_restart_converge() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let both = d.add_file("both.txt", ROOT, b"base");
    let gone_remote = d.add_file("gone-remote.txt", ROOT, b"base");
    let gone_local = d.add_file("gone-local.txt", ROOT, b"base");
    let plain = d.add_file("plain.txt", ROOT, b"base");
    h.start();
    h.wait("initial", T, |h| h.read("plain.txt").is_some()).await;
    h.settle().await;
    h.stop().await;

    // Offline edits on both sides.
    h.write("both.txt", b"local edit");
    d.edit(&both, b"remote edit");
    h.write("gone-remote.txt", b"local edit");
    d.trash(&gone_remote);
    std::fs::remove_file(h.path("gone-local.txt")).unwrap();
    d.edit(&gone_local, b"remote edit");
    h.write("offline-new.txt", b"made offline");
    d.add_file("remote-new.txt", ROOT, b"made remotely");
    std::fs::remove_file(h.path("plain.txt")).unwrap();

    h.start();
    h.wait("converged", T, |h| {
        h.read("both.txt").as_deref() == Some(b"remote edit")
            && h.read("gone-local.txt").as_deref() == Some(b"remote edit")
            && h.drive.content("gone-remote.txt").as_deref() == Some(b"local edit")
            && h.drive.content("offline-new.txt").is_some()
            && h.read("remote-new.txt").is_some()
            && h.drive.get(&plain).unwrap().trashed
    })
    .await;
    h.settle().await;

    let copies: Vec<String> = h.local_tree().into_iter().filter(|p| p.starts_with("both (conflicted copy")).collect();
    assert_eq!(copies.len(), 1, "{:?}", h.local_tree());
    assert_eq!(h.read(&copies[0]).unwrap(), b"local edit");
    assert_eq!(h.drive.content(&copies[0]).unwrap(), b"local edit");
    assert_eq!(h.drive.content("both.txt").unwrap(), b"remote edit");
    assert_eq!(h.read("gone-remote.txt").unwrap(), b"local edit");
    assert!(!h.drive.get(&gone_local).unwrap().trashed);

    // Both sides now list the same names.
    let mut remote = h.drive.children(ROOT);
    remote.sort();
    assert_eq!(h.local_tree(), remote);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settled_engine_is_quiet() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let folder = d.add_folder("F", ROOT);
    d.add_file("a.txt", &folder, b"a");
    d.add_doc("Doc", ROOT);
    h.start();
    h.wait("initial", T, |h| h.read("F/a.txt").is_some()).await;
    h.write("F/b.txt", b"b");
    std::fs::create_dir(h.path("G")).unwrap();
    h.wait("uploads", T, |h| h.drive.find("F/b.txt").is_some() && h.drive.find("G").is_some()).await;
    h.settle().await;

    let before = h.drive.counters();
    h.send(Command::SyncNow);
    tokio::time::sleep(Duration::from_secs(4)).await;
    let after = h.drive.counters();
    assert_eq!(before.writes, after.writes, "echoes caused writes: {before:?} -> {after:?}");
    assert_eq!(before.downloads, after.downloads, "echoes caused downloads: {before:?} -> {after:?}");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_sync_root_never_deletes_remote_files() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let folder = d.add_folder("F", ROOT);
    d.add_file("a.txt", &folder, b"a");
    d.add_file("b.txt", ROOT, b"b");
    h.start();
    h.wait("initial", T, |h| h.read("F/a.txt").is_some()).await;
    h.settle().await;

    // Marker gone (e.g. a different disk mounted there) — must stop, not delete.
    std::fs::remove_dir_all(h.path(".gdrive-linux")).unwrap();
    std::fs::remove_file(h.path("b.txt")).unwrap();
    h.wait("error state", T, |h| h.state() == SyncState::Error).await;

    // Whole root gone.
    std::fs::remove_dir_all(&h.root).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(h.drive.counters().trashes, 0);
    assert_eq!(h.drive.counters().writes, 0);
    assert_eq!(h.state(), SyncState::Error);
    h.stop().await;

    // A restart doesn't silently recreate the folder and trash everything either.
    h.start();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(h.drive.counters().trashes, 0);
    assert_ne!(h.state(), SyncState::Idle);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_api_failures_are_retried() {
    support::init_logs();
    let mut h = Harness::new().await;
    h.drive.add_file("a.txt", ROOT, b"a");
    h.drive.fail_next(2);
    h.start();
    h.wait("initial despite 503s", 30, |h| h.read("a.txt").is_some()).await;
    h.settle().await;
    h.drive.fail_next(3);
    h.write("b.txt", b"b");
    h.wait("upload despite 503s", 30, |h| h.drive.content("b.txt").is_some()).await;
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn edge_cases() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let folder = d.add_folder("Restorable", ROOT);
    let sub = d.add_folder("Sub", &folder);
    d.add_file("kept.txt", &sub, b"kept");
    let doc = d.add_doc("Report", ROOT);
    let doc2 = d.add_doc("Old", ROOT);
    let edited_dir = d.add_folder("Edited", ROOT);
    d.add_file("e.txt", &edited_dir, b"base");
    d.add_file("other.txt", &edited_dir, b"other");
    let mover = d.add_file("mover.txt", ROOT, b"m");
    h.start();
    h.wait("initial", T, |h| h.read("Restorable/Sub/kept.txt").is_some() && h.read("Edited/e.txt").is_some()).await;
    h.settle().await;

    // Trash + restore a folder on Drive: its contents come back too.
    d.trash(&folder);
    h.wait("trashed folder removed", T, |h| !h.path("Restorable").exists()).await;
    d.untrash(&folder);
    h.wait("restored folder contents", T, |h| h.read("Restorable/Sub/kept.txt").as_deref() == Some(b"kept")).await;

    // Renaming a link file renames the doc (without the extension); deleting it trashes the doc.
    std::fs::rename(h.path("Report.gdoc"), h.path("Final report.gdoc")).unwrap();
    h.wait("doc renamed", T, |h| h.drive.get(&doc).unwrap().name == "Final report").await;
    std::fs::remove_file(h.path("Old.gdoc")).unwrap();
    h.wait("doc trashed", T, |h| h.drive.get(&doc2).unwrap().trashed).await;

    // Folder trashed on Drive while a file inside was edited here: the local folder survives and is re-uploaded.
    h.settle().await;
    h.stop().await;
    h.write("Edited/e.txt", b"local edit");
    d.trash(&edited_dir);
    // Drive rename onto a name that's taken by a new local file.
    h.write("taken.txt", b"local taken");
    d.rename(&mover, "taken.txt");
    // Renames and deletes made while the engine was stopped.
    std::fs::rename(h.path("Restorable"), h.path("Renamed offline")).unwrap();
    h.start();
    h.wait("converged", T, |h| {
        h.drive.content("Edited/e.txt").as_deref() == Some(b"local edit")
            && h.drive.find("Renamed offline").is_some_and(|f| f.id == folder)
            && h.read("taken.txt").as_deref() == Some(b"m")
    })
    .await;
    h.settle().await;
    assert!(h.drive.find("Edited").unwrap().id != edited_dir, "re-uploaded as a new folder");
    assert!(h.local_tree().iter().any(|p| p.starts_with("taken (conflicted copy")), "{:?}", h.local_tree());
    assert_eq!(h.read("Renamed offline/Sub/kept.txt").as_deref(), Some(&b"kept"[..]));
    assert_eq!(d.find("Renamed offline/Sub/kept.txt").unwrap().content, b"kept");

    // Deleting a folder while stopped trashes it on restart.
    h.stop().await;
    std::fs::remove_dir_all(h.path("Renamed offline")).unwrap();
    h.start();
    h.wait("offline folder delete", T, |h| h.drive.get(&folder).unwrap().trashed).await;
    h.settle().await;
    let mut remote = d.children(ROOT);
    remote.retain(|n| !n.is_empty());
    let local: Vec<String> = h.local_tree().into_iter().filter(|p| !p.contains('/')).collect();
    let remote_local_names: Vec<String> =
        remote.iter().map(|n| if n == "Final report" { "Final report.gdoc".to_owned() } else { n.clone() }).collect();
    let mut a = local.clone();
    a.sort();
    let mut b = remote_local_names;
    b.sort();
    assert_eq!(a, b);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn speed_limits_throttle_transfers_and_speed_is_reported() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let mb = |seed: u8| (0..1_000_000u32).map(|i| (i as u8).wrapping_mul(seed)).collect::<Vec<u8>>();
    for (i, name) in ["a.bin", "b.bin", "c.bin"].into_iter().enumerate() {
        d.add_file(name, ROOT, &mb(i as u8 + 3));
    }
    let config = gdrive_core::config::Config { max_download_mb_per_sec: 1.0, max_upload_mb_per_sec: 0.5, ..h.config() };

    // 3 MB down at 1 MB/s, shared by 3 parallel transfers.
    let start = std::time::Instant::now();
    h.start_with(config);
    let mut peak = 0;
    h.wait("throttled downloads", T, |h| {
        peak = peak.max(h.status().download_bps);
        ["a.bin", "b.bin", "c.bin"].iter().all(|n| h.read(n).is_some())
    })
    .await;
    let took = start.elapsed().as_secs_f64();
    assert!(took >= 2.5, "3 MB at 1 MB/s finished in {took:.2}s");
    assert!((700_000..=1_400_000).contains(&peak), "reported download speed {peak} B/s");

    // 1.5 MB up at 0.5 MB/s.
    let start = std::time::Instant::now();
    h.write("up.bin", &mb(7)[..]);
    h.write("up2.bin", &mb(9)[..500_000]);
    h.wait("throttled uploads", T, |h| h.drive.find("up.bin").is_some() && h.drive.find("up2.bin").is_some()).await;
    let took = start.elapsed().as_secs_f64();
    assert!(took >= 2.5, "1.5 MB at 0.5 MB/s finished in {took:.2}s");
    assert_eq!(h.drive.content("up.bin").unwrap(), mb(7));
    h.stop().await;
}

/// Regression: a new file that can never be downloaded used to deadlock the engine
/// (re-locking the DB mutex while recording the error), freezing all syncing and
/// making the daemon hang on the next settings change.
#[tokio::test(flavor = "multi_thread")]
async fn undownloadable_file_is_reported_and_sync_continues() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let bad = d.add_file("flagged.exe", ROOT, b"nope");
    d.make_undownloadable(&bad);
    d.add_file("good.txt", ROOT, b"fine");
    h.start();
    h.wait("error reported", T, |h| h.status().errors.iter().any(|e| e.path == "flagged.exe")).await;
    let err = h.status().errors.into_iter().find(|e| e.path == "flagged.exe").unwrap();
    assert!(err.message.contains("only its owner can download it"), "{}", err.message);
    h.wait("other file synced", T, |h| h.read("good.txt").as_deref() == Some(&b"fine"[..])).await;

    // The engine must still be alive and responsive after the failure.
    h.write("after.txt", b"later");
    h.wait("upload after failure", T, |h| h.drive.find("after.txt").is_some()).await;

    let start = std::time::Instant::now();
    h.stop().await;
    assert!(start.elapsed() < Duration::from_secs(6), "stop took {:?}", start.elapsed());
}

/// Regression: the work queue is in memory, so remote files not yet downloaded when
/// the engine stopped mid-sync were never revisited after a restart.
#[tokio::test(flavor = "multi_thread")]
async fn restart_during_initial_sync_finishes_the_download() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let folder = d.add_folder("Big", ROOT);
    let names: Vec<String> = (0..12).map(|i| format!("Big/f{i:02}.bin")).collect();
    for i in 0..12u8 {
        d.add_file(&format!("f{i:02}.bin"), &folder, &vec![i; 200_000]);
    }
    // Throttled so the engine is reliably stopped part-way through.
    let slow = gdrive_core::config::Config { max_download_mb_per_sec: 0.5, ..h.config() };
    h.start_with(slow);
    h.wait("some files downloaded", T, |h| names.iter().any(|n| h.read(n).is_some())).await;
    h.stop().await;
    let done = names.iter().filter(|n| h.read(n).is_some()).count();
    assert!(done < names.len(), "test needs an interrupted sync, but all {done} files finished");

    h.start();
    h.wait("all files after restart", T, |h| names.iter().all(|n| h.read(n).is_some())).await;
    h.stop().await;
}

/// Files Google flagged as malware/spam are downloaded anyway when the user owns them.
#[tokio::test(flavor = "multi_thread")]
async fn flagged_files_the_user_owns_are_downloaded_anyway() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let id = d.add_file("DeltaPatcherLite.exe", ROOT, b"MZ not really malware");
    d.flag_as_abusive(&id);
    h.start();
    h.wait("flagged file downloaded", T, |h| h.read("DeltaPatcherLite.exe").as_deref() == Some(&b"MZ not really malware"[..])).await;
    assert!(h.status().errors.is_empty(), "{:?}", h.status().errors);
    h.stop().await;
}

/// Regression: a failed download was started again right away instead of after its
/// retry backoff, so a file Drive kept refusing was re-requested in a tight loop.
#[tokio::test(flavor = "multi_thread")]
async fn failed_download_waits_for_its_backoff() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let id = d.add_file("stubborn.bin", ROOT, b"eventually");
    d.break_downloads(&id);
    h.start();
    h.wait("error reported", T, |h| h.status().errors.iter().any(|e| e.path == "stubborn.bin")).await;
    // The first retry is due 10 s after the failure.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let refused = d.counters().refused_downloads;
    assert_eq!(refused, 1, "download retried {refused} times within its backoff");

    // Once Drive serves it again, the scheduled retry downloads it.
    d.fix_downloads(&id);
    h.wait("downloaded on retry", 30, |h| h.read("stubborn.bin").as_deref() == Some(&b"eventually"[..])).await;
    assert!(h.status().errors.is_empty(), "{:?}", h.status().errors);
    h.stop().await;
}

/// Files Drive will never serve to this account (flagged, and owned by someone else)
/// are not retried until they change on Drive, and don't keep the engine from idling.
#[tokio::test(flavor = "multi_thread")]
async fn undownloadable_file_is_not_retried_until_it_changes() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let id = d.add_file("flagged.exe", ROOT, b"v1");
    d.make_undownloadable(&id);
    h.start();
    h.wait("error reported", T, |h| h.status().errors.iter().any(|e| e.path == "flagged.exe")).await;
    h.settle().await;
    let refused = d.counters().refused_downloads;
    h.send(Command::SyncNow);
    h.settle().await;
    assert_eq!(d.counters().refused_downloads, refused, "retried a file Drive will never serve");
    assert_eq!(h.status().errors.len(), 1, "the error stays listed");

    // A change on Drive is worth another try.
    d.state.lock().unwrap().undownloadable.remove(&id);
    d.edit(&id, b"v2");
    h.wait("downloaded after it changed", T, |h| h.read("flagged.exe").as_deref() == Some(&b"v2"[..])).await;
    assert!(h.status().errors.is_empty(), "{:?}", h.status().errors);
    h.stop().await;
}

/// Regression: uploads went through the client's read timeout, which reqwest counts
/// from the start of the request, so any upload longer than it failed and started over.
#[tokio::test(flavor = "multi_thread")]
async fn upload_outlasts_the_read_timeout() {
    support::init_logs();
    let drive = support::FakeDrive::start().await;
    let http = reqwest::Client::builder().read_timeout(Duration::from_millis(500)).build().unwrap();
    let api = drive.client_with(http);
    api.bandwidth().upload.set_rate(200_000);
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("big.bin");
    let content = vec![7u8; 600_000];
    std::fs::write(&local, &content).unwrap();

    let started = std::time::Instant::now();
    let progress = Default::default();
    api.upload_new("big.bin", ROOT, &local, chrono::Utc::now(), &progress).await.unwrap();
    assert!(started.elapsed() > Duration::from_secs(1), "upload too fast to exercise the timeout");
    assert_eq!(drive.content("big.bin").as_deref(), Some(&content[..]));
}

/// A file deleted while its upload waits for a free transfer slot is dropped quietly:
/// no error is listed and it isn't retried.
#[tokio::test(flavor = "multi_thread")]
async fn file_deleted_before_its_upload_starts_is_skipped() {
    support::init_logs();
    let mut h = Harness::new().await;
    let config = gdrive_core::config::Config { max_upload_mb_per_sec: 0.25, max_concurrent_transfers: 1, ..h.config() };
    h.start_with(config);
    h.settle().await;

    // 1.5 MB at 0.25 MB/s holds the only transfer slot for ~6 s.
    h.write("big.bin", &vec![1u8; 1_500_000]);
    h.wait("big upload started", T, |h| h.status().transfers.iter().any(|t| t.path == "big.bin")).await;
    h.write("brief.txt", b"gone soon");
    // Past the debounce, so brief.txt is queued behind big.bin.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(h.status().transfers.iter().any(|t| t.path == "big.bin"), "big.bin finished too early");
    std::fs::remove_file(h.path("brief.txt")).unwrap();

    h.wait("big uploaded", T, |h| h.drive.find("big.bin").is_some()).await;
    h.settle().await;
    assert!(h.status().errors.is_empty(), "{:?}", h.status().errors);
    assert!(h.drive.find("brief.txt").is_none());
    h.stop().await;
}
