//! OAuth 2.0 for installed apps: loopback redirect + PKCE, token persistence and refresh.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::config::{self, Config};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub refresh_token: String,
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

impl Token {
    pub fn load() -> Result<Option<Self>> {
        let path = config::token_file();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(Some(serde_json::from_str(&text).context("corrupt token file")?))
    }

    pub fn save(&self) -> Result<()> {
        let path = config::token_file();
        std::fs::create_dir_all(path.parent().unwrap())?;
        config::write_private(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn delete() -> Result<()> {
        match std::fs::remove_file(config::token_file()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

/// The refresh token was revoked or expired; the user must sign in again.
#[derive(Debug)]
pub struct AuthRevoked(pub String);

impl std::fmt::Display for AuthRevoked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Google sign-in expired or was revoked: {}", self.0)
    }
}

impl std::error::Error for AuthRevoked {}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let resp = http.post(TOKEN_URL).form(form).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if status.is_success() {
        return Ok(serde_json::from_str(&body)?);
    }
    match serde_json::from_str::<TokenError>(&body) {
        Ok(e) if e.error == "invalid_grant" => {
            Err(AuthRevoked(e.error_description.unwrap_or(e.error)).into())
        }
        Ok(e) => bail!("token endpoint: {} {}", e.error, e.error_description.unwrap_or_default()),
        Err(_) => bail!("token endpoint returned {status}: {body}"),
    }
}

/// Hands out valid access tokens, refreshing them when needed.
#[derive(Clone)]
pub struct Authenticator {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    token: Arc<Mutex<Token>>,
}

impl Authenticator {
    pub fn new(http: reqwest::Client, config: &Config, token: Token) -> Self {
        Self {
            http,
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            token: Arc::new(Mutex::new(token)),
        }
    }

    pub async fn access_token(&self) -> Result<String> {
        let mut token = self.token.lock().await;
        if token.expires_at - Duration::seconds(60) <= Utc::now() {
            self.refresh_locked(&mut token).await?;
        }
        Ok(token.access_token.clone())
    }

    /// Forces a refresh, e.g. after the API rejected a token with 401.
    pub async fn invalidate(&self) {
        self.token.lock().await.expires_at = Utc::now();
    }

    async fn refresh_locked(&self, token: &mut Token) -> Result<()> {
        let resp = token_request(
            &self.http,
            &[
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("refresh_token", &token.refresh_token),
                ("grant_type", "refresh_token"),
            ],
        )
        .await?;
        token.access_token = resp.access_token;
        token.expires_at = Utc::now() + Duration::seconds(resp.expires_in);
        if let Some(rt) = resp.refresh_token {
            token.refresh_token = rt;
        }
        token.save()?;
        Ok(())
    }
}

/// An OAuth sign-in waiting for the browser to redirect back.
pub struct PendingLogin {
    pub url: String,
    listener: TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
    client_id: String,
    client_secret: String,
}

fn random_b64(len: usize) -> String {
    let mut buf = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Starts the loopback listener and builds the consent URL.
pub async fn begin_login(config: &Config) -> Result<PendingLogin> {
    if !config.is_complete() {
        bail!("set an OAuth client ID and secret first");
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let verifier = random_b64(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_b64(16);
    let url = url::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", &redirect_uri),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", &state),
        ],
    )?
    .to_string();
    Ok(PendingLogin {
        url,
        listener,
        redirect_uri,
        verifier,
        state,
        client_id: config.client_id.clone(),
        client_secret: config.client_secret.clone(),
    })
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Signed in</title>\
<body style=\"font-family:system-ui;display:grid;place-items:center;height:90vh\">\
<div><h2>You're signed in to gdrive-linux.</h2><p>You can close this tab.</p></div>";

impl PendingLogin {
    /// Waits for the redirect, then exchanges the code for tokens and saves them.
    pub async fn finish(self, http: &reqwest::Client) -> Result<Token> {
        let code = loop {
            let (mut sock, _) = self.listener.accept().await?;
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await?;
            let req = String::from_utf8_lossy(&buf[..n]);
            let Some(target) = req.lines().next().and_then(|l| l.split_whitespace().nth(1)) else {
                continue;
            };
            let url = url::Url::parse(&format!("http://localhost{target}"))?;
            let get = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());
            if get("state").as_deref() != Some(self.state.as_str()) {
                // Browsers also probe /favicon.ico etc.; ignore anything that isn't our redirect.
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                continue;
            }
            if let Some(err) = get("error") {
                let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
                bail!("sign-in was cancelled: {err}");
            }
            let code = get("code").ok_or_else(|| anyhow!("redirect without code"))?;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                DONE_PAGE.len(),
                DONE_PAGE
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            break code;
        };
        let resp = token_request(
            http,
            &[
                ("code", &code),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("redirect_uri", &self.redirect_uri),
                ("grant_type", "authorization_code"),
                ("code_verifier", &self.verifier),
            ],
        )
        .await?;
        let token = Token {
            refresh_token: resp
                .refresh_token
                .ok_or_else(|| anyhow!("Google did not return a refresh token"))?,
            access_token: resp.access_token,
            expires_at: Utc::now() + Duration::seconds(resp.expires_in),
        };
        token.save()?;
        Ok(token)
    }
}
