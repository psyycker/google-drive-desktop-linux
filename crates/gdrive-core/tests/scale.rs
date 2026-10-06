//! Restart cost with a large, fully synced tree. Run with:
//! `cargo test --release -p gdrive-core --test scale -- --ignored --nocapture`

mod support;

use std::time::Instant;

use gdrive_core::status::SyncState;
use support::{Harness, ROOT};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn restart_of_large_synced_tree() {
    support::init_logs();
    let mut h = Harness::new().await;
    let d = h.drive.clone();
    let mut n = 0;
    for a in 0..10 {
        let fa = d.add_folder(&format!("A{a}"), ROOT);
        for b in 0..10 {
            let fb = d.add_folder(&format!("B{b}"), &fa);
            for c in 0..5 {
                let fc = d.add_folder(&format!("C{c}"), &fb);
                for f in 0..20 {
                    d.add_file(&format!("f{f}.txt"), &fc, b"x");
                    n += 1;
                }
            }
        }
    }
    let t = Instant::now();
    h.start();
    h.wait("initial sync", 3600, |h| h.status().pending == 0 && h.state() == SyncState::Idle).await;
    println!("initial sync of {n} files: {:?}", t.elapsed());
    h.stop().await;

    h.status.write().unwrap().state = SyncState::Starting;
    let t = Instant::now();
    h.start();
    h.wait("restart idle", 3600, |h| {
        let st = h.status();
        st.state == SyncState::Idle && st.pending == 0
    })
    .await;
    println!("restart to idle: {:?}", t.elapsed());
    h.stop().await;
}
