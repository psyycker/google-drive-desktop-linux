//! Daemon <-> client protocol: newline-delimited JSON over a Unix socket.
//!
//! Each request is one JSON line and gets exactly one JSON line back. Clients poll
//! `Status` (it is cheap) rather than subscribing to a stream.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::config::{self, Config};
use crate::status::Status;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    Pause,
    Resume,
    /// Poll Drive and process pending local changes immediately.
    SyncNow,
    /// Re-list the whole Drive and rescan the whole sync folder.
    FullResync,
    /// Permanently delete everything in the sync folder, forget the sync state (but
    /// stay signed in) and download the whole Drive again from scratch.
    Redownload,
    /// Begin the OAuth flow. Returns `LoginUrl`; the daemon also opens it in the browser.
    StartLogin,
    /// Forget the account and all sync state. Local files are left in place.
    SignOut,
    /// Check GitHub for a newer version now.
    CheckForUpdates,
    GetConfig,
    SetConfig { config: Config },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Status { status: Box<Status> },
    LoginUrl { url: String },
    Config { config: Config },
    Error { message: String },
}

/// Sends one request to the daemon and waits for the reply.
pub async fn request(req: &Request) -> Result<Response> {
    request_at(&config::socket_path(), req).await
}

pub async fn request_at(socket: &Path, req: &Request) -> Result<Response> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("cannot reach gdrived at {} (is it running?)", socket.display()))?;
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    write.write_all(line.as_bytes()).await?;
    let mut reader = BufReader::new(read);
    let mut reply = String::new();
    if reader.read_line(&mut reply).await? == 0 {
        bail!("gdrived closed the connection");
    }
    Ok(serde_json::from_str(&reply)?)
}

/// Like [`request`], but turns `Response::Error` into an `Err`.
pub async fn call(req: &Request) -> Result<Response> {
    match request(req).await? {
        Response::Error { message } => bail!(message),
        other => Ok(other),
    }
}
