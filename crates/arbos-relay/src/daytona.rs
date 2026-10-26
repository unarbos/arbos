//! Daytona REST client (D25, D59): the calls the relay uses, typed from the live
//! OpenAPI document (`https://www.daytona.io/docs/openapi.json`, fetched
//! 2026-10-14; operationIds noted on each method). Base URL `https://app.daytona.io/api`.
//! The API key lives only here (D2, D28). The full call set is kept even where M0 does not use it yet.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Daytona {
    http: reqwest::Client,
    base: String,
    key: String,
    org: Option<String>,
}

/// `components.schemas.SandboxState` (live spec enum, 22 values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxState {
    Creating,
    Restoring,
    Destroyed,
    Destroying,
    Started,
    Stopped,
    Starting,
    Stopping,
    Error,
    BuildFailed,
    PendingBuild,
    BuildingSnapshot,
    Unknown,
    PullingSnapshot,
    Archived,
    Archiving,
    Resizing,
    Snapshotting,
    Forking,
    Pausing,
    Paused,
    Resuming,
}

impl SandboxState {
    /// Arbos's hibernate mapping (design §4.6).
    pub fn arbos_state(self) -> &'static str {
        match self {
            SandboxState::Started => "awake",
            SandboxState::Creating
            | SandboxState::Restoring
            | SandboxState::Starting
            | SandboxState::PullingSnapshot
            | SandboxState::PendingBuild
            | SandboxState::BuildingSnapshot
            | SandboxState::Resuming
            | SandboxState::Forking
            | SandboxState::Snapshotting
            | SandboxState::Resizing => "waking",
            SandboxState::Stopped
            | SandboxState::Stopping
            | SandboxState::Paused
            | SandboxState::Pausing => "hibernated_light",
            SandboxState::Archived | SandboxState::Archiving => "hibernated_deep",
            SandboxState::Destroyed | SandboxState::Destroying => "deleted",
            SandboxState::Error | SandboxState::BuildFailed | SandboxState::Unknown => "error",
        }
    }
}

/// Subset of `components.schemas.Sandbox`.
#[derive(Debug, Clone, Deserialize)]
pub struct Sandbox {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub state: SandboxState,
    #[serde(rename = "desiredState", default)]
    pub desired_state: Option<SandboxState>,
    #[serde(rename = "errorReason", default)]
    pub error_reason: Option<String>,
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(rename = "autoStopInterval", default)]
    pub auto_stop_interval: Option<f64>,
    #[serde(rename = "autoArchiveInterval", default)]
    pub auto_archive_interval: Option<f64>,
    #[serde(rename = "lastActivityAt", default)]
    pub last_activity_at: Option<String>,
    #[serde(rename = "sandboxClass", default)]
    pub sandbox_class: Option<String>,
    #[serde(rename = "runnerId", default)]
    pub runner_id: Option<String>,
}

/// `components.schemas.CreateSandbox` (the fields we set).
#[derive(Debug, Clone, Serialize, Default)]
pub struct CreateSandbox {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    pub env: serde_json::Map<String, Value>,
    pub labels: serde_json::Map<String, Value>,
    /// Resources are only sent for inline builds; a snapshot carries its own (Daytona rejects both).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<u32>,
    #[serde(rename = "autoStopInterval", skip_serializing_if = "Option::is_none")]
    pub auto_stop_interval: Option<i64>,
    #[serde(
        rename = "autoArchiveInterval",
        skip_serializing_if = "Option::is_none"
    )]
    pub auto_archive_interval: Option<i64>,
    #[serde(rename = "autoDeleteInterval", skip_serializing_if = "Option::is_none")]
    pub auto_delete_interval: Option<i64>,
    #[serde(rename = "ttlMinutes", skip_serializing_if = "Option::is_none")]
    pub ttl_minutes: Option<i64>,
    /// Each entry maps ENV_VAR → vault secret name (placeholder secrets, D28).
    pub secrets: Vec<serde_json::Map<String, Value>>,
    /// `buildInfo.dockerfileContent`: inline Dockerfile for the declarative builder.
    #[serde(rename = "buildInfo", skip_serializing_if = "Option::is_none")]
    pub build_info: Option<Value>,
}

/// `components.schemas.PortPreviewUrl`.
#[derive(Debug, Clone, Deserialize)]
pub struct PortPreviewUrl {
    pub url: String,
    pub token: String,
}

/// `components.schemas.SignedPortPreviewUrl`.
#[derive(Debug, Clone, Deserialize)]
pub struct SignedPortPreviewUrl {
    pub url: String,
    pub token: String,
    pub port: i64,
}

/// `components.schemas.SnapshotDto` (subset).
#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub cpu: Option<i64>,
    #[serde(default)]
    pub mem: Option<i64>,
    #[serde(default)]
    pub disk: Option<i64>,
    #[serde(rename = "errorReason", default)]
    pub error_reason: Option<String>,
}

impl Daytona {
    pub fn new(key: String, base: Option<String>, org: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("client");
        Self {
            http,
            base: base.unwrap_or_else(|| "https://app.daytona.io/api".to_owned()),
            key,
            org,
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(&self.key);
        if let Some(org) = &self.org {
            r = r.header("X-Daytona-Organization-ID", org);
        }
        r
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        r: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<T> {
        let resp = r
            .send()
            .await
            .with_context(|| format!("daytona {what}: request"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "daytona {what}: HTTP {status}: {}",
                text.chars().take(400).collect::<String>()
            ));
        }
        serde_json::from_str(&text).with_context(|| {
            format!(
                "daytona {what}: decode {}",
                text.chars().take(200).collect::<String>()
            )
        })
    }

    /// `createSandbox` — `POST /sandbox`.
    pub async fn create(&self, body: &CreateSandbox) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::POST, "/sandbox").json(body),
            "create",
        )
        .await
    }

    /// `getSandbox` — `GET /sandbox/{sandboxIdOrName}`.
    pub async fn get(&self, id: &str) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::GET, &format!("/sandbox/{id}")),
            "get",
        )
        .await
    }

    /// `startSandbox` — `POST /sandbox/{sandboxIdOrName}/start`.
    pub async fn start(&self, id: &str) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::POST, &format!("/sandbox/{id}/start")),
            "start",
        )
        .await
    }

    /// `stopSandbox` — `POST /sandbox/{sandboxIdOrName}/stop?force=`.
    pub async fn stop(&self, id: &str, force: bool) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::POST, &format!("/sandbox/{id}/stop"))
                .query(&[("force", force)]),
            "stop",
        )
        .await
    }

    /// `archiveSandbox` — `POST /sandbox/{sandboxIdOrName}/archive`.
    pub async fn archive(&self, id: &str) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::POST, &format!("/sandbox/{id}/archive")),
            "archive",
        )
        .await
    }

    /// `deleteSandbox` — `DELETE /sandbox/{sandboxIdOrName}`.
    pub async fn delete(&self, id: &str) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::DELETE, &format!("/sandbox/{id}")),
            "delete",
        )
        .await
    }

    /// `setAutostopInterval` — `POST /sandbox/{sandboxIdOrName}/autostop/{interval}` (minutes, 0 = off).
    pub async fn set_autostop(&self, id: &str, minutes: i64) -> Result<Sandbox> {
        self.send(
            self.req(
                reqwest::Method::POST,
                &format!("/sandbox/{id}/autostop/{minutes}"),
            ),
            "autostop",
        )
        .await
    }

    /// `updateLastActivity` — `POST /sandbox/{sandboxId}/last-activity` (the keep-alive, design §4.6).
    pub async fn refresh_activity(&self, id: &str) -> Result<()> {
        let r = self
            .req(
                reqwest::Method::POST,
                &format!("/sandbox/{id}/last-activity"),
            )
            .json(&json!({ "activityType": "arbos-keepalive" }));
        let resp = r.send().await.context("daytona last-activity")?;
        if !resp.status().is_success() {
            return Err(anyhow!("daytona last-activity: HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// `updateSandboxSecrets` — `PUT /sandbox/{sandboxIdOrName}/secrets` (`{}` on Stop project, D28).
    pub async fn update_secrets(
        &self,
        id: &str,
        secrets: Vec<serde_json::Map<String, Value>>,
    ) -> Result<Sandbox> {
        self.send(
            self.req(reqwest::Method::PUT, &format!("/sandbox/{id}/secrets"))
                .json(&json!({ "secrets": secrets })),
            "secrets",
        )
        .await
    }

    /// `secret.create` — `POST /secret { name, value, hosts }` (D28). Returns the secret id.
    pub async fn create_secret(&self, name: &str, value: &str, hosts: &[String]) -> Result<String> {
        let v: Value = self
            .send(
                self.req(reqwest::Method::POST, "/secret").json(&json!({
                    "name": name, "value": value, "hosts": hosts,
                    "description": "Arbos vault item (placeholder-protected)",
                })),
                "create secret",
            )
            .await?;
        v.get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("daytona create secret: reply has no id"))
    }

    /// `secret.update` — `PATCH /secret/{id} { value, hosts }`; the placeholder stays, sandboxes see the new value in ≤ 15 s.
    pub async fn update_secret(&self, id: &str, value: &str, hosts: &[String]) -> Result<()> {
        let _: Value = self
            .send(
                self.req(reqwest::Method::PATCH, &format!("/secret/{id}"))
                    .json(&json!({ "value": value, "hosts": hosts })),
                "update secret",
            )
            .await?;
        Ok(())
    }

    /// `getPortPreviewUrl` — `GET /sandbox/{sandboxIdOrName}/ports/{port}/preview-url` (sandbox-wide token; relay only).
    pub async fn preview_url(&self, id: &str, port: u16) -> Result<PortPreviewUrl> {
        self.send(
            self.req(
                reqwest::Method::GET,
                &format!("/sandbox/{id}/ports/{port}/preview-url"),
            ),
            "preview-url",
        )
        .await
    }

    /// `getSignedPortPreviewUrl` — per-port, expiring (v2 direct path, D24).
    pub async fn signed_preview_url(
        &self,
        id: &str,
        port: u16,
        expires_in_seconds: u32,
    ) -> Result<SignedPortPreviewUrl> {
        self.send(
            self.req(
                reqwest::Method::GET,
                &format!("/sandbox/{id}/ports/{port}/signed-preview-url"),
            )
            .query(&[("expiresInSeconds", expires_in_seconds)]),
            "signed-preview-url",
        )
        .await
    }

    /// `createSnapshot` — `POST /snapshots` from a registry image.
    pub async fn create_snapshot(
        &self,
        name: &str,
        image: &str,
        cpu: u32,
        memory: u32,
        disk: u32,
        entrypoint: Option<Vec<String>>,
    ) -> Result<Snapshot> {
        let mut body =
            json!({ "name": name, "imageName": image, "cpu": cpu, "memory": memory, "disk": disk });
        if let Some(e) = entrypoint {
            body["entrypoint"] = json!(e);
        }
        self.send(
            self.req(reqwest::Method::POST, "/snapshots").json(&body),
            "create-snapshot",
        )
        .await
    }

    /// `getSnapshot` — `GET /snapshots/{id}`.
    pub async fn get_snapshot(&self, id_or_name: &str) -> Result<Snapshot> {
        self.send(
            self.req(reqwest::Method::GET, &format!("/snapshots/{id_or_name}")),
            "get-snapshot",
        )
        .await
    }

    /// Poll `getSandbox` until `pred` holds or `timeout` passes.
    pub async fn wait_for(
        &self,
        id: &str,
        timeout: Duration,
        pred: impl Fn(&Sandbox) -> bool,
    ) -> Result<Sandbox> {
        let start = std::time::Instant::now();
        loop {
            let s = self.get(id).await?;
            if pred(&s) {
                return Ok(s);
            }
            if matches!(s.state, SandboxState::Error | SandboxState::BuildFailed) {
                return Err(anyhow!(
                    "sandbox {id} entered {:?}: {}",
                    s.state,
                    s.error_reason.unwrap_or_default()
                ));
            }
            if start.elapsed() > timeout {
                return Err(anyhow!(
                    "sandbox {id} still {:?} after {:?}",
                    s.state,
                    timeout
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // ---- toolbox (runner-injected HTTP API inside the sandbox; sandbox-image.md §1) -----------
    //
    // The toolbox is reached through the proxy host (`GET /sandbox/{id}/toolbox-proxy-url`, today
    // `https://proxy.app.daytona.io/toolbox`) as `{proxy}/{sandboxId}/<toolbox path>` with the same
    // API key. The `/api/toolbox/...` form in the OpenAPI document returns 404 (verified 2026-10-15).

    async fn toolbox_base(&self, id: &str) -> String {
        let fallback = "https://proxy.app.daytona.io/toolbox".to_owned();
        let url = match self
            .send::<Value>(
                self.req(
                    reqwest::Method::GET,
                    &format!("/sandbox/{id}/toolbox-proxy-url"),
                ),
                "toolbox proxy url",
            )
            .await
        {
            Ok(v) => v
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(fallback),
            Err(_) => fallback,
        };
        format!("{}/{id}", url.trim_end_matches('/'))
    }

    /// A handle on one sandbox's toolbox with the proxy base resolved once — for runs of many calls
    /// (the box upgrade's part-wise carry) that would otherwise ask for the proxy URL every time.
    pub async fn toolbox(&self, id: &str) -> Toolbox<'_> {
        Toolbox {
            dt: self,
            base: self.toolbox_base(id).await,
        }
    }

    /// `POST {proxy}/{sandboxId}/files/upload?path=` — write one file into a started sandbox (parent
    /// directories are created). Used to hand the box the current relay URL at every wake.
    pub async fn toolbox_upload(&self, id: &str, path: &str, bytes: Vec<u8>) -> Result<()> {
        self.toolbox(id)
            .await
            .upload(path, bytes, Duration::from_secs(30))
            .await
    }

    /// `GET {proxy}/{sandboxId}/files/download?path=` — read one file out of a started sandbox.
    pub async fn toolbox_download(&self, id: &str, path: &str) -> Result<Vec<u8>> {
        self.toolbox(id)
            .await
            .download(path, Duration::from_secs(600))
            .await
    }

    /// `POST {proxy}/{sandboxId}/process/execute`.
    pub async fn toolbox_exec(&self, id: &str, command: &str, timeout_s: u64) -> Result<Value> {
        self.toolbox(id).await.exec(command, timeout_s).await
    }

    fn toolbox_auth(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let r = r.bearer_auth(&self.key);
        match &self.org {
            Some(org) => r.header("X-Daytona-Organization-ID", org),
            None => r,
        }
    }

    /// `POST /toolbox/{sandboxId}/toolbox/process/session` then `.../session/{sessionId}/exec` with `runAsync`.
    pub async fn toolbox_session_exec_async(
        &self,
        id: &str,
        session: &str,
        command: &str,
    ) -> Result<Value> {
        let base = self.toolbox_base(id).await;
        let _ = self
            .toolbox_auth(self.http.post(format!("{base}/process/session")))
            .json(&json!({ "sessionId": session }))
            .send()
            .await;
        self.send(
            self.toolbox_auth(
                self.http
                    .post(format!("{base}/process/session/{session}/exec")),
            )
            .json(&json!({ "command": command, "runAsync": true })),
            "toolbox session exec",
        )
        .await
    }
}

/// One sandbox's toolbox, proxy base already resolved. Every call takes its own deadline: the
/// client default (120 s) is far too short for a part of `/work` and far too long for a `df`.
pub struct Toolbox<'a> {
    dt: &'a Daytona,
    base: String,
}

impl Toolbox<'_> {
    /// `POST .../files/upload?path=` with `deadline` for the whole request.
    pub async fn upload(&self, path: &str, bytes: Vec<u8>, deadline: Duration) -> Result<()> {
        let part = reqwest::multipart::Part::bytes(bytes).file_name("file");
        let form = reqwest::multipart::Form::new().part("file", part);
        let r = self
            .dt
            .toolbox_auth(self.dt.http.post(format!("{}/files/upload", self.base)))
            .query(&[("path", path)])
            .multipart(form)
            .timeout(deadline);
        let resp = r.send().await.context("toolbox upload")?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "toolbox upload: HTTP {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        Ok(())
    }

    /// `GET .../files/download?path=` with `deadline` for the whole request.
    pub async fn download(&self, path: &str, deadline: Duration) -> Result<Vec<u8>> {
        let r = self
            .dt
            .toolbox_auth(self.dt.http.get(format!("{}/files/download", self.base)))
            .query(&[("path", path)])
            .timeout(deadline);
        let resp = r.send().await.context("toolbox download")?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "toolbox download: HTTP {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        Ok(resp
            .bytes()
            .await
            .context("toolbox download body")?
            .to_vec())
    }

    /// `POST .../process/execute` — `timeout_s` is the box-side limit; the request gets 30 s more.
    pub async fn exec(&self, command: &str, timeout_s: u64) -> Result<Value> {
        let r = self
            .dt
            .toolbox_auth(self.dt.http.post(format!("{}/process/execute", self.base)))
            .json(&json!({ "command": command, "timeout": timeout_s }))
            .timeout(Duration::from_secs(timeout_s + 30));
        self.dt.send(r, "toolbox exec").await
    }
}
