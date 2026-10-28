//! HTTP client for the Arbos Session Protocol (bearer key). The daemon is the only holder of a
//! `Relay`; `mcp` and the CLI go through the control socket.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Relay {
    base: String,
    key: String,
    http: reqwest::Client,
}

/// One page of the inbox.
#[derive(Debug, Clone, Default)]
pub struct InboxPage {
    pub events: Vec<Value>,
    pub next: i64,
}

/// What the relay said about the key.
#[derive(Debug)]
pub enum RelayError {
    /// 401/403 on the key itself: rotated or revoked. Exit, do not retry.
    KeyRejected,
    /// A scope switch is off; the message is the relay's sentence.
    Scope(String),
    Other(anyhow::Error),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyRejected => write!(f, "the relay rejected the agent key"),
            Self::Scope(s) => write!(f, "{s}"),
            Self::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for RelayError {}

impl From<anyhow::Error> for RelayError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}

impl From<reqwest::Error> for RelayError {
    fn from(e: reqwest::Error) -> Self {
        Self::Other(anyhow!("{e}"))
    }
}

pub type RelayResult<T> = std::result::Result<T, RelayError>;

impl Relay {
    pub fn new(base: &str, key: String) -> Result<Self> {
        let base = base.trim_end_matches('/').to_owned();
        if !(base.starts_with("https://")
            || base.starts_with("http://127.0.0.1")
            || base.starts_with("http://localhost"))
        {
            bail!("relay must be https:// (http is allowed for localhost only)");
        }
        let http = reqwest::Client::builder()
            .user_agent(format!("arbos-sidecar/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { base, key, http })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("Authorization", format!("Bearer {}", self.key))
            .header("Arbos-Protocol", "asp/1")
    }

    async fn check(resp: reqwest::Response) -> RelayResult<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let code = v["error"].as_str().unwrap_or("");
        let msg = v["message"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| body.chars().take(200).collect());
        if status.as_u16() == 401 || (status.as_u16() == 403 && code != "scope") {
            // 403 with a non-scope code is the relay refusing the key or the chat; treat an
            // explicit auth failure as rejected, a chat refusal as an ordinary error.
            if status.as_u16() == 401 || code == "unauthorized" || code == "revoked" {
                return Err(RelayError::KeyRejected);
            }
            return Err(RelayError::Other(anyhow!("{status}: {msg}")));
        }
        if code == "scope" {
            return Err(RelayError::Scope(msg));
        }
        Err(RelayError::Other(anyhow!("{status}: {msg}")))
    }

    pub async fn me(&self) -> RelayResult<Value> {
        let r = self
            .auth(self.http.get(format!("{}/asp/me", self.base)))
            .send()
            .await?;
        Ok(Self::check(r).await?.json().await?)
    }

    /// Long-poll: parks up to `wait` seconds (≤ 25) when nothing is queued.
    pub async fn inbox(
        &self,
        since: i64,
        wait: u64,
        limit: usize,
        context: usize,
    ) -> RelayResult<InboxPage> {
        let r = self
            .auth(self.http.get(format!("{}/asp/inbox", self.base)).query(&[
                ("since", since.to_string()),
                ("wait", wait.min(25).to_string()),
                ("limit", limit.to_string()),
                ("context", context.to_string()),
            ]))
            .send()
            .await?;
        let v: Value = Self::check(r).await?.json().await?;
        let events = v["events"].as_array().cloned().unwrap_or_default();
        let next = v["next"].as_i64().unwrap_or(since);
        Ok(InboxPage { events, next })
    }

    /// `POST /asp/events` with one event; answers the first result.
    pub async fn event(&self, event: Value) -> RelayResult<Value> {
        let r = self
            .auth(
                self.http
                    .post(format!("{}/asp/events", self.base))
                    .json(&event),
            )
            .send()
            .await?;
        let v: Value = Self::check(r).await?.json().await?;
        let first = v["results"]
            .as_array()
            .and_then(|a| a.first().cloned())
            .unwrap_or(v);
        if first["ok"] == false {
            let code = first["error"].as_str().unwrap_or("");
            let msg = first["message"]
                .as_str()
                .unwrap_or("the relay refused the event");
            if code == "scope" {
                return Err(RelayError::Scope(msg.to_owned()));
            }
            return Err(RelayError::Other(anyhow!("{msg}")));
        }
        Ok(first)
    }

    pub async fn heartbeat(&self) -> RelayResult<()> {
        self.event(json!({ "type": "presence.heartbeat" })).await?;
        Ok(())
    }

    /// History (#2182), oldest first; `before` pages backwards.
    pub async fn history(
        &self,
        chat: &str,
        before: Option<&str>,
        limit: usize,
    ) -> RelayResult<Value> {
        let mut q = vec![("limit", limit.min(100).to_string())];
        if let Some(b) = before {
            q.push(("before", b.to_owned()));
        }
        let r = self
            .auth(
                self.http
                    .get(format!("{}/asp/chats/{chat}/messages", self.base))
                    .query(&q),
            )
            .send()
            .await?;
        Ok(Self::check(r).await?.json().await?)
    }

    /// Members (#2189).
    pub async fn members(&self, chat: &str) -> RelayResult<Value> {
        let r = self
            .auth(
                self.http
                    .get(format!("{}/asp/chats/{chat}/members", self.base)),
            )
            .send()
            .await?;
        Ok(Self::check(r).await?.json().await?)
    }

    /// A blob named by `attachments[].blob` (#2182, `files.read`).
    pub async fn file(&self, blob: &str) -> RelayResult<(Vec<u8>, String)> {
        let r = self
            .auth(self.http.get(format!("{}/asp/files/{blob}", self.base)))
            .send()
            .await?;
        let r = Self::check(r).await?;
        let mime = r
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = r.bytes().await.context("reading the file")?;
        Ok((bytes.to_vec(), mime))
    }

    /// `POST /asp/log` (#2224): the text-free session log, ≤ 100 entries.
    pub async fn upload_log(&self, entries: &[Value]) -> RelayResult<Value> {
        let r = self
            .auth(
                self.http
                    .post(format!("{}/asp/log", self.base))
                    .json(&json!({ "entries": entries })),
            )
            .send()
            .await?;
        Ok(Self::check(r).await?.json().await?)
    }

    /// `POST /asp/wake/test` (#2185) — only meaningful when the owner set a Wake URL.
    pub async fn wake_test(&self) -> RelayResult<Value> {
        let r = self
            .auth(self.http.post(format!("{}/asp/wake/test", self.base)))
            .send()
            .await?;
        Ok(Self::check(r).await?.json().await?)
    }
}
