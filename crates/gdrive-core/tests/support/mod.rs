//! In-memory fake of the parts of the Google Drive v3 API the engine uses,
//! plus a harness that runs the real engine against it.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use gdrive_core::api::DriveClient;
use gdrive_core::auth::{Authenticator, Token};
use gdrive_core::config::Config;
use gdrive_core::db::Db;
use gdrive_core::engine::{self, Command, EngineHandle};
use gdrive_core::status::{Status, SyncState};
use md5::{Digest, Md5};
use serde_json::{json, Value};

pub const ROOT: &str = "root-id";
pub const FOLDER: &str = "application/vnd.google-apps.folder";
pub const DOC: &str = "application/vnd.google-apps.document";
/// Small pages exercise pagination of files.list and changes.list.
const PAGE: usize = 3;

#[derive(Clone, Debug)]
pub struct FFile {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub parents: Vec<String>,
    pub content: Vec<u8>,
    pub modified: DateTime<Utc>,
    pub trashed: bool,
}

impl FFile {
    fn is_binary(&self) -> bool {
        !self.mime.starts_with("application/vnd.google-apps.")
    }

    fn json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "name": self.name,
            "mimeType": self.mime,
            "parents": self.parents,
            "modifiedTime": self.modified.to_rfc3339_opts(SecondsFormat::Millis, true),
            "trashed": self.trashed,
            "webViewLink": format!("https://docs.example/{}", self.id),
        });
        if self.is_binary() {
            v["md5Checksum"] = json!(hex::encode(Md5::digest(&self.content)));
            v["size"] = json!(self.content.len().to_string());
        }
        v
    }
}

#[derive(Default, Debug, Clone)]
pub struct Counters {
    pub uploads: usize,
    pub downloads: usize,
    pub creates: usize,
    pub patches: usize,
    pub trashes: usize,
    /// Mutating requests of any kind (uploads, creates, patches).
    pub writes: usize,
}

#[derive(Default)]
pub struct DriveState {
    pub files: BTreeMap<String, FFile>,
    /// (token, file id); a change with token t is visible to page tokens <= t.
    changes: Vec<(u64, String)>,
    next_token: u64,
    next_id: u64,
    sessions: HashMap<String, (Option<String>, Value)>,
    pub counters: Counters,
    pub fail_next: u32,
    /// Files flagged as malware/spam that the user doesn't own: never downloadable.
    pub undownloadable: HashSet<String>,
    /// Files flagged as malware/spam that the user owns: downloadable with acknowledgeAbuse.
    pub flagged: HashSet<String>,
}

impl DriveState {
    fn new_id(&mut self) -> String {
        self.next_id += 1;
        format!("f{:04}", self.next_id)
    }

    fn record(&mut self, id: &str) {
        self.next_token += 1;
        self.changes.push((self.next_token, id.to_owned()));
    }
}

#[derive(Clone)]
pub struct FakeDrive {
    pub state: Arc<Mutex<DriveState>>,
    pub base: String,
}

type St = Arc<Mutex<DriveState>>;

/// What Drive returns for files it flagged as malware or spam.
fn abusive_error() -> Response {
    let msg = "This file has been identified as malware or spam and cannot be downloaded.";
    let body = json!({"error": {"code": 403, "message": msg, "errors": [{"reason": "cannotDownloadAbusiveFile"}]}});
    (StatusCode::FORBIDDEN, Json(body)).into_response()
}

fn api_error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({"error": {"code": status.as_u16(), "message": msg, "errors": [{"reason": "fake"}]}}))).into_response()
}

async fn faults(State(st): State<St>, req: Request, next: Next) -> Response {
    {
        let mut s = st.lock().unwrap();
        if s.fail_next > 0 {
            s.fail_next -= 1;
            return api_error(StatusCode::SERVICE_UNAVAILABLE, "injected failure");
        }
    }
    next.run(req).await
}

async fn about() -> Json<Value> {
    Json(json!({
        "user": {"displayName": "Test User", "emailAddress": "test@example.com"},
        "storageQuota": {"limit": "1000000000", "usage": "1234"}
    }))
}

async fn start_page_token(State(st): State<St>) -> Json<Value> {
    let s = st.lock().unwrap();
    Json(json!({"startPageToken": (s.next_token + 1).to_string()}))
}

async fn changes(State(st): State<St>, Query(q): Query<HashMap<String, String>>) -> Response {
    let s = st.lock().unwrap();
    let from: u64 = q.get("pageToken").and_then(|t| t.parse().ok()).unwrap_or(1);
    let pending: Vec<&(u64, String)> = s.changes.iter().filter(|(t, _)| *t >= from).collect();
    let page: Vec<Value> = pending
        .iter()
        .take(PAGE)
        .map(|(_, id)| match s.files.get(id) {
            Some(f) => json!({"fileId": id, "removed": false, "file": f.json()}),
            None => json!({"fileId": id, "removed": true}),
        })
        .collect();
    let mut out = json!({"changes": page});
    if pending.len() > PAGE {
        out["nextPageToken"] = json!(pending[PAGE].0.to_string());
    } else {
        out["newStartPageToken"] = json!((s.next_token + 1).to_string());
    }
    Json(out).into_response()
}

async fn list_files(State(st): State<St>, Query(q): Query<HashMap<String, String>>) -> Response {
    let s = st.lock().unwrap();
    let query = q.get("q").cloned().unwrap_or_default();
    let parent = query.strip_prefix('\'').and_then(|r| r.split_once('\'')).map(|(p, _)| p.to_owned());
    let all: Vec<&FFile> = s
        .files
        .values()
        .filter(|f| !f.trashed)
        .filter(|f| parent.as_ref().is_none_or(|p| f.parents.contains(p)))
        .collect();
    let offset: usize = q.get("pageToken").and_then(|t| t.parse().ok()).unwrap_or(0);
    let page: Vec<Value> = all.iter().skip(offset).take(PAGE).map(|f| f.json()).collect();
    let mut out = json!({"files": page});
    if offset + PAGE < all.len() {
        out["nextPageToken"] = json!((offset + PAGE).to_string());
    }
    Json(out).into_response()
}

async fn get_file(State(st): State<St>, UrlPath(id): UrlPath<String>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut s = st.lock().unwrap();
    if id == "root" {
        return Json(json!({"id": ROOT, "name": "My Drive", "mimeType": FOLDER})).into_response();
    }
    let Some(f) = s.files.get(&id).cloned() else { return api_error(StatusCode::NOT_FOUND, "File not found") };
    if q.get("alt").map(String::as_str) == Some("media") {
        if !f.is_binary() {
            return api_error(StatusCode::FORBIDDEN, "native file");
        }
        let acknowledged = q.get("acknowledgeAbuse").map(String::as_str) == Some("true");
        if s.undownloadable.contains(&id) || (s.flagged.contains(&id) && !acknowledged) {
            return abusive_error();
        }
        s.counters.downloads += 1;
        return f.content.into_response();
    }
    Json(f.json()).into_response()
}

fn parse_body(body: &Bytes) -> Value {
    if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(body).unwrap_or(json!({}))
    }
}

fn parse_time(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(Into::into)
}

async fn create_file(State(st): State<St>, body: Bytes) -> Response {
    let meta = parse_body(&body);
    let mut s = st.lock().unwrap();
    let id = s.new_id();
    let f = FFile {
        id: id.clone(),
        name: meta["name"].as_str().unwrap_or("Untitled").to_owned(),
        mime: meta["mimeType"].as_str().unwrap_or("application/octet-stream").to_owned(),
        parents: meta["parents"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect()).unwrap_or_else(|| vec![ROOT.into()]),
        content: Vec::new(),
        modified: Utc::now(),
        trashed: false,
    };
    let out = f.json();
    s.files.insert(id.clone(), f);
    s.record(&id);
    s.counters.creates += 1;
    s.counters.writes += 1;
    Json(out).into_response()
}

async fn patch_file(
    State(st): State<St>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let meta = parse_body(&body);
    let mut s = st.lock().unwrap();
    let Some(f) = s.files.get_mut(&id) else { return api_error(StatusCode::NOT_FOUND, "File not found") };
    if let Some(name) = meta["name"].as_str() {
        f.name = name.to_owned();
    }
    let mut trashed = false;
    if let Some(t) = meta["trashed"].as_bool() {
        f.trashed = t;
        trashed = t;
    }
    if let Some(remove) = q.get("removeParents") {
        f.parents.retain(|p| p != remove);
    }
    if let Some(add) = q.get("addParents") {
        f.parents.push(add.clone());
    }
    let out = f.json();
    s.record(&id);
    s.counters.patches += 1;
    s.counters.writes += 1;
    if trashed {
        s.counters.trashes += 1;
    }
    Json(out).into_response()
}

fn start_session(st: &St, base: &str, id: Option<String>, body: &Bytes) -> Response {
    let mut s = st.lock().unwrap();
    if let Some(id) = &id {
        if !s.files.contains_key(id) {
            return api_error(StatusCode::NOT_FOUND, "File not found");
        }
    }
    s.next_id += 1;
    let sid = format!("s{}", s.next_id);
    s.sessions.insert(sid.clone(), (id, parse_body(body)));
    let mut headers = HeaderMap::new();
    headers.insert(header::LOCATION, format!("{base}/session/{sid}").parse().unwrap());
    (StatusCode::OK, headers).into_response()
}

async fn finish_session(State((st, _)): State<(St, String)>, UrlPath(sid): UrlPath<String>, body: Bytes) -> Response {
    let mut s = st.lock().unwrap();
    let Some((existing, meta)) = s.sessions.remove(&sid) else { return api_error(StatusCode::NOT_FOUND, "no session") };
    let modified = parse_time(&meta["modifiedTime"]).unwrap_or_else(Utc::now);
    let id = match existing {
        Some(id) => {
            let Some(f) = s.files.get_mut(&id) else { return api_error(StatusCode::NOT_FOUND, "File not found") };
            f.content = body.to_vec();
            f.modified = modified;
            id
        }
        None => {
            let id = s.new_id();
            let f = FFile {
                id: id.clone(),
                name: meta["name"].as_str().unwrap_or("Untitled").to_owned(),
                mime: "application/octet-stream".into(),
                parents: meta["parents"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect())
                    .unwrap_or_else(|| vec![ROOT.into()]),
                content: body.to_vec(),
                modified,
                trashed: false,
            };
            s.files.insert(id.clone(), f);
            id
        }
    };
    s.record(&id);
    s.counters.uploads += 1;
    s.counters.writes += 1;
    Json(s.files[&id].json()).into_response()
}

impl FakeDrive {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state: St = Arc::new(Mutex::new(DriveState::default()));
        let up_base = format!("{base}/upload/drive/v3");

        let api = Router::new()
            .route("/about", get(about))
            .route("/changes/startPageToken", get(start_page_token))
            .route("/changes", get(changes))
            .route("/files", get(list_files).post(create_file))
            .route("/files/{id}", get(get_file).patch(patch_file))
            .with_state(state.clone());
        let ub = up_base.clone();
        let st2 = state.clone();
        let ub2 = up_base.clone();
        let st3 = state.clone();
        let upload = Router::new()
            .route("/files", post(move |body: Bytes| async move { start_session(&st2, &ub, None, &body) }))
            .route(
                "/files/{id}",
                axum::routing::patch(move |UrlPath(id): UrlPath<String>, body: Bytes| async move {
                    start_session(&st3, &ub2, Some(id), &body)
                }),
            )
            .route("/session/{sid}", put(finish_session))
            .with_state((state.clone(), up_base.clone()));
        let app = Router::new()
            .nest("/drive/v3", api)
            .nest("/upload/drive/v3", upload)
            .layer(axum::middleware::from_fn_with_state(state.clone(), faults));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { state, base }
    }

    pub fn client(&self) -> DriveClient {
        let http = reqwest::Client::new();
        let token = Token {
            refresh_token: "r".into(),
            access_token: "a".into(),
            expires_at: Utc::now() + chrono::Duration::days(365),
        };
        let auth = Authenticator::new(http.clone(), &Config::default(), token);
        DriveClient::with_endpoints(http, auth, &format!("{}/drive/v3", self.base), &format!("{}/upload/drive/v3", self.base))
    }

    pub fn counters(&self) -> Counters {
        self.state.lock().unwrap().counters.clone()
    }

    pub fn fail_next(&self, n: u32) {
        self.state.lock().unwrap().fail_next = n;
    }

    pub fn make_undownloadable(&self, id: &str) {
        self.state.lock().unwrap().undownloadable.insert(id.to_owned());
    }

    pub fn flag_as_abusive(&self, id: &str) {
        self.state.lock().unwrap().flagged.insert(id.to_owned());
    }

    // ----- "another device" mutations -----

    fn insert(&self, name: &str, parent: &str, mime: &str, content: &[u8]) -> String {
        let mut s = self.state.lock().unwrap();
        let id = s.new_id();
        s.files.insert(
            id.clone(),
            FFile {
                id: id.clone(),
                name: name.into(),
                mime: mime.into(),
                parents: vec![parent.into()],
                content: content.to_vec(),
                modified: DateTime::parse_from_rfc3339("2024-03-01T12:34:56.789Z").unwrap().into(),
                trashed: false,
            },
        );
        s.record(&id);
        id
    }

    pub fn add_folder(&self, name: &str, parent: &str) -> String {
        self.insert(name, parent, FOLDER, b"")
    }

    pub fn add_file(&self, name: &str, parent: &str, content: &[u8]) -> String {
        self.insert(name, parent, "application/octet-stream", content)
    }

    pub fn add_doc(&self, name: &str, parent: &str) -> String {
        self.insert(name, parent, DOC, b"")
    }

    fn mutate(&self, id: &str, f: impl FnOnce(&mut FFile)) {
        let mut s = self.state.lock().unwrap();
        f(s.files.get_mut(id).expect("file exists"));
        s.record(id);
    }

    pub fn edit(&self, id: &str, content: &[u8]) {
        self.mutate(id, |f| {
            f.content = content.to_vec();
            f.modified = Utc::now();
        });
    }

    pub fn rename(&self, id: &str, name: &str) {
        self.mutate(id, |f| f.name = name.into());
    }

    pub fn move_to(&self, id: &str, parent: &str) {
        self.mutate(id, |f| f.parents = vec![parent.into()]);
    }

    pub fn trash(&self, id: &str) {
        self.mutate(id, |f| f.trashed = true);
    }

    pub fn untrash(&self, id: &str) {
        self.mutate(id, |f| f.trashed = false);
    }

    // ----- inspection -----

    pub fn get(&self, id: &str) -> Option<FFile> {
        self.state.lock().unwrap().files.get(id).cloned()
    }

    /// Finds a non-trashed item by its path of names from the root.
    pub fn find(&self, path: &str) -> Option<FFile> {
        let s = self.state.lock().unwrap();
        let mut parent = ROOT.to_owned();
        let mut found = None;
        for name in path.split('/') {
            let f = s.files.values().find(|f| !f.trashed && f.name == name && f.parents.contains(&parent))?;
            parent = f.id.clone();
            found = Some(f.clone());
        }
        found
    }

    pub fn content(&self, path: &str) -> Option<Vec<u8>> {
        self.find(path).map(|f| f.content)
    }

    /// Names of non-trashed children of a folder, sorted.
    pub fn children(&self, parent: &str) -> Vec<String> {
        let s = self.state.lock().unwrap();
        let mut v: Vec<String> =
            s.files.values().filter(|f| !f.trashed && f.parents.iter().any(|p| p == parent)).map(|f| f.name.clone()).collect();
        v.sort();
        v
    }
}

/// Runs the real engine against a [`FakeDrive`] in temporary directories.
pub struct Harness {
    pub drive: FakeDrive,
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub status: Arc<RwLock<Status>>,
    engine: Option<EngineHandle>,
}

impl Harness {
    pub async fn new() -> Self {
        let drive = FakeDrive::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Drive");
        let db_path = tmp.path().join("state.db");
        let status = Arc::new(RwLock::new(Status::new(SyncState::Starting, root.display().to_string())));
        Self { drive, _tmp: tmp, root, db_path, status, engine: None }
    }

    pub fn config(&self) -> Config {
        Config {
            sync_root: self.root.clone(),
            poll_interval_secs: 1,
            use_local_trash: false,
            max_concurrent_transfers: 3,
            ..Config::default()
        }
    }

    pub fn start(&mut self) {
        self.start_with(self.config());
    }

    pub fn start_with(&mut self, config: Config) {
        assert!(self.engine.is_none());
        let db = Db::open(&self.db_path).unwrap();
        self.engine = Some(engine::start(&config, self.drive.client(), db, self.status.clone()));
    }

    pub async fn stop(&mut self) {
        if let Some(e) = self.engine.take() {
            e.stop().await;
        }
    }

    pub fn send(&self, cmd: Command) {
        self.engine.as_ref().unwrap().send(cmd);
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(rel)).ok()
    }

    pub fn write(&self, rel: &str, content: &[u8]) {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    pub fn state(&self) -> SyncState {
        self.status.read().unwrap().state
    }

    pub fn status(&self) -> Status {
        self.status.read().unwrap().clone()
    }

    /// Local entries (files and dirs) relative to the root, excluding the meta dir, sorted.
    pub fn local_tree(&self) -> Vec<String> {
        let mut out: Vec<String> = walkdir::WalkDir::new(&self.root)
            .min_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .map(|e| e.path().strip_prefix(&self.root).unwrap().to_string_lossy().into_owned())
            .filter(|p| !p.starts_with(".gdrive-linux"))
            .collect();
        out.sort();
        out
    }

    /// Waits until `cond` holds, panicking with `what` after `secs` seconds.
    pub async fn wait(&self, what: &str, secs: u64, mut cond: impl FnMut(&Self) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            if cond(self) {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                let st = self.status();
                panic!(
                    "timed out waiting for: {what}\nstate={:?} msg={:?} pending={}\nerrors={:#?}\nlocal={:#?}\nremote={:#?}",
                    st.state,
                    st.message,
                    st.pending,
                    st.errors,
                    self.local_tree(),
                    self.drive.state.lock().unwrap().files.values().map(|f| (&f.id, &f.name, &f.parents, f.trashed)).collect::<Vec<_>>()
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Waits until the engine is idle with nothing pending, stably for a moment.
    pub async fn settle(&self) {
        let mut stable = 0;
        self.wait("engine idle", 30, |h| {
            let st = h.status();
            if st.state == SyncState::Idle && st.pending == 0 {
                stable += 1;
            } else {
                stable = 0;
            }
            // ~2.5s of continuous idleness spans debounce + a couple of polls.
            stable >= 25
        })
        .await;
    }
}

pub fn mtime(path: &Path) -> DateTime<Utc> {
    let m = std::fs::metadata(path).unwrap().modified().unwrap();
    m.into()
}

pub fn init_logs() {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_test_writer().try_init();
}
