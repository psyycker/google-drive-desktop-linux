//! Thin Google Drive v3 REST client with retries and backoff.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use futures::{StreamExt, TryStreamExt};
use md5::{Digest, Md5};
use reqwest::{header, Method, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::auth::{AuthRevoked, Authenticator};
use crate::bandwidth::Bandwidth;

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
pub const SHORTCUT_MIME: &str = "application/vnd.google-apps.shortcut";
const FILE_FIELDS: &str = "id,name,mimeType,parents,md5Checksum,size,modifiedTime,trashed,webViewLink";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    #[serde(default)]
    pub parents: Vec<String>,
    pub md5_checksum: Option<String>,
    /// int64 values are serialized as strings by the Drive API.
    pub size: Option<String>,
    pub modified_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub trashed: bool,
    pub web_view_link: Option<String>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.mime_type == FOLDER_MIME
    }

    /// Docs, Sheets, Slides… — files that have no binary content to download.
    pub fn is_google_native(&self) -> bool {
        self.mime_type.starts_with("application/vnd.google-apps.") && !self.is_folder()
    }

    pub fn size(&self) -> u64 {
        self.size.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub file_id: Option<String>,
    #[serde(default)]
    pub removed: bool,
    pub file: Option<DriveFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePage {
    #[serde(default)]
    pub changes: Vec<Change>,
    pub next_page_token: Option<String>,
    pub new_start_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct About {
    pub user: AboutUser,
    pub storage_quota: StorageQuota,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutUser {
    pub display_name: String,
    pub email_address: String,
    pub photo_link: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageQuota {
    pub limit: Option<String>,
    pub usage: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    next_page_token: Option<String>,
}

#[derive(Debug)]
pub enum ApiError {
    /// Could not reach Google at all.
    Offline(String),
    Http { status: StatusCode, reason: String, message: String },
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Offline(m) => write!(f, "network unavailable: {m}"),
            ApiError::Http { status, message, .. } => write!(f, "Drive API error {}: {message}", status.as_u16()),
        }
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    pub fn is_not_found(err: &anyhow::Error) -> bool {
        matches!(err.downcast_ref::<ApiError>(), Some(ApiError::Http { status, .. }) if *status == StatusCode::NOT_FOUND)
    }

    pub fn is_offline(err: &anyhow::Error) -> bool {
        matches!(err.downcast_ref::<ApiError>(), Some(ApiError::Offline(_)))
    }
}

/// Shared byte counter used to report transfer progress.
pub type Progress = Arc<AtomicU64>;

#[derive(Clone)]
pub struct DriveClient {
    http: reqwest::Client,
    auth: Authenticator,
    api_base: String,
    upload_base: String,
    bandwidth: Arc<Bandwidth>,
}

async fn error_from(resp: Response) -> anyhow::Error {
    #[derive(Deserialize)]
    struct Body {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        message: String,
        #[serde(default)]
        errors: Vec<Detail>,
    }
    #[derive(Deserialize)]
    struct Detail {
        reason: String,
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let (reason, message) = match serde_json::from_str::<Body>(&text) {
        Ok(b) => (b.error.errors.into_iter().next().map(|d| d.reason).unwrap_or_default(), b.error.message),
        Err(_) => (String::new(), text),
    };
    ApiError::Http { status, reason, message }.into()
}

fn is_retryable(status: StatusCode, reason: &str) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || (status == StatusCode::FORBIDDEN && (reason == "rateLimitExceeded" || reason == "userRateLimitExceeded"))
}

impl DriveClient {
    pub fn new(http: reqwest::Client, auth: Authenticator) -> Self {
        Self::with_endpoints(http, auth, API, UPLOAD_API)
    }

    /// Like [`DriveClient::new`] but against other base URLs (used by tests with a fake Drive).
    pub fn with_endpoints(http: reqwest::Client, auth: Authenticator, api_base: &str, upload_base: &str) -> Self {
        Self {
            http,
            auth,
            api_base: api_base.trim_end_matches('/').to_owned(),
            upload_base: upload_base.trim_end_matches('/').to_owned(),
            bandwidth: Arc::default(),
        }
    }

    /// Rate limits and byte counters shared by every transfer made through this client.
    pub fn bandwidth(&self) -> &Arc<Bandwidth> {
        &self.bandwidth
    }

    /// Sends a request built by `build`, retrying on rate limits, server errors,
    /// transient network failures, and once on an expired access token.
    async fn send(&self, build: impl Fn(&reqwest::Client) -> Result<RequestBuilder>) -> Result<Response> {
        const MAX_ATTEMPTS: u32 = 6;
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let token = self.auth.access_token().await.map_err(|e| {
                if e.is::<AuthRevoked>() || e.is::<ApiError>() {
                    e
                } else if let Some(re) = e.downcast_ref::<reqwest::Error>() {
                    ApiError::Offline(re.to_string()).into()
                } else {
                    e
                }
            })?;
            let result = build(&self.http)?.bearer_auth(token).send().await;
            let backoff = Duration::from_millis(500 * 2u64.pow(attempt.min(6)) + rand::random::<u64>() % 500);
            match result {
                Ok(resp) if resp.status().is_success() => return Ok(resp),
                Ok(resp) if resp.status() == StatusCode::UNAUTHORIZED && !refreshed => {
                    refreshed = true;
                    self.auth.invalidate().await;
                }
                Ok(resp) => {
                    let err = error_from(resp).await;
                    let retry = match err.downcast_ref::<ApiError>() {
                        Some(ApiError::Http { status, reason, .. }) => is_retryable(*status, reason),
                        _ => false,
                    };
                    if !retry || attempt >= MAX_ATTEMPTS {
                        return Err(err);
                    }
                    tracing::debug!("retrying after {err}");
                    tokio::time::sleep(backoff).await;
                }
                Err(e) if e.is_connect() || e.is_timeout() || e.is_request() => {
                    if attempt >= 3 {
                        return Err(ApiError::Offline(e.to_string()).into());
                    }
                    tokio::time::sleep(backoff).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str, query: &[(&str, &str)]) -> Result<T> {
        let resp = self.send(|c| Ok(c.get(url).query(query))).await?;
        Ok(resp.json().await?)
    }

    pub async fn about(&self) -> Result<About> {
        self.get_json(&format!("{}/about", self.api_base), &[("fields", "user(displayName,emailAddress,photoLink),storageQuota(limit,usage)")])
            .await
    }

    pub async fn root_id(&self) -> Result<String> {
        let f: DriveFile = self.get_json(&format!("{}/files/root", self.api_base), &[("fields", FILE_FIELDS)]).await?;
        Ok(f.id)
    }

    pub async fn get(&self, id: &str) -> Result<DriveFile> {
        self.get_json(&format!("{}/files/{id}", self.api_base), &[("fields", FILE_FIELDS)]).await
    }

    pub async fn start_page_token(&self) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R {
            start_page_token: String,
        }
        let r: R = self.get_json(&format!("{}/changes/startPageToken", self.api_base), &[]).await?;
        Ok(r.start_page_token)
    }

    async fn list(&self, q: &str, mut on_page: impl FnMut(Vec<DriveFile>) -> Result<()>) -> Result<()> {
        let fields = format!("nextPageToken,files({FILE_FIELDS})");
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("q", q), ("fields", fields.as_str()), ("pageSize", "1000"), ("spaces", "drive")];
            if let Some(t) = &page_token {
                query.push(("pageToken", t));
            }
            let page: FileList = self.get_json(&format!("{}/files", self.api_base), &query).await?;
            on_page(page.files)?;
            match page.next_page_token {
                Some(t) => page_token = Some(t),
                None => return Ok(()),
            }
        }
    }

    /// Lists every non-trashed file visible to the user, page by page.
    pub async fn list_all(&self, on_page: impl FnMut(Vec<DriveFile>) -> Result<()>) -> Result<()> {
        self.list("trashed = false", on_page).await
    }

    pub async fn list_children(&self, folder_id: &str) -> Result<Vec<DriveFile>> {
        let mut out = Vec::new();
        self.list(&format!("'{folder_id}' in parents and trashed = false"), |files| {
            out.extend(files);
            Ok(())
        })
        .await?;
        Ok(out)
    }

    pub async fn changes(&self, page_token: &str) -> Result<ChangePage> {
        let fields = format!("nextPageToken,newStartPageToken,changes(fileId,removed,file({FILE_FIELDS}))");
        self.get_json(
            &format!("{}/changes", self.api_base),
            &[
                ("pageToken", page_token),
                ("fields", &fields),
                ("pageSize", "1000"),
                ("includeRemoved", "true"),
                ("spaces", "drive"),
            ],
        )
        .await
    }

    /// Streams a file's content to `dest` and returns the MD5 of what was written.
    pub async fn download(&self, id: &str, dest: &Path, progress: &Progress) -> Result<String> {
        let url = format!("{}/files/{id}", self.api_base);
        let resp = self.send(|c| Ok(c.get(&url).query(&[("alt", "media")]))).await?;
        let mut file = tokio::fs::File::create(dest).await.with_context(|| format!("creating {}", dest.display()))?;
        let mut hasher = Md5::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::Offline(e.to_string()))?;
            self.bandwidth.download.consume(chunk.len()).await;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        file.sync_all().await?;
        Ok(hex::encode(hasher.finalize()))
    }

    pub async fn create_folder(&self, name: &str, parent: &str) -> Result<DriveFile> {
        let url = format!("{}/files", self.api_base);
        let body = json!({ "name": name, "parents": [parent], "mimeType": FOLDER_MIME });
        let resp = self.send(|c| Ok(c.post(&url).query(&[("fields", FILE_FIELDS)]).json(&body))).await?;
        Ok(resp.json().await?)
    }

    /// Renames and/or moves a file. Pass `parents = Some((new, old))` to move.
    pub async fn update_metadata(&self, id: &str, name: Option<&str>, parents: Option<(&str, &str)>) -> Result<DriveFile> {
        let url = format!("{}/files/{id}", self.api_base);
        let body = match name {
            Some(n) => json!({ "name": n }),
            None => json!({}),
        };
        let mut query = vec![("fields", FILE_FIELDS)];
        if let Some((add, remove)) = parents {
            query.push(("addParents", add));
            query.push(("removeParents", remove));
        }
        let resp = self.send(|c| Ok(c.patch(&url).query(&query).json(&body))).await?;
        Ok(resp.json().await?)
    }

    /// Moves a file to the Drive trash (recoverable for 30 days), like the official app.
    pub async fn trash(&self, id: &str) -> Result<()> {
        let url = format!("{}/files/{id}", self.api_base);
        self.send(|c| Ok(c.patch(&url).query(&[("fields", "id")]).json(&json!({ "trashed": true })))).await?;
        Ok(())
    }

    /// Uploads `local` as a new file in `parent`.
    pub async fn upload_new(&self, name: &str, parent: &str, local: &Path, mtime: DateTime<Utc>, progress: &Progress) -> Result<DriveFile> {
        let meta = json!({ "name": name, "parents": [parent], "modifiedTime": mtime.to_rfc3339() });
        self.resumable_upload(Method::POST, format!("{}/files", self.upload_base), meta, local, progress).await
    }

    /// Replaces the content of an existing file (keeps its revision history).
    pub async fn upload_update(&self, id: &str, local: &Path, mtime: DateTime<Utc>, progress: &Progress) -> Result<DriveFile> {
        let meta = json!({ "modifiedTime": mtime.to_rfc3339() });
        self.resumable_upload(Method::PATCH, format!("{}/files/{id}", self.upload_base), meta, local, progress).await
    }

    async fn resumable_upload(
        &self,
        method: Method,
        url: String,
        meta: serde_json::Value,
        local: &Path,
        progress: &Progress,
    ) -> Result<DriveFile> {
        let size = tokio::fs::metadata(local).await?.len();
        let resp = self
            .send(|c| {
                Ok(c.request(method.clone(), &url)
                    .query(&[("uploadType", "resumable"), ("fields", FILE_FIELDS)])
                    .header("X-Upload-Content-Length", size)
                    .json(&meta))
            })
            .await?;
        let session = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| anyhow!("upload session without Location header"))?
            .to_owned();

        let file = tokio::fs::File::open(local).await?;
        let counter = progress.clone();
        let bandwidth = self.bandwidth.clone();
        // Small chunks keep throttled uploads smooth.
        let stream = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024).and_then(move |chunk| {
            let counter = counter.clone();
            let bandwidth = bandwidth.clone();
            async move {
                bandwidth.upload.consume(chunk.len()).await;
                counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                Ok(chunk)
            }
        });
        // The session URL is pre-authorized; a body stream can't be replayed, so failures
        // here bubble up and the engine retries the whole upload later.
        let resp = self
            .http
            .put(&session)
            .header(header::CONTENT_LENGTH, size)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| ApiError::Offline(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(error_from(resp).await);
        }
        Ok(resp.json().await?)
    }
}
