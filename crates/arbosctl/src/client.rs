//! Local config (device token, relay URL) and the REST client.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(default = "default_relay")]
    pub relay_url: String,
    #[serde(default)]
    pub device_token: String,
    #[serde(default)]
    pub device_id: String,
}

fn default_relay() -> String {
    "http://127.0.0.1:7700".to_owned()
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = path.unwrap_or_else(|| {
            let base = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                        .join(".config")
                });
            base.join("arbos").join("arbosctl.json")
        });
        let mut cfg: Config = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s)?,
            Err(_) => Config {
                path: path.clone(),
                relay_url: default_relay(),
                device_token: String::new(),
                device_id: String::new(),
            },
        };
        cfg.path = path;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Where local room replicas live.
    pub fn rooms_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(|p| p.join("rooms"))
            .unwrap_or_else(|| PathBuf::from("rooms"))
    }

    pub fn device_name(&self) -> String {
        format!(
            "dev:{}",
            if self.device_id.is_empty() {
                "anon"
            } else {
                &self.device_id
            }
        )
    }
}

pub struct Relay {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Relay {
    pub fn new(cfg: &Config) -> Self {
        Self::with_token(cfg, &cfg.device_token)
    }

    /// Same relay, another bearer (the admin token for `arbosctl admin …`).
    pub fn with_token(cfg: &Config, token: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: cfg.relay_url.clone(),
            token: token.to_owned(),
        }
    }

    fn need_token(&self) -> Result<()> {
        if self.token.is_empty() {
            return Err(anyhow!("not paired: run `arbosctl pair <code>` first"));
        }
        Ok(())
    }

    async fn finish(resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("relay HTTP {status}: {text}"));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.need_token()?;
        Self::finish(
            self.http
                .get(format!("{}{}", self.base, path))
                .bearer_auth(&self.token)
                .send()
                .await?,
        )
        .await
    }

    /// Raw body with its content type (blobs).
    pub async fn get_bytes(&self, path: &str) -> Result<(String, Vec<u8>)> {
        self.need_token()?;
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("relay HTTP {status}: {text}"));
        }
        let mime = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        Ok((mime, resp.bytes().await?.to_vec()))
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.need_token()?;
        Self::finish(
            self.http
                .post(format!("{}{}", self.base, path))
                .bearer_auth(&self.token)
                .json(body)
                .send()
                .await?,
        )
        .await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.need_token()?;
        Self::finish(
            self.http
                .put(format!("{}{}", self.base, path))
                .bearer_auth(&self.token)
                .json(body)
                .send()
                .await?,
        )
        .await
    }

    /// `PUT` raw bytes (a blob upload: `/blob/{slot}/{key}`).
    pub async fn put_bytes(&self, path: &str, mime: &str, bytes: Vec<u8>) -> Result<Value> {
        self.need_token()?;
        Self::finish(
            self.http
                .put(format!("{}{}", self.base, path))
                .bearer_auth(&self.token)
                .header("content-type", mime)
                .body(bytes)
                .send()
                .await?,
        )
        .await
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value> {
        self.need_token()?;
        Self::finish(
            self.http
                .patch(format!("{}{}", self.base, path))
                .bearer_auth(&self.token)
                .json(body)
                .send()
                .await?,
        )
        .await
    }

    pub async fn post_unauth(&self, path: &str, body: &Value) -> Result<Value> {
        Self::finish(
            self.http
                .post(format!("{}{}", self.base, path))
                .json(body)
                .send()
                .await?,
        )
        .await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.need_token()?;
        Self::finish(
            self.http
                .delete(format!("{}{}", self.base, path))
                .bearer_auth(&self.token)
                .send()
                .await?,
        )
        .await
    }
}
