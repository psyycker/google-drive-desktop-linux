//! Updater against a fake GitHub Releases API.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use gdrive_core::update::{self, appimage_asset_name, Updater, Version};
use serde_json::json;
use sha2::{Digest, Sha256};

/// A stand-in "AppImage": a script answering `--cli --version` like the real one.
fn fake_appimage(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\n[ \"$1 $2\" = \"--cli --version\" ] && echo \"gdrive {version}\"\n").into_bytes()
}

/// Serves a `latest` release whose AppImage is `served` while SHA256SUMS lists `listed`.
async fn fake_github(version: &str, served: Vec<u8>, listed: &[u8]) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let name = appimage_asset_name(&Version::parse(version).unwrap());
    let release = json!({
        "tag_name": format!("v{version}"),
        "html_url": format!("{base}/release"),
        "assets": [
            { "name": name, "browser_download_url": format!("{base}/dl/app"), "size": served.len() },
            { "name": "SHA256SUMS", "browser_download_url": format!("{base}/dl/sums"), "size": 0 },
        ],
    });
    let sums = format!("{}  {name}\nabc  other-file.deb\n", hex::encode(Sha256::digest(listed)));
    let app = Router::new()
        .route(
            &format!("/repos/{}/releases/latest", update::REPO),
            get(move || {
                let r = release.clone();
                async move { axum::Json(r) }
            }),
        )
        .route("/dl/app", get(move || {
            let b = served.clone();
            async move { b }
        }))
        .route("/dl/sums", get(move || {
            let s = sums.clone();
            async move { s }
        }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn downloads_verifies_and_installs_a_new_release() {
    let image = fake_appimage("9.9.9");
    let base = fake_github("9.9.9", image.clone(), &image).await;
    let updater = Updater::with_api_base(reqwest::Client::new(), &base);

    let release = updater.latest().await.unwrap();
    assert_eq!(release.version(), Some(Version(9, 9, 9)));
    assert!(release.version().unwrap() > Version::current());

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("gdrive-linux.AppImage");
    std::fs::write(&target, fake_appimage("0.0.1")).unwrap();
    assert!(update::can_replace(&target));
    let staged = dir.path().join(".gdrive-linux.AppImage.update");
    let progress = Arc::new(AtomicU64::new(0));
    updater.download_appimage(&release, &staged, &progress).await.unwrap();
    assert_eq!(std::fs::read(&staged).unwrap(), image);
    assert_eq!(std::fs::metadata(&staged).unwrap().permissions().mode() & 0o777, 0o755);

    update::sanity_check(&staged, &Version(9, 9, 9)).await.unwrap();
    update::install(&staged, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), image);
    assert!(!staged.exists());
}

#[tokio::test]
async fn rejects_a_download_that_does_not_match_the_checksum() {
    let good = fake_appimage("9.9.9");
    let mut tampered = good.clone();
    tampered.extend_from_slice(b"# injected\n");
    tampered.truncate(good.len()); // same size, different bytes
    tampered[good.len() - 2] = b'X';
    let base = fake_github("9.9.9", tampered, &good).await;
    let updater = Updater::with_api_base(reqwest::Client::new(), &base);
    let release = updater.latest().await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("staged");
    let err = updater.download_appimage(&release, &staged, &Arc::new(AtomicU64::new(0))).await.unwrap_err();
    assert!(format!("{err:#}").contains("checksum mismatch"), "{err:#}");
    assert!(!staged.exists(), "a rejected download must be deleted");
}

#[tokio::test]
async fn self_test_rejects_a_binary_reporting_the_wrong_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app");
    std::fs::write(&path, fake_appimage("1.0.0")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    update::sanity_check(&path, &Version(1, 0, 0)).await.unwrap();
    assert!(update::sanity_check(&path, &Version(2, 0, 0)).await.is_err());

    std::fs::write(&path, b"#!/bin/sh\nexit 3\n").unwrap();
    assert!(update::sanity_check(&path, &Version(1, 0, 0)).await.is_err());
}

#[test]
fn inherited_descriptors_do_not_leak_into_children() {
    use std::os::fd::AsRawFd;
    // An inheritable descriptor, like the one the AppImage runtime leaves us.
    let file = std::fs::File::open("/proc/self/stat").unwrap();
    let fd = file.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
    }
    let visible = |fd: i32| {
        let out = std::process::Command::new("sh").args(["-c", &format!("[ -e /proc/self/fd/{fd} ] && echo yes || echo no")]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    assert_eq!(visible(fd), "yes", "precondition: the descriptor is inheritable");
    update::mark_inherited_fds_cloexec();
    assert_eq!(visible(fd), "no");
}
