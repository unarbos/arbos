//! Relay-backed source: HTTP for the registry (`/pair`, `/projects`, `/blob`)
//! and one WebSocket per room on `arbos-relay` (`crdt-sync.md` §7–§9).
//! Frames come from `arbos_core::wire`; the chat doc is the same Loro schema
//! `arbos-core::doc` writes on the box side. Each project also has a project
//! doc room (`{project}/project`) that carries `threads`, `status`,
//! `settings` and `memory`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use arbos_core::wire::{
    Ack, BoxState as WireBox, ErrorHeader, Frame, FrameType, Hello, RowHeader, RowsReq,
    State as WireState, StatusHeader, WakeHeader, PROJECT_DOC_CHAT_ID,
};
use arbos_core::{wire, PROTO_VERSION};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep, timeout, Instant};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::credentials::{self, DeviceCredentials};
use super::doc::ChatDoc;
use super::store::{self, Store};
use super::{ClientCommand, SyncEvent, SyncSource};
use crate::model::{
    new_id, now_ms, AccountView, AgentMember, Attachment, AttachmentKind, BoxState, Call,
    CallEnded, CallLeft, CallMissed, ChannelBinding, ChannelPairing, ChannelPlatform, ChannelsView,
    Chat, ChatKind, Connector, ConnectorProvider, ConnectorsView, Effort, Id, Link, LocalDevice,
    LocalView, Machine, MachineState, Message, MessageStatus, Part, Person, Project,
    ProjectDocView, ProjectSecrets, Reachable, ReplyPolicy, Role, Routine, RoutineRun, Secret,
    SecretActivity, SecretRequest, SecretsView, Sender, Skill, SpaceKind, ThreadInfo, Voice,
};
use crate::update;

const HELLO_DEADLINE: Duration = Duration::from_secs(15);
const PRESENCE_EVERY: Duration = Duration::from_secs(15);
const REGISTRY_POLL: Duration = Duration::from_secs(15);
/// `GET /machine` is the source of truth for the rail indicator and Settings.
const MACHINE_POLL: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(16);
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(30);

pub struct RelaySource {
    http_url: String,
    ws_url: String,
}

impl RelaySource {
    /// Accepts `http://`, `https://`, `ws://` or `wss://`; derives the other.
    pub fn from_env(url: String) -> Self {
        let url = url.trim().trim_end_matches('/').to_string();
        let (http_url, ws_url) = if let Some(rest) = url.strip_prefix("ws://") {
            (format!("http://{rest}"), url.clone())
        } else if let Some(rest) = url.strip_prefix("wss://") {
            (format!("https://{rest}"), url.clone())
        } else if let Some(rest) = url.strip_prefix("https://") {
            (url.clone(), format!("wss://{rest}"))
        } else if let Some(rest) = url.strip_prefix("http://") {
            (url.clone(), format!("ws://{rest}"))
        } else {
            (format!("http://{url}"), format!("ws://{url}"))
        };
        Self { http_url, ws_url }
    }
}

enum RoomCmd {
    Wake(&'static str),
    /// Presence frame with `typing` (people chats, D53).
    Typing(bool),
    /// Presence frame now (the focused chat changed, API §7c).
    Presence,
}

struct Room {
    cmds: mpsc::UnboundedSender<RoomCmd>,
    task: JoinHandle<()>,
}

struct Shared {
    http: reqwest::Client,
    http_url: String,
    ws_url: String,
    creds: RwLock<Option<DeviceCredentials>>,
    /// Chat docs keyed by chat id; project docs keyed by `proj:{project_id}`.
    docs: Mutex<HashMap<Id, ChatDoc>>,
    rooms: Mutex<HashMap<Id, Room>>,
    projects: Mutex<Vec<Project>>,
    chats: Mutex<Vec<Chat>>,
    /// Client-minted message ids whose push the relay has not `ACK`ed (S3).
    unacked: Mutex<HashSet<Id>>,
    events: mpsc::UnboundedSender<SyncEvent>,
    subscriptions: Mutex<Vec<loro::Subscription>>,
    machine: Mutex<Machine>,
    refresh_lock: tokio::sync::Mutex<()>,
    /// People chat ids (rooms live under `/room/people/{id}`).
    people: Mutex<HashSet<Id>>,
    /// Highest relay seq seen per room key (read marks, D61).
    room_seq: Mutex<HashMap<Id, i64>>,
    /// Avatars already fetched (by URL).
    avatars_done: Mutex<HashSet<String>>,
    /// `/me` user when the stored credentials carry none (env token, legacy adv_).
    me: Mutex<Option<credentials::AccountUser>>,
    /// The chat on screen in a focused window; presence frames carry it as
    /// `focused_chat` so the relay does not push this device for it (§7c).
    focused_chat: Mutex<Option<Id>>,
    /// Calls v1: `in_call` on every presence frame while this device is in one (§8).
    in_call: Mutex<Option<String>>,
    /// On-device store (#929); `None` without credentials or with `ARBOS_NO_STORE=1`.
    store: Mutex<Option<Store>>,
    /// Room keys whose doc or cursor changed since the last save.
    dirty: Mutex<HashSet<Id>>,
}

fn project_doc_key(project_id: &str) -> String {
    format!("proj:{project_id}")
}

impl Shared {
    /// `{device, at, focused_chat?}` — every presence frame (15 s tick, typing,
    /// focus change) says which chat this device is looking at, if any.
    fn presence_header(&self) -> Value {
        let mut header = json!({
            "device": self.device_id(),
            "at": now_ms(),
            "user": self.me_id(),
            "handle": self.me_handle(),
        });
        if let Some(chat) = self.focused_chat.lock().unwrap().as_deref() {
            header["focused_chat"] = json!(chat);
        }
        if let Some(call) = self.in_call.lock().unwrap().as_deref() {
            header["in_call"] = json!(call);
        }
        header
    }

    fn token(&self) -> String {
        self.creds
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.access_token.clone())
            .unwrap_or_default()
    }

    /// `POST /auth/refresh` with the stored refresh token; the relay rotates
    /// it, so this is serialised and never retried with a used token. A 401
    /// here means the device was revoked: back to the sign-in screen.
    ///
    /// Single-flight: `stale_access` is the access token the caller was using when it was
    /// refused. Requests that were refused at the same moment queue on the lock, and every one
    /// after the first finds the token already replaced and returns at once — one rotation per
    /// expiry instead of one per in-flight request (the refresh storm at launch).
    async fn refresh_tokens(self: &Arc<Self>, stale_access: Option<&str>) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let (current_access, refresh, due) = {
            let creds = self.creds.read().unwrap();
            let Some(c) = creds.as_ref() else {
                return Err(anyhow!("no refresh token"));
            };
            (
                c.access_token.clone(),
                c.refresh_token.clone(),
                c.needs_refresh(),
            )
        };
        let Some(refresh) = refresh else {
            return Err(anyhow!("no refresh token"));
        };
        match stale_access {
            // Someone refreshed while we waited for the lock: the new token is already stored.
            Some(stale) if stale != current_access => return Ok(()),
            // The periodic refresher: nothing to do when a 401-driven refresh just happened.
            None if !due => return Ok(()),
            _ => {}
        }
        let resp = self
            .http
            .post(format!("{}/auth/refresh", self.http_url))
            .json(&json!({ "refresh_token": refresh }))
            .send()
            .await
            .context("POST /auth/refresh")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            self.sign_out_locally();
            return Err(anyhow!("refresh rejected: {text}"));
        }
        if !status.is_success() {
            return Err(anyhow!("POST /auth/refresh: HTTP {status} {text}"));
        }
        let v: Value = serde_json::from_str(&text).context("refresh: bad JSON")?;
        self.apply_token_response(&v)?;
        Ok(())
    }

    /// Store the body of `/auth/token` or `/auth/refresh`.
    fn apply_token_response(&self, v: &Value) -> Result<DeviceCredentials> {
        let access = v
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow!("relay returned no access token"))?;
        let expires_in = v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
        let previous = self.creds.read().unwrap().clone();
        let user = v
            .get("user")
            .and_then(|u| serde_json::from_value(u.clone()).ok())
            .or_else(|| previous.as_ref().and_then(|c| c.user.clone()));
        let creds = DeviceCredentials {
            relay_url: self.http_url.clone(),
            device_id: v
                .get("device_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| previous.as_ref().map(|c| c.device_id.clone()))
                .unwrap_or_else(|| "device".into()),
            access_token: access.to_string(),
            refresh_token: v
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| previous.and_then(|c| c.refresh_token)),
            expires_at: now_ms() + expires_in * 1000,
            user,
        };
        if let Err(err) = credentials::store(&creds) {
            self.notice(format!("Token saved in memory only: {err:#}"));
        }
        *self.creds.write().unwrap() = Some(creds.clone());
        Ok(creds)
    }

    fn me_user(&self) -> Option<credentials::AccountUser> {
        self.creds
            .read()
            .unwrap()
            .as_ref()
            .and_then(|c| c.user.clone())
            .or_else(|| self.me.lock().unwrap().clone())
    }

    fn me_id(&self) -> String {
        self.me_user().map(|u| u.id).unwrap_or_default()
    }

    fn me_handle(&self) -> String {
        self.me_user().map(|u| u.login).unwrap_or_default()
    }

    /// Make sure we know who we are before reading chat members (`/me` once).
    async fn ensure_me(self: &Arc<Self>) {
        if self.me_user().is_some() {
            return;
        }
        if let Ok(v) = self.get_json("/me").await {
            if let Some(user) = v
                .get("user")
                .and_then(|u| serde_json::from_value(u.clone()).ok())
            {
                *self.me.lock().unwrap() = Some(user);
            }
        }
    }

    fn is_people(&self, chat_id: &str) -> bool {
        self.people.lock().unwrap().contains(chat_id)
    }

    async fn delete(self: &Arc<Self>, path: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!("{}{}", self.http_url, path))
            .bearer_auth(self.token())
            .send()
            .await
            .with_context(|| format!("DELETE {path}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("DELETE {path}: HTTP {status} {text}"));
        }
        Ok(())
    }

    async fn put_json(self: &Arc<Self>, path: &str, body: Value) -> Result<Value> {
        let resp = self
            .http
            .put(format!("{}{}", self.http_url, path))
            .bearer_auth(self.token())
            .json(&body)
            .send()
            .await
            .with_context(|| format!("PUT {path}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("PUT {path}: HTTP {status} {text}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Forget the tokens and show the sign-in screen.
    fn sign_out_locally(&self) {
        credentials::clear(&self.http_url);
        self.drop_store(true);
        *self.creds.write().unwrap() = None;
        let _ = self.events.send(SyncEvent::Account(None));
        let _ = self.events.send(SyncEvent::NeedsPairing {
            relay_url: self.http_url.clone(),
        });
    }

    /// Run `f` against the store when there is one.
    fn with_store<R>(&self, f: impl FnOnce(&Store) -> R) -> Option<R> {
        self.store.lock().unwrap().as_ref().map(f)
    }

    /// A room's doc or cursor changed: the flusher saves it on its next tick.
    fn mark_dirty(&self, key: &str) {
        if self.store.lock().unwrap().is_some() {
            self.dirty.lock().unwrap().insert(key.to_string());
        }
    }

    /// Close the store; `wipe` also deletes the file (sign-out, revoked token).
    fn drop_store(&self, wipe: bool) {
        let taken = self.store.lock().unwrap().take();
        if let (true, Some(s)) = (wipe, taken) {
            let path = s.path().to_path_buf();
            drop(s);
            store::remove_files(&path);
        }
    }

    fn is_token_expired(status: reqwest::StatusCode, body: &str) -> bool {
        status.as_u16() == 401 && body.contains("token_expired")
    }

    fn device_id(&self) -> String {
        self.creds
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.device_id.clone())
            .unwrap_or_else(|| "mac:unpaired".into())
    }

    fn notice(&self, text: impl Into<String>) {
        let _ = self.events.send(SyncEvent::Notice(text.into()));
    }

    async fn get_json(self: &Arc<Self>, path: &str) -> Result<Value> {
        let mut refreshed = false;
        loop {
            let token = self.token();
            let resp = self
                .http
                .get(format!("{}{}", self.http_url, path))
                .bearer_auth(&token)
                .send()
                .await
                .with_context(|| format!("GET {path}"))?;
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if Self::is_token_expired(status, &body) && !refreshed {
                refreshed = true;
                self.refresh_tokens(Some(&token)).await?;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("GET {path}: HTTP {status} {body}"));
            }
            return serde_json::from_str(&body).with_context(|| format!("GET {path}: bad JSON"));
        }
    }

    /// Like [`post_json`] but keeps the status (201 "created" vs 200 "joined" for calls)
    /// and sends `extra` headers (`X-Arbos-Client` on `POST …/calls`, contract §8).
    async fn post_json_status(
        self: &Arc<Self>,
        path: &str,
        body: Value,
        extra: &[(&str, String)],
    ) -> Result<(reqwest::StatusCode, Value)> {
        let mut refreshed = false;
        loop {
            let token = self.token();
            let mut req = self
                .http
                .post(format!("{}{}", self.http_url, path))
                .bearer_auth(&token)
                .json(&body);
            for (name, value) in extra {
                req = req.header(*name, value);
            }
            let resp = req.send().await.with_context(|| format!("POST {path}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if Self::is_token_expired(status, &text) && !refreshed {
                refreshed = true;
                self.refresh_tokens(Some(&token)).await?;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("POST {path}: HTTP {status} {text}"));
            }
            let value = if text.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&text).unwrap_or(Value::Null)
            };
            return Ok((status, value));
        }
    }

    async fn post_json(self: &Arc<Self>, path: &str, body: Value) -> Result<Value> {
        let mut refreshed = false;
        loop {
            let token = self.token();
            let resp = self
                .http
                .post(format!("{}{}", self.http_url, path))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .with_context(|| format!("POST {path}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if Self::is_token_expired(status, &text) && !refreshed {
                refreshed = true;
                self.refresh_tokens(Some(&token)).await?;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("POST {path}: HTTP {status} {text}"));
            }
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
        }
    }

    async fn patch_json(self: &Arc<Self>, path: &str, body: Value) -> Result<Value> {
        let mut refreshed = false;
        loop {
            let token = self.token();
            let resp = self
                .http
                .patch(format!("{}{}", self.http_url, path))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .with_context(|| format!("PATCH {path}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if Self::is_token_expired(status, &text) && !refreshed {
                refreshed = true;
                self.refresh_tokens(Some(&token)).await?;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("PATCH {path}: HTTP {status} {text}"));
            }
            return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
        }
    }
}

impl SyncSource for RelaySource {
    fn name(&self) -> &'static str {
        "relay"
    }

    fn run(
        self: Box<Self>,
        events: mpsc::UnboundedSender<SyncEvent>,
        mut commands: mpsc::UnboundedReceiver<ClientCommand>,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let _ = SOURCE_START.set(Instant::now());
            let shared = Arc::new(Shared {
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .context("http client")?,
                http_url: self.http_url.clone(),
                ws_url: self.ws_url.clone(),
                creds: RwLock::new(credentials::load(&self.http_url)),
                docs: Mutex::new(HashMap::new()),
                rooms: Mutex::new(HashMap::new()),
                focused_chat: Mutex::new(None),
                in_call: Mutex::new(None),
                projects: Mutex::new(Vec::new()),
                chats: Mutex::new(Vec::new()),
                unacked: Mutex::new(HashSet::new()),
                events: events.clone(),
                subscriptions: Mutex::new(Vec::new()),
                machine: Mutex::new(Machine::default()),
                refresh_lock: tokio::sync::Mutex::new(()),
                people: Mutex::new(HashSet::new()),
                room_seq: Mutex::new(HashMap::new()),
                avatars_done: Mutex::new(HashSet::new()),
                me: Mutex::new(None),
                store: Mutex::new(None),
                dirty: Mutex::new(HashSet::new()),
            });
            let _ = events.send(SyncEvent::Link(Link::Connecting));

            // Rig hook (D82 first-login recordings): `ARBOS_TEST_TOKENS=<file>` holds a
            // `/auth/token` response for a test account; it is taken as a fresh sign-in
            // regardless of what the keychain holds, and is not kept in the keychain.
            let test_tokens = std::env::var("ARBOS_TEST_TOKENS")
                .ok()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| {
                    *shared.creds.write().unwrap() = None;
                    let creds = shared.apply_token_response(&v).ok()?;
                    credentials::clear(&shared.http_url);
                    Some(creds)
                });
            if let Some(creds) = test_tokens {
                if let Some(user) = creds.user.clone() {
                    let _ = shared.events.send(SyncEvent::Account(Some(AccountView {
                        id: user.id,
                        login: user.login,
                        name: user.name,
                        avatar_url: user.avatar_url,
                        ..AccountView::default()
                    })));
                }
                let _ = shared.events.send(SyncEvent::Paired {
                    device_id: creds.device_id,
                });
                start_after_pairing(&shared);
            } else if shared.creds.read().unwrap().is_some() {
                start_after_pairing(&shared);
            } else {
                let _ = events.send(SyncEvent::NeedsPairing {
                    relay_url: self.http_url.clone(),
                });
            }

            while let Some(cmd) = commands.recv().await {
                handle_command(&shared, cmd);
            }
            Ok(())
        })
    }
}

/// `GET /skills` → `SyncEvent::Skills` (F5).
async fn refresh_skills(shared: &Arc<Shared>) {
    match shared.get_json("/skills").await {
        Ok(v) => {
            let skills: Vec<Skill> = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| serde_json::from_value::<Skill>(s.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            let _ = shared.events.send(SyncEvent::Skills(skills));
        }
        Err(err) => shared.notice(format!("Skills: {err:#}")),
    }
}

/// `GET /me` → `SyncEvent::Account`.
async fn refresh_me(shared: &Arc<Shared>) {
    let me = shared.get_json("/me").await;
    if let Err(err) = &me {
        eprintln!("relay: GET /me: {err:#}");
    }
    let Ok(v) = me else {
        // Pair-code relays have no /me; the stored user (if any) stands.
        let user = shared
            .creds
            .read()
            .unwrap()
            .as_ref()
            .and_then(|c| c.user.clone());
        if let Some(user) = user {
            let _ = shared.events.send(SyncEvent::Account(Some(AccountView {
                id: user.id,
                login: user.login,
                name: user.name,
                avatar_url: user.avatar_url,
                ..AccountView::default()
            })));
        }
        return;
    };
    let user = v.get("user").cloned().unwrap_or(Value::Null);
    if let Ok(u) = serde_json::from_value::<credentials::AccountUser>(user.clone()) {
        shared.with_store(|s| s.put_json(store::KEY_ACCOUNT, &u));
        *shared.me.lock().unwrap() = Some(u);
    }
    let credits = v.get("credits").cloned().unwrap_or(Value::Null);
    let str_of = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let view = AccountView {
        id: str_of(&user, "id"),
        login: str_of(&user, "login"),
        name: str_of(&user, "name"),
        avatar_url: str_of(&user, "avatar_url"),
        device_name: v
            .pointer("/device/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        balance_usd: credits
            .get("balance_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        spent_today_usd: credits
            .get("spent_today_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        daily_cap_usd: credits.get("daily_cap_usd").and_then(Value::as_f64),
    };
    let _ = shared.events.send(SyncEvent::Account(Some(view)));
    if let Some(m) = v.get("machine").and_then(Value::as_object) {
        if let Some(always_on) = m.get("always_on").and_then(Value::as_bool) {
            let snapshot = {
                let mut machine = shared.machine.lock().unwrap();
                machine.always_on = always_on;
                machine.clone()
            };
            let _ = shared.events.send(SyncEvent::Machine(snapshot));
        }
        if m.get("exists").and_then(Value::as_bool) != Some(false) {
            if let Some(word) = m.get("state").and_then(Value::as_str) {
                let since = m.get("since").and_then(Value::as_u64).filter(|v| *v > 0);
                set_machine(shared, machine_state_from_word(word), since, None);
            }
        }
    }
}

/// Keep the JWT fresh: the relay wants a refresh when less than five minutes
/// remain; check twice a minute.
fn start_token_refresher(shared: &Arc<Shared>) {
    let shared = shared.clone();
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            let due = shared
                .creds
                .read()
                .unwrap()
                .as_ref()
                .map(|c| c.needs_refresh())
                .unwrap_or(false);
            if due {
                if let Err(err) = shared.refresh_tokens(None).await {
                    shared.notice(format!("Session refresh: {err:#}"));
                }
            }
            if shared.creds.read().unwrap().is_none() {
                return;
            }
        }
    });
}

fn start_after_pairing(shared: &Arc<Shared>) {
    start_token_refresher(shared);
    open_store(shared);
    load_store(shared);
    start_store_flusher(shared);
    {
        let shared = shared.clone();
        tokio::spawn(async move { refresh_me(&shared).await });
    }
    let skills_shared = shared.clone();
    tokio::spawn(async move { refresh_skills(&skills_shared).await });
    let models_shared = shared.clone();
    tokio::spawn(async move {
        if let Ok(v) = models_shared.get_json("/proxy/v1/models").await {
            let mut ids: Vec<String> = v
                .get("data")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.get("id").and_then(Value::as_str))
                        .filter(|id| !id.contains(':'))
                        .filter(|id| {
                            id.starts_with("anthropic/")
                                || id.starts_with("openai/")
                                || id.starts_with("google/")
                                || id.starts_with("x-ai/")
                        })
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            ids.sort();
            ids.dedup();
            if !ids.is_empty() {
                let _ = models_shared.events.send(SyncEvent::Models(ids));
            }
        }
    });
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            refresh_connectors(&shared).await;
            refresh_secrets(&shared).await;
        });
    }
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut poll = interval(MACHINE_POLL);
            loop {
                poll.tick().await;
                refresh_machine(&shared).await;
                poll_me_pushes(&shared).await;
            }
        });
    }
    let shared = shared.clone();
    tokio::spawn(async move {
        let mut poll = interval(REGISTRY_POLL);
        loop {
            poll.tick().await;
            // Pending secret requests show up without a manual refresh.
            refresh_secrets(&shared).await;
            if let Err(err) = refresh_registry(&shared).await {
                let text = format!("{err:#}");
                if text.contains("HTTP 401") {
                    // Token revoked or relay re-initialised: pair again.
                    shared.drop_store(true);
                    *shared.creds.write().unwrap() = None;
                    let _ = shared.events.send(SyncEvent::NeedsPairing {
                        relay_url: shared.http_url.clone(),
                    });
                    return;
                }
                shared.notice(format!("Relay: {text}"));
                let _ = shared.events.send(SyncEvent::Link(Link::Reconnecting));
            }
        }
    });
}

// ---------------------------------------------------------------------------
// On-device store (#929)
// ---------------------------------------------------------------------------

/// How often dirty rooms are written (coalesces streaming frames).
const STORE_FLUSH_EVERY: Duration = Duration::from_millis(1500);

/// When the source started (≈ process start): launch timing for the log.
static SOURCE_START: OnceLock<Instant> = OnceLock::new();
static FIRST_PAINT_LOGGED: AtomicBool = AtomicBool::new(false);

/// `launch: first-content <ms> ms source=store|relay` once per process, when
/// the first non-empty project list goes to the UI.
fn log_first_paint(source: &str, projects: usize) {
    if projects == 0 || FIRST_PAINT_LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    let ms = SOURCE_START
        .get()
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0);
    eprintln!("launch: first-content {ms} ms source={source} projects={projects}");
}

fn open_store(shared: &Arc<Shared>) {
    let device_id = shared.device_id();
    let mut slot = shared.store.lock().unwrap();
    if slot.is_none() {
        *slot = Store::open(&shared.http_url, &device_id);
    }
}

/// Paint everything the last run saved — rail, tabs, people, every transcript,
/// the open chat — before any request leaves. Rooms then dial with their
/// saved cursor and only backfill what is new.
fn load_store(shared: &Arc<Shared>) {
    let started = std::time::Instant::now();
    let Some((projects, chats, people, docs, pending, selection, account)) =
        shared.with_store(|s| {
            (
                s.get_json::<Vec<Project>>(store::KEY_PROJECTS)
                    .unwrap_or_default(),
                s.get_json::<Vec<Chat>>(store::KEY_CHATS)
                    .unwrap_or_default(),
                s.get_json::<Vec<Id>>(store::KEY_PEOPLE).unwrap_or_default(),
                s.load_docs(),
                s.load_pending(),
                s.get_json::<(Id, Id)>(store::KEY_SELECTION),
                s.get_json::<credentials::AccountUser>(store::KEY_ACCOUNT),
            )
        })
    else {
        return;
    };
    if let Some((last_chats, archived)) = shared.with_store(|s| {
        (
            s.get_json::<HashMap<Id, Id>>(store::KEY_LAST_CHATS)
                .unwrap_or_default(),
            s.get_json::<Vec<Id>>(store::KEY_ARCHIVED_CHATS)
                .unwrap_or_default(),
        )
    }) {
        let _ = shared.events.send(SyncEvent::ChatPrefs {
            last_chats,
            archived,
        });
    }
    if let Some(u) = account {
        // Who is signed in, before `/me` answers (and when it cannot).
        *shared.me.lock().unwrap() = Some(u.clone());
        let _ = shared.events.send(SyncEvent::Account(Some(AccountView {
            id: u.id,
            login: u.login,
            name: u.name,
            avatar_url: u.avatar_url,
            ..AccountView::default()
        })));
    }
    if projects.is_empty() {
        return;
    }
    shared.people.lock().unwrap().extend(people);
    *shared.projects.lock().unwrap() = projects.clone();
    *shared.chats.lock().unwrap() = chats;
    let _ = shared.events.send(SyncEvent::Projects(projects.clone()));
    log_first_paint("store", projects.len());
    emit_chats(shared);
    let mut pending = pending;
    let mut restored = 0usize;
    for saved in docs {
        let doc = ChatDoc::empty(&saved.key);
        if let Err(err) = doc.doc.import(&saved.snapshot) {
            eprintln!("store: snapshot for {} unreadable: {err}", saved.key);
            shared.with_store(|s| s.delete_doc(&saved.key));
            continue;
        }
        // A doc the registry no longer lists (closed elsewhere) is dropped.
        let known = saved.room_chat == PROJECT_DOC_CHAT_ID
            && projects.iter().any(|p| p.id == saved.project_id)
            || shared
                .chats
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.id == saved.key);
        if !known {
            shared.with_store(|s| s.delete_doc(&saved.key));
            continue;
        }
        let pending = pending.remove(&saved.key).unwrap_or_default();
        ensure_room_with(
            shared,
            &saved.project_id,
            &saved.key,
            &saved.room_chat,
            Some(Restored {
                doc,
                cursor: saved.cursor,
                pending,
            }),
        );
        restored += 1;
    }
    if let Some((project_id, chat_id)) = selection {
        if projects.iter().any(|p| p.id == project_id) {
            let _ = shared.events.send(SyncEvent::RestoreSelection {
                project_id,
                chat_id,
            });
        }
    }
    emit_store_usage(shared);
    eprintln!(
        "store: restored {} projects, {restored} docs in {} ms",
        projects.len(),
        started.elapsed().as_millis()
    );
}

/// Registry JSON after every successful refresh.
fn save_registry(shared: &Arc<Shared>) {
    let projects = shared.projects.lock().unwrap().clone();
    let chats = shared.chats.lock().unwrap().clone();
    let people: Vec<Id> = shared.people.lock().unwrap().iter().cloned().collect();
    shared.with_store(|s| {
        if projects.is_empty() {
            s.remove(store::KEY_PROJECTS);
            s.remove(store::KEY_CHATS);
        } else {
            s.put_json(store::KEY_PROJECTS, &projects);
            s.put_json(store::KEY_CHATS, &chats);
        }
        s.put_json(store::KEY_PEOPLE, &people);
    });
}

fn emit_store_usage(shared: &Arc<Shared>) {
    if let Some(u) = shared.with_store(|s| s.usage()) {
        let _ = shared.events.send(SyncEvent::StoreUsage {
            bytes: u.bytes,
            chats: u.docs,
        });
    }
}

/// Every 1.5 s: write each dirty room's snapshot with its cursor. The cursor
/// is read before the export, so a saved snapshot always covers its cursor.
fn start_store_flusher(shared: &Arc<Shared>) {
    let shared = shared.clone();
    tokio::spawn(async move {
        let mut tick = interval(STORE_FLUSH_EVERY);
        let mut since_caps = 0u32;
        loop {
            tick.tick().await;
            if shared.store.lock().unwrap().is_none() {
                if shared.creds.read().unwrap().is_none() {
                    return;
                }
                continue;
            }
            let keys: Vec<Id> = shared.dirty.lock().unwrap().drain().collect();
            if keys.is_empty() {
                continue;
            }
            let rooms: HashMap<Id, (Id, String)> = {
                let projects = shared.projects.lock().unwrap();
                let chats = shared.chats.lock().unwrap();
                keys.iter()
                    .filter_map(|k| {
                        if let Some(pid) = k.strip_prefix("proj:") {
                            projects.iter().any(|p| p.id == pid).then(|| {
                                (
                                    k.clone(),
                                    (pid.to_string(), PROJECT_DOC_CHAT_ID.to_string()),
                                )
                            })
                        } else {
                            chats
                                .iter()
                                .find(|c| &c.id == k)
                                .map(|c| (k.clone(), (c.project_id.clone(), c.id.clone())))
                        }
                    })
                    .collect()
            };
            for (key, (project_id, room_chat)) in rooms {
                let cursor = shared
                    .room_seq
                    .lock()
                    .unwrap()
                    .get(&key)
                    .copied()
                    .unwrap_or(0);
                let snapshot = {
                    let docs = shared.docs.lock().unwrap();
                    let Some(doc) = docs.get(&key) else { continue };
                    if doc.doc.oplog_frontiers().is_empty() {
                        continue;
                    }
                    doc.doc.export(loro::ExportMode::Snapshot)
                };
                match snapshot {
                    Ok(bytes) => {
                        shared.with_store(|s| {
                            s.save_doc(&key, &project_id, &room_chat, cursor, &bytes)
                        });
                    }
                    Err(err) => eprintln!("store: export {key}: {err}"),
                }
            }
            since_caps += 1;
            if since_caps >= 20 {
                since_caps = 0;
                shared.with_store(|s| s.enforce_caps());
            }
            emit_store_usage(&shared);
        }
    });
}

// ---------------------------------------------------------------------------
// Registry (HTTP)
// ---------------------------------------------------------------------------

fn effort_from(v: &Value) -> Effort {
    match v.as_str().unwrap_or("") {
        "high" | "xhigh" | "max" => Effort::High,
        "medium" => Effort::Medium,
        _ => Effort::Low,
    }
}

fn effort_to(e: Effort) -> &'static str {
    match e {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
    }
}

fn box_state_from(state: &str, box_state: &str) -> BoxState {
    match state {
        "awake" => BoxState::Awake,
        "waking" => BoxState::Waking,
        "hibernated_light" => BoxState::HibernatedLight,
        "hibernated_deep" => BoxState::HibernatedDeep,
        "stopped_by_user" => BoxState::StoppedByUser,
        "error" => BoxState::Error,
        _ => match box_state {
            "awake" => BoxState::Awake,
            "waking" => BoxState::Waking,
            "error" => BoxState::Error,
            _ => BoxState::HibernatedLight,
        },
    }
}

/// `GET /chats` item → a rail space (kind dm/group) and its one chat.
fn people_space_from(shared: &Shared, v: &Value) -> Option<(Project, Chat)> {
    let id = v.get("id")?.as_str()?.to_string();
    let me = shared.me_id();
    let mut people: Vec<Person> = Vec::new();
    let mut agents: Vec<AgentMember> = Vec::new();
    for m in v
        .get("members")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let member = m.get("member").and_then(Value::as_str).unwrap_or("");
        if let Some(uid) = member.strip_prefix("user:") {
            if uid == me {
                continue;
            }
            people.push(Person {
                id: uid.to_string(),
                handle: m
                    .get("handle")
                    .and_then(Value::as_str)
                    .unwrap_or(uid)
                    .to_string(),
                name: m.get("name").and_then(Value::as_str).map(str::to_string),
                avatar_url: m
                    .get("avatar_url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                status: m
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("member")
                    .to_string(),
                role: m
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("owner")
                    .to_string(),
                last_seen_at: m.get("last_seen_at").and_then(Value::as_i64),
                online: m.get("online").and_then(Value::as_bool).unwrap_or(false),
                avatar_color_index: m
                    .get("avatar_color_index_effective")
                    .or_else(|| m.get("avatar_color_index"))
                    .and_then(Value::as_u64)
                    .map(|i| (i % 7) as u8),
            });
        } else if let Some(pid) = member.strip_prefix("agent:") {
            let project = m.get("project").cloned().unwrap_or(Value::Null);
            let name = project
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Agent")
                .to_string();
            agents.push(AgentMember {
                project_id: pid.to_string(),
                handle: m
                    .get("handle")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| Project::mention_slug(&name)),
                icon: project
                    .get("icon")
                    .and_then(Value::as_str)
                    .unwrap_or("Ω")
                    .to_string(),
                color: project
                    .get("color")
                    .and_then(Value::as_str)
                    .unwrap_or("#8b7cf6")
                    .to_string(),
                name,
                owner_id: m
                    .get("owner")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                owner_handle: m
                    .get("owner_handle")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                description: m
                    .get("description")
                    .or_else(|| project.get("description"))
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
                model: project
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
                owner: project
                    .get("owner")
                    .filter(|o| o.is_object())
                    .and_then(|o| serde_json::from_value::<Person>(o.clone()).ok()),
                reply_policy: match m.pointer("/settings/reply_policy").and_then(Value::as_str) {
                    Some("every_message") => ReplyPolicy::EveryMessage,
                    _ => ReplyPolicy::Mention,
                },
                avatar: project
                    .get("avatar")
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                    .map(str::to_string),
                avatar_color: project
                    .get("avatar_color")
                    .or_else(|| m.get("avatar_color"))
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                    .map(str::to_string),
                avatar_color_index: project
                    .get("avatar_color_index_effective")
                    .or_else(|| project.get("avatar_color_index"))
                    .or_else(|| m.get("avatar_color_index_effective"))
                    .or_else(|| m.get("avatar_color_index"))
                    .and_then(Value::as_u64)
                    .map(|i| (i % 7) as u8),
            });
        }
    }
    let kind = match v.get("kind").and_then(Value::as_str) {
        Some("group") => SpaceKind::Group,
        _ => SpaceKind::Dm,
    };
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| people.first().map(|p| p.handle.clone()))
        .unwrap_or_else(|| "chat".into());
    let pending = v.get("pending").and_then(Value::as_bool).unwrap_or(false)
        || people.iter().all(|p| p.status == "invited");
    let unread = v.get("unread").and_then(Value::as_bool).unwrap_or(false);
    let last = v
        .get("last_activity_at")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let icon = people
        .first()
        .map(|p| p.initial())
        .unwrap_or_else(|| "@".into());
    let space = Project {
        id: id.clone(),
        name: title.clone(),
        icon,
        color: "#8b7cf6".into(),
        state: BoxState::Awake,
        state_detail: None,
        wake_progress: None,
        model: String::new(),
        effort: Effort::Low,
        spend_today_usd: 0.0,
        spend_cap_usd: 0.0,
        kind,
        people,
        agents,
        pending,
        shared: false,
        owner_handle: None,
        avatar: None,
        avatar_color: None,
        description: None,
        owner_person: None,
        machine_info: None,
        muted: v
            .pointer("/me/muted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        primary_chat: None,
        voice: None,
        vision_enabled: true,
        avatar_color_index: None,
        avatar_color_override: None,
    };
    let previous_unread = shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.unread);
    let chat = Chat {
        id: id.clone(),
        project_id: id,
        title,
        is_pinned: true,
        is_system: false,
        kind: ChatKind::Chat,
        participants: Vec::new(),
        unread: previous_unread.unwrap_or(unread),
        last_message_at: last,
        archived_at: v
            .get("archived_at")
            .and_then(Value::as_i64)
            .filter(|t| *t > 0)
            .map(|t| t as u64),
    };
    Some((space, chat))
}

/// `GET /me/pushes`: people_* kinds that change the chat list refresh it.
async fn poll_me_pushes(shared: &Arc<Shared>) {
    let Ok(list) = shared.get_json("/me/pushes").await else {
        return;
    };
    let mut refresh = false;
    for push in list.as_array().cloned().unwrap_or_default() {
        let kind = push.get("kind").and_then(Value::as_str).unwrap_or("");
        let chat_id = push
            .pointer("/payload/chat_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        match kind {
            "people_chat_created"
            | "people_chat_updated"
            | "people_member_added"
            | "people_member_removed"
            | "people_agent_added"
            | "people_agent_removed"
            | "people_project_shared" => refresh = true,
            "people_message" if !shared.is_people(chat_id) => refresh = true,
            "call_started" => {
                let call = push.pointer("/payload/call").and_then(parse_call);
                let sender: Sender = push
                    .pointer("/payload/sender")
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                if let Some(call) = call {
                    let _ = shared.events.send(SyncEvent::CallStarted { call, sender });
                }
            }
            "chat_archived" => {
                let payload = push.get("payload").cloned().unwrap_or(Value::Null);
                let chat_id = payload
                    .get("chat_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if chat_id.is_empty() {
                    continue;
                }
                let archived_at = payload
                    .get("archived_at")
                    .and_then(Value::as_i64)
                    .filter(|t| *t > 0)
                    .map(|t| t as u64);
                if let Some(c) = shared
                    .chats
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|c| c.id == chat_id)
                {
                    c.archived_at = archived_at;
                }
                cache_archived(shared, &chat_id, archived_at.is_some());
                eprintln!(
                    "relay: chat_archived push {chat_id} archived={}",
                    archived_at.is_some()
                );
                let _ = shared.events.send(SyncEvent::ChatArchived {
                    chat_id,
                    archived_at,
                });
            }
            "feedback_capture" => {
                let request_id = push
                    .pointer("/payload/request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !request_id.is_empty() {
                    let _ = shared.events.send(SyncEvent::FeedbackCapture {
                        request_id,
                        expires_at: push.pointer("/payload/expires_at").and_then(Value::as_i64),
                    });
                }
            }
            "call_missed" => {
                if let Some(missed) = push
                    .get("payload")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<CallMissed>(v).ok())
                    .filter(|m| !m.call_id.is_empty())
                {
                    let _ = shared.events.send(SyncEvent::CallMissed(missed));
                }
            }
            "call_ended" => {
                if let Some(mut ended) = push
                    .get("payload")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<CallEnded>(v).ok())
                {
                    ended.writer_is_me =
                        ended.writer_device.as_deref() == Some(shared.device_id().as_str());
                    let _ = shared.events.send(SyncEvent::CallEnded(ended));
                }
            }
            _ => {}
        }
    }
    if refresh {
        if let Err(err) = refresh_registry(shared).await {
            shared.notice(format!("Relay: {err:#}"));
        }
    }
}

async fn fetch_avatar(shared: Arc<Shared>, url: String) {
    if url.is_empty() || !url.starts_with("http") {
        return;
    }
    if !shared.avatars_done.lock().unwrap().insert(url.clone()) {
        return;
    }
    let Ok(resp) = shared.http.get(&url).send().await else {
        return;
    };
    if !resp.status().is_success() {
        return;
    }
    if let Ok(bytes) = resp.bytes().await {
        let _ = shared.events.send(SyncEvent::Avatar {
            url,
            bytes: Arc::new(bytes.to_vec()),
        });
    }
}

/// `GET /voices` → `SyncEvent::Voices` (a relay without it just leaves the list empty).
async fn fetch_voices(shared: &Arc<Shared>) {
    match shared.get_json("/voices").await {
        Ok(v) => {
            let default = v
                .get("default")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
                .map(str::to_string);
            let list = v.get("voices").cloned().unwrap_or(v);
            let voices: Vec<Voice> = list
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| serde_json::from_value::<Voice>(x.clone()).ok())
                        .filter(|x| !x.id.is_empty())
                        .map(|mut x| {
                            // Relative sample paths live on the relay.
                            if x.sample_url.starts_with('/') {
                                x.sample_url = format!("{}{}", shared.http_url, x.sample_url);
                            } else if x.sample_url.is_empty() {
                                x.sample_url =
                                    format!("{}/voices/{}/sample.mp3", shared.http_url, x.id);
                            }
                            x
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = shared.events.send(SyncEvent::Voices { default, voices });
        }
        Err(err) => {
            if !err.to_string().contains("HTTP 404") {
                shared.notice(format!("Voices: {}", people_error(&err)));
            }
        }
    }
}

/// Keep the store's offline copy of my archived chat ids in step.
fn cache_archived(shared: &Arc<Shared>, chat_id: &str, archived: bool) {
    shared.with_store(|s| {
        let mut ids: Vec<Id> = s.get_json(store::KEY_ARCHIVED_CHATS).unwrap_or_default();
        ids.retain(|c| c != chat_id);
        if archived {
            ids.push(chat_id.to_string());
        }
        s.put_json(store::KEY_ARCHIVED_CHATS, &ids);
    });
}

/// Push a presence frame on the chat's room right away (focus / call changes).
fn presence_now(shared: &Arc<Shared>, chat_id: &str) {
    if let Some(room) = shared.rooms.lock().unwrap().get(chat_id) {
        let _ = room.cmds.send(RoomCmd::Presence);
    }
}

/// `{ call }` or a bare call object.
fn parse_call(v: &Value) -> Option<Call> {
    let obj = v.get("call").unwrap_or(v);
    serde_json::from_value::<Call>(obj.clone())
        .ok()
        .filter(|c| !c.call_id.is_empty())
}

fn call_error(err: &anyhow::Error) -> String {
    let text = people_error(err);
    if text.contains("calls_off") || text.contains("HTTP 503") {
        "Calls are not available on this relay yet.".into()
    } else if text.contains("call_ended") || text.contains("HTTP 410") {
        "That call has ended.".into()
    } else if text.contains("call_full") || text.contains("HTTP 409") {
        "The call is full (16 people).".into()
    } else if text.contains("HTTP 404") {
        "Calls are not available on this relay yet.".into()
    } else {
        text
    }
}

/// Calls v1 (contract §1): the client writes the system lines. One Loro row,
/// role `system`, text, `call_id`; the room syncs it like any message.
fn write_system_line(shared: &Arc<Shared>, chat_id: &str, text: String, call_id: Option<String>) {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(chat_id) else {
        return;
    };
    let msg = Message {
        id: new_id(),
        chat_id: chat_id.to_string(),
        role: Role::System,
        sender: "system".into(),
        sender_handle: None,
        sender_name: None,
        sender_avatar: None,
        sender_avatar_color: None,
        mentions: Vec::new(),
        status: MessageStatus::Done,
        turn: None,
        decision: None,
        subagent_id: None,
        subagent_name: None,
        handoff_messages: None,
        reactions: Default::default(),
        in_reply_to: None,
        routine_name: None,
        sender_icon: None,
        reply_to: None,
        asked_by_handle: None,
        owner_handle: None,
        usage: None,
        return_contract: None,
        waiting_user: false,
        waiting_approval: false,
        model: None,
        effort: None,
        created_at: now_ms(),
        completed_at: Some(now_ms()),
        call_id,
        activity: None,
        pending: false,
        parts: vec![Part::Text { id: new_id(), text }],
        hidden_parts: Vec::new(),
        greeting: false,
    };
    let _ = doc.insert_message(&msg);
    doc.commit();
}

/// D82: the agent's first line in a brand-new project's chat, written by the
/// device that created the project. Idempotent: a chat that already holds a
/// `greeting: true` row (ours or another device's) gets nothing.
fn write_greeting(shared: &Arc<Shared>, chat_id: &str, project_id: &str, agent_name: &str) -> bool {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(chat_id) else {
        return false;
    };
    if doc.read_all().iter().any(|m| m.greeting) {
        return true;
    }
    let msg = Message {
        id: new_id(),
        chat_id: chat_id.to_string(),
        role: Role::Assistant,
        sender: format!("agent:{project_id}"),
        sender_handle: None,
        sender_name: Some(agent_name.to_string()),
        sender_avatar: None,
        sender_avatar_color: None,
        mentions: Vec::new(),
        status: MessageStatus::Done,
        turn: None,
        decision: None,
        subagent_id: None,
        subagent_name: None,
        handoff_messages: None,
        in_reply_to: None,
        routine_name: None,
        sender_icon: None,
        reply_to: None,
        asked_by_handle: None,
        owner_handle: None,
        usage: None,
        return_contract: None,
        waiting_user: false,
        waiting_approval: false,
        model: None,
        effort: None,
        created_at: now_ms(),
        completed_at: Some(now_ms()),
        call_id: None,
        activity: None,
        pending: false,
        reactions: Default::default(),
        parts: vec![Part::Text {
            id: new_id(),
            text: format!(
                "Hi, I’m {agent_name}. Your computer is ready — what should we build first?"
            ),
        }],
        hidden_parts: Vec::new(),
        greeting: true,
    };
    let _ = doc.insert_message(&msg);
    doc.commit();
    true
}

fn people_error(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    serde_json::from_str::<Value>(
        text.split_once(" HTTP ")
            .map(|(_, r)| r)
            .and_then(|r| r.split_once(' '))
            .map(|(_, b)| b)
            .unwrap_or(""),
    )
    .ok()
    .and_then(|v| {
        v.pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
    .unwrap_or(text)
}

fn parse_chat(v: &Value) -> Option<Chat> {
    Some(Chat {
        id: v.get("id")?.as_str()?.to_string(),
        project_id: v.get("project_id")?.as_str()?.to_string(),
        title: v
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("New chat")
            .to_string(),
        is_pinned: v.get("is_pinned").and_then(Value::as_bool).unwrap_or(false),
        is_system: v.get("is_system").and_then(Value::as_bool).unwrap_or(false),
        kind: v
            .get("kind")
            .cloned()
            .and_then(|k| serde_json::from_value(k).ok())
            .unwrap_or_default(),
        participants: v
            .get("participants")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        unread: false,
        last_message_at: v.get("created_at").and_then(Value::as_i64).unwrap_or(0) as u64,
        archived_at: v
            .get("archived_at")
            .and_then(Value::as_i64)
            .filter(|t| *t > 0)
            .map(|t| t as u64),
    })
}

/// `GET /projects` then `GET /projects/{id}` for chats, models and spend.
async fn refresh_registry(shared: &Arc<Shared>) -> Result<()> {
    shared.ensure_me().await;
    let list = shared.get_json("/projects").await?;
    let items = list.as_array().cloned().unwrap_or_default();
    let mut projects: Vec<Project> = Vec::new();
    let mut all_chats: Vec<Chat> = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let detail = shared.get_json(&format!("/projects/{id}")).await?;
        // Who else is in this project (D72 step 3); older relays answer 404.
        let members = shared
            .get_json(&format!("/projects/{id}/members"))
            .await
            .ok()
            .and_then(|v| v.get("members").cloned())
            .unwrap_or(Value::Null);
        let p = detail.get("project").cloned().unwrap_or(item.clone());
        let models = p.get("models").cloned().unwrap_or(Value::Null);
        let talking = models.get("talking_model").cloned().unwrap_or(Value::Null);
        let state = box_state_from(
            p.get("state").and_then(Value::as_str).unwrap_or(""),
            detail
                .get("box")
                .or(item.get("box"))
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let previous = shared
            .projects
            .lock()
            .unwrap()
            .iter()
            .find(|x| x.id == id)
            .cloned();
        projects.push(Project {
            id: id.to_string(),
            name: p
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Project")
                .to_string(),
            icon: p
                .get("icon")
                .and_then(Value::as_str)
                .unwrap_or("Ω")
                .to_string(),
            color: "#8b7cf6".into(),
            state,
            state_detail: previous.as_ref().and_then(|x| x.state_detail.clone()),
            wake_progress: None,
            model: talking
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| previous.as_ref().map(|x| x.model.clone()))
                .unwrap_or_else(|| "anthropic/claude-haiku-5.5".into()),
            effort: effort_from(talking.get("effort").unwrap_or(&Value::Null)),
            spend_today_usd: detail
                .get("spend_today")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            spend_cap_usd: p
                .get("spend_cap_daily")
                .and_then(Value::as_f64)
                .unwrap_or(25.0),
            kind: SpaceKind::Project,
            agents: Vec::new(),
            pending: false,
            people: members
                .as_array()
                .or_else(|| item.get("members").and_then(Value::as_array))
                .map(|a| {
                    a.iter()
                        .filter_map(|m| {
                            let id = m.get("user_id").and_then(Value::as_str)?;
                            Some(Person {
                                id: id.to_string(),
                                handle: m
                                    .get("handle")
                                    .and_then(Value::as_str)
                                    .unwrap_or(id)
                                    .to_string(),
                                name: m.get("name").and_then(Value::as_str).map(str::to_string),
                                avatar_url: m
                                    .get("avatar_url")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                                status: "member".into(),
                                last_seen_at: m.get("last_seen_at").and_then(Value::as_i64),
                                online: m.get("online").and_then(Value::as_bool).unwrap_or(false),
                                avatar_color_index: m
                                    .get("avatar_color_index_effective")
                                    .or_else(|| m.get("avatar_color_index"))
                                    .and_then(Value::as_u64)
                                    .map(|i| (i % 7) as u8),
                                role: m
                                    .get("role")
                                    .and_then(Value::as_str)
                                    .unwrap_or("member")
                                    .to_string(),
                            })
                        })
                        .filter(|p| p.id != shared.me_id())
                        .collect()
                })
                .unwrap_or_default(),
            shared: item.get("shared").and_then(Value::as_bool).unwrap_or(false),
            owner_handle: item
                .get("owner_handle")
                .and_then(Value::as_str)
                .map(str::to_string),
            avatar: p
                .get("avatar")
                .or_else(|| item.get("avatar"))
                .and_then(Value::as_str)
                .filter(|a| !a.is_empty())
                .map(str::to_string)
                .or_else(|| previous.as_ref().and_then(|x| x.avatar.clone())),
            avatar_color: p
                .get("avatar_color")
                .or_else(|| item.get("avatar_color"))
                .and_then(Value::as_str)
                .filter(|a| !a.is_empty())
                .map(str::to_string)
                .or_else(|| previous.as_ref().and_then(|x| x.avatar_color.clone())),
            description: p
                .get("description")
                .or_else(|| item.get("description"))
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
                .map(str::to_string),
            owner_person: p
                .get("owner")
                .or_else(|| item.get("owner"))
                .filter(|o| o.is_object())
                .and_then(|o| serde_json::from_value::<Person>(o.clone()).ok()),
            machine_info: p
                .get("machine")
                .or_else(|| item.get("machine"))
                .filter(|m| m.is_object())
                .and_then(|m| {
                    serde_json::from_value::<crate::model::ProjectMachine>(m.clone()).ok()
                }),
            muted: previous.as_ref().is_some_and(|x| x.muted),
            // §9b: the relay names the agent's one chat (stored, never flips).
            primary_chat: [&p, &detail, &item]
                .iter()
                .find_map(|v| {
                    v.get("primary_chat_id")
                        .or_else(|| v.get("primary_chat"))
                        .and_then(Value::as_str)
                        .filter(|c| !c.is_empty())
                })
                .map(str::to_string),
            voice: p
                .get("voice")
                .or_else(|| item.get("voice"))
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            vision_enabled: [&p, &item]
                .iter()
                .find_map(|v| v.get("vision_enabled").and_then(Value::as_bool))
                .unwrap_or(true),
            // D79: the relay's effective index when it sends one, else the
            // owner's pick, else derived from the id (the same rule everywhere).
            avatar_color_index: [&p, &item]
                .iter()
                .find_map(|v| {
                    v.get("avatar_color_index_effective")
                        .or_else(|| v.get("avatar_color_index"))
                        .and_then(Value::as_u64)
                })
                .map(|i| (i % 7) as u8)
                .or_else(|| Some(crate::ui::avatars::color_index(id))),
            avatar_color_override: [&p, &item]
                .iter()
                .find_map(|v| v.get("avatar_color_index").and_then(Value::as_u64))
                .map(|i| (i % 7) as u8),
        });
        let chats: Vec<Chat> = detail
            .get("chats")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(parse_chat).collect())
            .unwrap_or_default();
        all_chats.extend(chats);
    }

    // People chats (D72) share the rail: one space + one pinned chat each.
    let people_ids: Vec<Id> = projects
        .iter()
        .filter(|p| p.is_people())
        .map(|p| p.id.clone())
        .collect();
    match shared.get_json("/chats").await {
        Ok(list) => {
            let mut ids = shared.people.lock().unwrap();
            for item in list.as_array().cloned().unwrap_or_default() {
                let Some((space, chat)) = people_space_from(shared, &item) else {
                    continue;
                };
                ids.insert(space.id.clone());
                projects.push(space);
                all_chats.push(chat);
            }
        }
        Err(err) => {
            // Older relays have no /chats; keep whatever we already knew.
            if !format!("{err:#}").contains("HTTP 404") {
                shared.notice(format!("People chats: {err:#}"));
            }
            let known = shared.people.lock().unwrap().clone();
            let previous = shared.projects.lock().unwrap().clone();
            for p in previous.into_iter().filter(|p| known.contains(&p.id)) {
                projects.push(p);
            }
            let prev_chats = shared.chats.lock().unwrap().clone();
            for c in prev_chats
                .into_iter()
                .filter(|c| known.contains(&c.project_id))
            {
                all_chats.push(c);
            }
        }
    }
    let _ = people_ids;
    migrate_local_archive(shared, &mut all_chats).await;
    // The relay's archive state is the truth; the store keeps an offline copy.
    shared.with_store(|s| {
        let ids: Vec<Id> = all_chats
            .iter()
            .filter(|c| c.archived_at.is_some())
            .map(|c| c.id.clone())
            .collect();
        s.put_json(store::KEY_ARCHIVED_CHATS, &ids);
    });
    // Keep local-only tabs (minted, not yet created) and unread flags.
    {
        let mut local = shared.chats.lock().unwrap();
        let mut merged: Vec<Chat> = Vec::new();
        for c in &all_chats {
            let mut c = c.clone();
            if let Some(prev) = local.iter().find(|x| x.id == c.id) {
                c.unread = prev.unread;
                c.last_message_at = c.last_message_at.max(prev.last_message_at);
                if c.title == "New chat" && prev.title != "New chat" {
                    c.title = prev.title.clone();
                }
            }
            merged.push(c);
        }
        *local = merged;
    }
    *shared.projects.lock().unwrap() = projects.clone();
    let _ = shared.events.send(SyncEvent::Projects(projects.clone()));
    log_first_paint("relay", projects.len());
    emit_chats(shared);
    save_registry(shared);
    for p in projects.iter().filter(|p| !p.is_people()) {
        ensure_room(shared, &p.id, &project_doc_key(&p.id), PROJECT_DOC_CHAT_ID);
    }
    for c in &all_chats {
        ensure_room(shared, &c.project_id, &c.id, &c.id);
    }
    Ok(())
}

/// §9c, once after the update: chats archived on this Mac before archive state
/// lived on the relay are pushed up as a union (one PUT each), then the local
/// list is only a cache of the relay's answer.
async fn migrate_local_archive(shared: &Arc<Shared>, all_chats: &mut [Chat]) {
    let Some((done, local)) = shared.with_store(|s| {
        (
            s.get_json::<bool>(store::KEY_ARCHIVE_MIGRATED)
                .unwrap_or(false),
            s.get_json::<Vec<Id>>(store::KEY_ARCHIVED_CHATS)
                .unwrap_or_default(),
        )
    }) else {
        return;
    };
    if done {
        return;
    }
    let mut all_ok = true;
    for chat in all_chats
        .iter_mut()
        .filter(|c| c.archived_at.is_none() && local.contains(&c.id))
    {
        match set_archived(shared, &chat.project_id, &chat.id, true).await {
            Ok(at) => chat.archived_at = Some(at.unwrap_or_else(now_ms)),
            Err(err) => {
                all_ok = false;
                // A relay without §9c yet answers 404: try again on a later refresh, quietly.
                if err.to_string().contains("HTTP 404") {
                    break;
                }
                shared.notice(format!("Archive sync: {}", people_error(&err)));
            }
        }
    }
    if all_ok {
        shared.with_store(|s| s.put_json(store::KEY_ARCHIVE_MIGRATED, &true));
    }
}

/// `PUT /chats/{id}/settings {archived}` (§9c). Answers the stored
/// `archived_at` when the relay echoes the chat; `Ok(None)` when it does not.
async fn set_archived(
    shared: &Arc<Shared>,
    project_id: &str,
    chat_id: &str,
    archived: bool,
) -> Result<Option<u64>> {
    let body = json!({ "archived": archived });
    let v = match shared
        .put_json(&format!("/chats/{chat_id}/settings"), body.clone())
        .await
    {
        Ok(v) => v,
        // Older relays route project chats under the project.
        Err(err) if err.to_string().contains("HTTP 404") && !shared.is_people(chat_id) => {
            shared
                .put_json(
                    &format!("/projects/{project_id}/chats/{chat_id}/settings"),
                    body,
                )
                .await?
        }
        Err(err) => return Err(err),
    };
    Ok(v.get("archived_at")
        .or_else(|| v.pointer("/chat/archived_at"))
        .and_then(Value::as_i64)
        .filter(|t| *t > 0)
        .map(|t| t as u64))
}

fn emit_chats(shared: &Shared) {
    let chats = shared.chats.lock().unwrap().clone();
    let projects: Vec<Id> = shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .map(|p| p.id.clone())
        .collect();
    for project_id in projects {
        let mut mine: Vec<Chat> = chats
            .iter()
            .filter(|c| c.project_id == project_id)
            .cloned()
            .collect();
        mine.sort_by_key(|c| (!c.is_pinned, c.last_message_at));
        let _ = shared.events.send(SyncEvent::Chats {
            project_id,
            chats: mine,
        });
    }
}

// ---------------------------------------------------------------------------
// Rooms (WebSocket)
// ---------------------------------------------------------------------------

fn apply_pending(shared: &Shared, message: &mut Message) {
    if message.role == Role::User {
        message.pending = shared.unacked.lock().unwrap().contains(&message.id);
    }
}

fn emit_snapshot(shared: &Shared, chat_id: &str) {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(chat_id) else { return };
    let mut messages = doc.read_all();
    for m in &mut messages {
        apply_pending(shared, m);
    }
    let _ = shared.events.send(SyncEvent::ChatSnapshot {
        chat_id: chat_id.to_string(),
        messages,
    });
}

fn emit_project_doc(shared: &Shared, project_id: &str) {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(&project_doc_key(project_id)) else {
        return;
    };
    let to_json = |v: loro::LoroValue| serde_json::to_value(v).unwrap_or(Value::Null);
    let threads_v = to_json(doc.doc.get_map("threads").get_deep_value());
    let mut threads: Vec<ThreadInfo> = threads_v
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(id, v)| {
                    let mut t: ThreadInfo = serde_json::from_value(v.clone()).ok()?;
                    t.id = id.clone();
                    Some(t)
                })
                .collect()
        })
        .unwrap_or_default();
    threads.sort_by_key(|t| std::cmp::Reverse(t.last_turn_at.unwrap_or(0)));
    let memory = to_json(doc.doc.get_map("memory").get_deep_value());
    let view = ProjectDocView {
        threads,
        status: to_json(doc.doc.get_map("status").get_deep_value()),
        settings: to_json(doc.doc.get_map("settings").get_deep_value()),
        memory_t1: memory
            .get("t1")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        model_note: view_settings_note(&doc.doc),
    };
    let _ = shared.events.send(SyncEvent::ProjectDoc {
        project_id: project_id.to_string(),
        view,
    });
}

fn view_settings_note(doc: &loro::LoroDoc) -> String {
    match doc.get_map("settings").get("modelNote") {
        Some(loro::ValueOrContainer::Value(loro::LoroValue::String(s))) => s.to_string(),
        Some(_) | None => String::new(),
    }
}

/// Open a room once; `key` is the docs map key, `room_chat` the URL segment.
fn ensure_room(shared: &Arc<Shared>, project_id: &str, key: &str, room_chat: &str) {
    ensure_room_with(shared, project_id, key, room_chat, None);
}

/// A doc restored from the on-device store with the relay cursor it was saved
/// at and the pushes the relay never ACKed.
struct Restored {
    doc: ChatDoc,
    cursor: i64,
    pending: Vec<(String, Vec<u8>)>,
}

fn ensure_room_with(
    shared: &Arc<Shared>,
    project_id: &str,
    key: &str,
    room_chat: &str,
    restored: Option<Restored>,
) {
    if shared.rooms.lock().unwrap().contains_key(key) {
        return;
    }
    let from_store = restored.is_some();
    let (doc, cursor, pending) = match restored {
        Some(r) => (r.doc, r.cursor, r.pending),
        None => (ChatDoc::empty(key), 0, Vec::new()),
    };
    let (local_tx, local_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (change_tx, change_rx) = mpsc::unbounded_channel::<super::doc::Touched>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<RoomCmd>();
    let is_project_doc = room_chat == PROJECT_DOC_CHAT_ID;

    let sub = doc.doc.subscribe_root(Arc::new(move |event| {
        let touched = ChatDoc::touched(&event);
        if is_project_doc || touched.list_changed || !touched.messages.is_empty() {
            let _ = change_tx.send(touched);
        }
    }));
    let local_sub = doc
        .doc
        .subscribe_local_update(Box::new(move |bytes| local_tx.send(bytes.clone()).is_ok()));
    {
        let mut subs = shared.subscriptions.lock().unwrap();
        subs.push(sub);
        subs.push(local_sub);
    }
    shared.docs.lock().unwrap().insert(key.to_string(), doc);
    if from_store {
        // Paint from disk now; the room's backfill only adds what is new.
        shared
            .room_seq
            .lock()
            .unwrap()
            .insert(key.to_string(), cursor);
        if is_project_doc {
            emit_project_doc(shared, project_id);
            apply_project_doc_status(shared, project_id);
        } else {
            emit_snapshot(shared, key);
        }
    }

    let shared2 = shared.clone();
    let key2 = key.to_string();
    let project2 = project_id.to_string();
    tokio::spawn(async move {
        forward_changes(shared2, project2, key2, is_project_doc, change_rx).await;
    });

    let shared3 = shared.clone();
    let project = project_id.to_string();
    let key3 = key.to_string();
    let room_chat = room_chat.to_string();
    let task = tokio::spawn(async move {
        room_loop(
            shared3, project, key3, room_chat, cursor, pending, local_rx, cmd_rx,
        )
        .await;
    });
    shared
        .rooms
        .lock()
        .unwrap()
        .insert(key.to_string(), Room { cmds: cmd_tx, task });
}

async fn forward_changes(
    shared: Arc<Shared>,
    project_id: Id,
    key: Id,
    is_project_doc: bool,
    mut rx: mpsc::UnboundedReceiver<super::doc::Touched>,
) {
    while let Some(touched) = rx.recv().await {
        shared.mark_dirty(&key);
        if is_project_doc {
            emit_project_doc(&shared, &project_id);
            apply_project_doc_status(&shared, &project_id);
            continue;
        }
        if touched.list_changed {
            emit_snapshot(&shared, &key);
            mark_activity(&shared, &key);
            continue;
        }
        let docs = shared.docs.lock().unwrap();
        let Some(doc) = docs.get(&key) else { continue };
        for ix in touched.messages {
            if let Some(mut message) = doc.read_message(ix) {
                apply_pending(&shared, &mut message);
                let _ = shared.events.send(SyncEvent::MessageUpsert {
                    chat_id: key.clone(),
                    message: Box::new(message),
                });
            }
        }
    }
}

/// `@slug` / `@handle` in the text → `agent:<project>` / `user:<id>` (API §3).
fn mentions_for(shared: &Shared, chat_id: &str, text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let projects = shared.projects.lock().unwrap();
    let Some(space) = projects.iter().find(|p| p.id == chat_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in &space.agents {
        if lower.contains(&format!("@{}", a.handle.to_lowercase())) {
            out.push(format!("agent:{}", a.project_id));
        }
    }
    for p in &space.people {
        if lower.contains(&format!("@{}", p.handle.to_lowercase())) {
            out.push(format!("user:{}", p.id));
        }
    }
    out
}

fn mark_activity(shared: &Shared, chat_id: &str) {
    let mut chats = shared.chats.lock().unwrap();
    if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
        c.last_message_at = now_ms();
    }
}

fn box_state_from_word(word: &str) -> Option<BoxState> {
    Some(match word {
        "awake" => BoxState::Awake,
        "waking" => BoxState::Waking,
        "error" => BoxState::Error,
        "running" => BoxState::Awake,
        "starting" => BoxState::Waking,
        "asleep" | "stopped" | "hibernated_light" => BoxState::HibernatedLight,
        "hibernated_deep" => BoxState::HibernatedDeep,
        "stopped_by_user" => BoxState::StoppedByUser,
        _ => return None,
    })
}

fn machine_state_from_word(word: &str) -> MachineState {
    match word {
        "awake" => MachineState::Awake,
        "waking" | "starting" | "creating" => MachineState::Waking,
        "asleep" | "hibernated_light" | "hibernated_deep" => MachineState::Asleep,
        "stopped" | "stopped_by_user" => MachineState::Stopped,
        "error" => MachineState::Error,
        _ => MachineState::Unknown,
    }
}

/// Project doc `status` (D70): `box {state: awake|asleep, since}` is the
/// user's machine as the box itself knows it, mirrored into every project
/// doc — the instant signal between `GET /machine` polls; `workspace {state:
/// running|stopped, since}` is this project. Older docs carry `box` as a bare
/// word — read that as both.
fn apply_project_doc_status(shared: &Shared, project_id: &str) {
    let status = {
        let docs = shared.docs.lock().unwrap();
        let Some(doc) = docs.get(&project_doc_key(project_id)) else {
            return;
        };
        serde_json::to_value(doc.doc.get_map("status").get_deep_value()).unwrap_or(Value::Null)
    };
    match status.get("box") {
        Some(Value::Object(b)) => {
            let word = b.get("state").and_then(Value::as_str).unwrap_or("");
            let since = b.get("since").and_then(Value::as_u64).filter(|v| *v > 0);
            let state = machine_state_from_word(word);
            // The box can only say awake/asleep; never let a stale "asleep"
            // mirror override the relay's live "waking".
            let waking = shared.machine.lock().unwrap().state == MachineState::Waking;
            if !(waking && state == MachineState::Asleep) {
                set_machine(shared, state, since, None);
            }
        }
        Some(Value::String(word)) => {
            set_machine(shared, machine_state_from_word(word), None, None);
            if let Some(state) = box_state_from_word(word) {
                set_project_state(shared, project_id, state, None);
            }
        }
        Some(_) | None => {}
    }
    if let Some(ws) = status.get("workspace").and_then(Value::as_object) {
        if let Some(word) = ws.get("state").and_then(Value::as_str) {
            let _ = shared.events.send(SyncEvent::Workspace {
                project_id: project_id.to_string(),
                running: word == "running",
            });
            if let Some(state) = box_state_from_word(word) {
                set_project_state(shared, project_id, state, None);
            }
        }
    }
}

/// `GET /machine`: state, spec, spend and every workspace's state.
async fn refresh_machine(shared: &Arc<Shared>) {
    let value = match shared.get_json("/machine").await {
        Ok(v) => v,
        // Quiet: the registry poll already surfaces relay outages (S14).
        Err(_) => return,
    };
    let word = value.get("state").and_then(Value::as_str).unwrap_or("");
    let since = value
        .get("since")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0);
    let detail = value
        .get("error")
        .or_else(|| value.get("detail"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let spec_obj = value.get("spec").and_then(Value::as_object);
    let spec = spec_obj.map(|spec| {
        let num = |k: &str| spec.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        format!(
            "{} vCPU · {} GiB · {} GiB",
            num("cpu"),
            num("mem_gib"),
            num("disk_gib")
        )
    });
    let class = spec_obj
        .and_then(|spec| spec.get("class"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let snapshot = {
        let mut m = shared.machine.lock().unwrap();
        let state = machine_state_from_word(word);
        if m.state != state {
            m.since = since.or(Some(now_ms()));
        } else if since.is_some() {
            m.since = since;
        }
        m.state = state;
        m.detail = detail;
        m.progress = None;
        if spec.is_some() {
            m.spec = spec;
        }
        if class.is_some() {
            m.class = class;
        }
        if let Some(spec) = spec_obj {
            m.cpu = spec.get("cpu").and_then(Value::as_u64).map(|v| v as u32);
            m.mem_gib = spec
                .get("mem_gib")
                .and_then(Value::as_u64)
                .map(|v| v as u32);
        }
        if let Some(img) = value
            .pointer("/image/current")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            m.image = Some(img.to_string());
        }
        if let Some(v) = value.get("spend_today_usd").and_then(Value::as_f64) {
            m.spend_today_usd = v;
        }
        if let Some(v) = value.get("spend_cap_usd").and_then(Value::as_f64) {
            m.spend_cap_usd = v;
        }
        if let Some(v) = value.get("always_on").and_then(Value::as_bool) {
            m.always_on = v;
        }
        m.clone()
    };
    let _ = shared.events.send(SyncEvent::Machine(snapshot));
    if let Some(workspaces) = value.get("workspaces").and_then(Value::as_array) {
        for ws in workspaces {
            let (Some(id), Some(word)) = (
                ws.get("project_id").and_then(Value::as_str),
                ws.get("state").and_then(Value::as_str),
            ) else {
                continue;
            };
            let _ = shared.events.send(SyncEvent::Workspace {
                project_id: id.to_string(),
                running: word == "running",
            });
            if let Some(state) = box_state_from_word(word) {
                set_project_state(shared, id, state, None);
            }
        }
    }
}

fn set_machine(shared: &Shared, state: MachineState, since: Option<u64>, detail: Option<String>) {
    let snapshot = {
        let mut m = shared.machine.lock().unwrap();
        let changed = m.state != state || m.detail != detail;
        if m.state != state {
            m.since = since.or(Some(now_ms()));
        } else if since.is_some() {
            m.since = since;
        }
        m.state = state;
        m.detail = detail;
        m.progress = None;
        if !changed {
            return;
        }
        m.clone()
    };
    let _ = shared.events.send(SyncEvent::Machine(snapshot));
}

fn set_project_state(shared: &Shared, project_id: &str, state: BoxState, detail: Option<String>) {
    {
        let mut projects = shared.projects.lock().unwrap();
        if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
            if p.state == state && p.state_detail == detail {
                return;
            }
            p.state = state;
            p.state_detail = detail.clone();
            p.wake_progress = None;
        }
    }
    let _ = shared.events.send(SyncEvent::ProjectStatus {
        project_id: project_id.to_string(),
        state,
        detail,
        progress: None,
    });
}

/// Dial, handshake, backfill, then stream — forever, with backoff (§8.6).
#[allow(clippy::too_many_arguments)]
async fn room_loop(
    shared: Arc<Shared>,
    project_id: Id,
    key: Id,
    room_chat: String,
    saved_cursor: i64,
    saved_pending: Vec<(String, Vec<u8>)>,
    mut local_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut cmd_rx: mpsc::UnboundedReceiver<RoomCmd>,
) {
    // Restored from the on-device store: HELLO with this cursor so the relay
    // only sends rows after it; a fresh room starts at 0.
    let mut cursor: i64 = saved_cursor;
    let mut backoff = BACKOFF_MIN;
    let mut pending: Vec<(String, Vec<u8>)> = saved_pending;
    loop {
        let connected_at = Instant::now();
        let outcome = run_session(
            &shared,
            &project_id,
            &key,
            &room_chat,
            &mut cursor,
            &mut pending,
            &mut local_rx,
            &mut cmd_rx,
        )
        .await;
        if let Err(err) = outcome {
            eprintln!("relay room {project_id}/{room_chat}: {err:#}");
        }
        let _ = shared.events.send(SyncEvent::Link(Link::Reconnecting));
        if connected_at.elapsed() > BACKOFF_RESET_AFTER {
            backoff = BACKOFF_MIN;
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    shared: &Arc<Shared>,
    project_id: &str,
    key: &str,
    room_chat: &str,
    cursor: &mut i64,
    pending: &mut Vec<(String, Vec<u8>)>,
    local_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    cmd_rx: &mut mpsc::UnboundedReceiver<RoomCmd>,
) -> Result<()> {
    let is_people = shared.people.lock().unwrap().contains(room_chat);
    let room = if is_people {
        format!("people/{room_chat}")
    } else {
        wire::room_name(project_id, room_chat)
    };
    let url = format!("{}/room/{}?token={}", shared.ws_url, room, shared.token());
    let (ws, _) = timeout(HELLO_DEADLINE, connect_async(&url))
        .await
        .context("dial timed out")??;
    let (mut tx, mut rx) = ws.split();

    let hello = Hello {
        cursor: *cursor,
        device: shared.device_id(),
        room: room.clone(),
        proto: PROTO_VERSION,
    };
    tx.send(binary(Frame::new(
        FrameType::Hello,
        serde_json::to_value(&hello)?,
        Vec::new(),
    )))
    .await?;

    let state = timeout(HELLO_DEADLINE, async {
        while let Some(msg) = rx.next().await {
            if let WsMessage::Binary(bytes) = msg? {
                if let Some(frame) = Frame::decode(bytes)? {
                    if frame.ty == FrameType::State {
                        let header: WireState = serde_json::from_value(frame.header)?;
                        return Ok::<_, anyhow::Error>(Some((header, frame.payload)));
                    }
                }
            }
        }
        Ok(None)
    })
    .await
    .context("STATE did not arrive")??
    .context("socket closed before STATE")?;
    let (state, frontier) = state;
    set_project_state(shared, project_id, map_box(state.box_state), None);
    set_machine(shared, machine_from_wire(state.box_state), None, None);

    // Checkpoint-on-connect (§7.3): fetch when our replica does not contain
    // the checkpoint frontier, or when our cursor is below it.
    let contained = {
        let docs = shared.docs.lock().unwrap();
        match (docs.get(key), arbos_core::doc::decode_vv(&frontier)) {
            (Some(doc), Some(vv)) => doc.doc.oplog_vv().includes_vv(&vv),
            (Some(_), None) | (None, _) => false,
        }
    };
    // A replica restored from the on-device store usually contains the
    // frontier already: the rows the relay compacted away are ours, so only
    // rows after `max(cursor, checkpointSeq)` are needed (§7.3).
    if state.checkpoint_seq > 0 {
        if contained {
            *cursor = (*cursor).max(state.checkpoint_seq);
        } else {
            match fetch_checkpoint(shared, &room).await {
                Ok((seq, bytes)) => {
                    let docs = shared.docs.lock().unwrap();
                    if let Some(doc) = docs.get(key) {
                        match doc.doc.import(&bytes) {
                            Ok(_) => *cursor = (*cursor).max(seq.max(state.checkpoint_seq)),
                            Err(err) => {
                                eprintln!("relay room {room}: checkpoint import failed: {err}")
                            }
                        }
                    }
                }
                Err(err) => eprintln!("relay room {room}: checkpoint fetch failed: {err:#}"),
            }
        }
        shared
            .room_seq
            .lock()
            .unwrap()
            .insert(key.to_string(), *cursor);
        shared.mark_dirty(key);
    }
    // Own rows are requested too: a row pushed by this device but saved before
    // its ACK is only on the relay (the relay's `excludeOwn` filters by device,
    // not by connection). Loro drops the duplicates.
    tx.send(binary(Frame::new(
        FrameType::RowsReq,
        serde_json::to_value(RowsReq {
            after: *cursor,
            exclude_own: false,
        })?,
        Vec::new(),
    )))
    .await?;

    for (batch_id, bytes) in pending.iter() {
        tx.send(binary(Frame::new(
            FrameType::Push,
            json!({ "batchId": batch_id }),
            bytes.clone(),
        )))
        .await?;
    }

    let mut presence = interval(PRESENCE_EVERY);
    loop {
        tokio::select! {
            msg = rx.next() => {
                let Some(msg) = msg else { anyhow::bail!("socket closed") };
                let WsMessage::Binary(bytes) = msg? else { continue };
                let Some(frame) = Frame::decode(bytes)? else { continue };
                match frame.ty {
                    FrameType::Row => {
                        let header: RowHeader = serde_json::from_value(frame.header)?;
                        let docs = shared.docs.lock().unwrap();
                        if let Some(doc) = docs.get(key) {
                            if let Err(err) = doc.doc.import(&frame.payload) {
                                eprintln!("relay room {room}: import seq {} failed: {err}", header.seq);
                            }
                        }
                        *cursor = (*cursor).max(header.seq);
                        shared.room_seq.lock().unwrap().insert(key.to_string(), *cursor);
                    }
                    FrameType::RowsDone => {
                        let head_seq = frame.header.get("headSeq").and_then(Value::as_i64).unwrap_or(*cursor);
                        *cursor = (*cursor).max(head_seq);
                        shared.room_seq.lock().unwrap().insert(key.to_string(), *cursor);
                        shared.mark_dirty(key);
                        let _ = shared.events.send(SyncEvent::Link(Link::Live));
                        if room_chat == PROJECT_DOC_CHAT_ID {
                            emit_project_doc(shared, project_id);
                        } else {
                            emit_snapshot(shared, key);
                            let _ = shared.events.send(SyncEvent::ChatSynced(key.to_string()));
                        }
                    }
                    FrameType::Ack => {
                        let header: Ack = serde_json::from_value(frame.header)?;
                        let before = pending.len();
                        pending.retain(|(id, _)| *id != header.batch_id);
                        if pending.len() != before {
                            shared.with_store(|s| s.save_pending(key, pending));
                        }
                        *cursor = (*cursor).max(header.seq);
                        shared.room_seq.lock().unwrap().insert(key.to_string(), *cursor);
                        shared.mark_dirty(key);
                        let had: Vec<Id> = shared.unacked.lock().unwrap().drain().collect();
                        if !had.is_empty() && room_chat != PROJECT_DOC_CHAT_ID {
                            emit_snapshot(shared, key);
                        }
                    }
                    FrameType::Status => {
                        let header: StatusHeader = serde_json::from_value(frame.header)?;
                        set_project_state(shared, project_id, map_box(header.box_state), header.detail.clone());
                        set_machine(shared, machine_from_wire(header.box_state), None, header.detail);
                        let poll = shared.clone();
                        tokio::spawn(async move { refresh_machine(&poll).await });
                    }
                    FrameType::Error => {
                        let header: ErrorHeader = serde_json::from_value(frame.header)?;
                        if header.code == "lagging" {
                            tx.send(binary(Frame::new(
                                FrameType::RowsReq,
                                serde_json::to_value(RowsReq { after: *cursor, exclude_own: true })?,
                                Vec::new(),
                            ))).await?;
                        } else {
                            anyhow::bail!("relay error {}: {}", header.code, header.message);
                        }
                    }
                    FrameType::Presence => {
                        let h = &frame.header;
                        let mine = h.get("device").and_then(Value::as_str) == Some(shared.device_id().as_str());
                        // Calls v1: another device of the chat says it is in a call →
                        // fetch it so the "On a call · Join" bar can show (§8 presence).
                        if let Some(call_id) = h.get("in_call").and_then(Value::as_str) {
                            if !mine && shared.in_call.lock().unwrap().as_deref() != Some(call_id) {
                                let _ = shared.events.send(SyncEvent::CallSeen {
                                    chat_id: room_chat.to_string(),
                                    call_id: call_id.to_string(),
                                });
                            }
                        }
                        let handle = h.get("handle").and_then(Value::as_str).filter(|s| !s.is_empty());
                        if let (false, Some(handle)) = (mine, handle) {
                            let _ = shared.events.send(SyncEvent::Presence { handle: handle.to_string() });
                            if is_people {
                                if let Some(typing) = h.get("typing").and_then(Value::as_bool) {
                                    let _ = shared.events.send(SyncEvent::Typing {
                                        chat_id: room_chat.to_string(),
                                        handle: handle.to_string(),
                                        active: typing,
                                    });
                                }
                            }
                        }
                    }
                    FrameType::ProbeOk | FrameType::State => {}
                    FrameType::Hello | FrameType::RowsReq | FrameType::Push | FrameType::Probe | FrameType::Wake => {}
                }
            }
            bytes = local_rx.recv() => {
                let Some(bytes) = bytes else { anyhow::bail!("doc dropped") };
                if bytes.len() > wire::ROW_MAX {
                    eprintln!("relay room {room}: local update {} bytes exceeds row cap; dropped", bytes.len());
                    continue;
                }
                let batch_id = new_id();
                pending.push((batch_id.clone(), bytes.clone()));
                shared.with_store(|s| s.save_pending(key, pending));
                tx.send(binary(Frame::new(
                    FrameType::Push,
                    json!({ "batchId": batch_id }),
                    bytes,
                ))).await?;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { anyhow::bail!("room closed") };
                match cmd {
                    RoomCmd::Wake(reason) => {
                        tx.send(binary(Frame::new(
                            FrameType::Wake,
                            serde_json::to_value(WakeHeader { reason: reason.to_string() })?,
                            Vec::new(),
                        ))).await?;
                    }
                    RoomCmd::Typing(active) => {
                        let mut header = shared.presence_header();
                        header["typing"] = json!(active);
                        tx.send(binary(Frame::new(FrameType::Presence, header, Vec::new()))).await?;
                    }
                    RoomCmd::Presence => {
                        tx.send(binary(Frame::new(
                            FrameType::Presence,
                            shared.presence_header(),
                            Vec::new(),
                        ))).await?;
                    }
                }
            }
            _ = presence.tick() => {
                tx.send(binary(Frame::new(
                    FrameType::Presence,
                    shared.presence_header(),
                    Vec::new(),
                ))).await?;
            }
        }
    }
}

async fn fetch_checkpoint(shared: &Shared, room: &str) -> Result<(i64, Vec<u8>)> {
    let resp = shared
        .http
        .get(format!("{}/room/{}/checkpoint", shared.http_url, room))
        .bearer_auth(shared.token())
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let seq = resp
        .headers()
        .get("x-arbos-checkpoint-seq")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let bytes = resp.bytes().await?.to_vec();
    Ok((seq, bytes))
}

fn binary(frame: Frame) -> WsMessage {
    WsMessage::Binary(frame.encode())
}

fn machine_from_wire(state: WireBox) -> MachineState {
    match state {
        WireBox::Awake => MachineState::Awake,
        WireBox::Waking => MachineState::Waking,
        WireBox::Asleep => MachineState::Asleep,
        WireBox::Error => MachineState::Error,
    }
}

fn map_box(state: WireBox) -> BoxState {
    match state {
        WireBox::Awake => BoxState::Awake,
        WireBox::Waking => BoxState::Waking,
        WireBox::Asleep => BoxState::HibernatedLight,
        WireBox::Error => BoxState::Error,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn project_of(shared: &Shared, chat_id: &str) -> Option<Id> {
    shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.id == chat_id)
        .map(|c| c.project_id.clone())
}

fn project_asleep(shared: &Shared, project_id: &str) -> bool {
    shared
        .projects
        .lock()
        .unwrap()
        .iter()
        .find(|p| p.id == project_id)
        .map(|p| p.state.is_asleep() || p.state == BoxState::Error)
        .unwrap_or(false)
}

fn send_wake(shared: &Arc<Shared>, project_id: &str, reason: &'static str) {
    {
        let rooms = shared.rooms.lock().unwrap();
        if let Some(room) = rooms.get(&project_doc_key(project_id)) {
            let _ = room.cmds.send(RoomCmd::Wake(reason));
        }
    }
    let shared = shared.clone();
    let project_id = project_id.to_string();
    tokio::spawn(async move {
        set_project_state(
            &shared,
            &project_id,
            BoxState::Waking,
            Some("Waking sandbox…".into()),
        );
        if let Err(err) = shared
            .post_json(&format!("/projects/{project_id}/wake"), json!({}))
            .await
        {
            shared.notice(format!("Wake failed: {err:#}"));
            set_project_state(
                &shared,
                &project_id,
                BoxState::Error,
                Some(format!("{err:#}")),
            );
        }
    });
}

pub(crate) fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "md" | "txt" | "rs" | "toml" | "json" | "ts" | "py" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// `GET /blob/{project}/{key}` → (mime, bytes).
async fn fetch_blob_bytes(
    shared: &Shared,
    project_id: &str,
    key: &str,
) -> Result<(String, Vec<u8>)> {
    let r = shared
        .http
        .get(format!("{}/blob/{}/{}", shared.http_url, project_id, key))
        .bearer_auth(shared.token())
        .send()
        .await?;
    if !r.status().is_success() {
        anyhow::bail!("HTTP {}", r.status());
    }
    let mime = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    Ok((mime, r.bytes().await?.to_vec()))
}

/// `~/Downloads/<name>`, or `<stem> (2).<ext>` when the name is taken.
pub(crate) fn unique_download_path(name: &str) -> PathBuf {
    let dir = credentials::downloads_dir();
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = std::path::Path::new(name);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_owned();
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    (2..1000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or(first)
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Upload attachments as blobs, then write the user row and its `send`
/// command in one commit (crdt-sync §6.2).
#[allow(clippy::too_many_arguments)]
async fn send_message(
    shared: Arc<Shared>,
    chat_id: Id,
    message_id: Id,
    text: String,
    attachments: Vec<PathBuf>,
    voice_wav: Option<Arc<Vec<u8>>>,
    command: &'static str,
    answer: Option<(String, serde_json::Value)>,
    reply_to: Option<String>,
) {
    let project_id = project_of(&shared, &chat_id).unwrap_or_default();
    // People chats keep their blobs under `people:<chat_id>` and have no box to wake.
    let blob_slot = if shared.is_people(&chat_id) {
        format!("people:{chat_id}")
    } else {
        project_id.clone()
    };
    let mut parts: Vec<Part> = Vec::new();
    for path in attachments {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string();
        let key = format!("att-{}-{}", new_id().to_lowercase(), safe_name(&name));
        let mime = mime_for(&path);
        let blob_ref = match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let put = shared
                    .http
                    .put(format!("{}/blob/{}/{}", shared.http_url, blob_slot, key))
                    .bearer_auth(shared.token())
                    .header("content-type", mime)
                    .body(bytes)
                    .send()
                    .await;
                match put {
                    Ok(r) if r.status().is_success() => Some(key),
                    Ok(r) => {
                        shared.notice(format!("Attachment upload failed: HTTP {}", r.status()));
                        None
                    }
                    Err(err) => {
                        shared.notice(format!("Attachment upload failed: {err}"));
                        None
                    }
                }
            }
            Err(err) => {
                shared.notice(format!("Could not read {}: {err}", path.display()));
                None
            }
        };
        parts.push(Part::Attachment {
            id: new_id(),
            attachment: Attachment {
                name,
                mime: mime.into(),
                blob_ref,
                bytes: None,
                kind: None,
                title: None,
            },
        });
    }
    if let Some(wav) = voice_wav {
        parts.push(upload_voice_memo(&shared, &project_id, wav).await);
    }
    parts.push(Part::Text {
        id: new_id(),
        text: text.clone(),
    });
    shared.unacked.lock().unwrap().insert(message_id.clone());
    // People chats, and projects with more than one human, carry who wrote
    // (API §3/§7); a solo project keeps the plain `user` the box expects.
    let in_people = shared.is_people(&chat_id) || {
        let projects = shared.projects.lock().unwrap();
        projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.shared || !p.people.is_empty())
            .unwrap_or(false)
    };
    {
        let docs = shared.docs.lock().unwrap();
        let Some(doc) = docs.get(&chat_id) else {
            return;
        };
        let me = shared.me_user();
        let mentions = if in_people {
            mentions_for(&shared, &chat_id, &text)
        } else {
            Vec::new()
        };
        let msg = Message {
            id: message_id.clone(),
            chat_id: chat_id.clone(),
            role: Role::User,
            sender: if in_people {
                format!("user:{}", shared.me_id())
            } else {
                "user".into()
            },
            // Always attach the author's display name so agents address them by name, not @handle.
            sender_handle: me.as_ref().map(|u| u.login.clone()),
            sender_name: me
                .as_ref()
                .map(|u| u.name.clone())
                .filter(|n| !n.is_empty()),
            sender_avatar: if in_people {
                me.as_ref().map(|u| u.avatar_url.clone())
            } else {
                None
            },
            sender_avatar_color: None,
            mentions,
            status: MessageStatus::Done,
            turn: None,
            decision: None,
            subagent_id: None,
            subagent_name: None,
            handoff_messages: None,
            reactions: Default::default(),
            in_reply_to: None,
            routine_name: None,
            sender_icon: None,
            reply_to,
            asked_by_handle: None,
            owner_handle: None,
            usage: None,
            return_contract: None,
            waiting_user: false,
            waiting_approval: false,
            model: None,
            effort: None,
            created_at: now_ms(),
            completed_at: None,
            call_id: None,
            activity: None,
            pending: false,
            parts,
            hidden_parts: Vec::new(),
            greeting: false,
        };
        let _ = doc.insert_message(&msg);
        let payload = match &answer {
            Some((qid, value)) => {
                json!({ "messageId": message_id, "questionId": qid, "value": value, "text": text })
            }
            None => json!({ "messageId": message_id }),
        };
        let _ = doc.push_command(command, payload, &shared.device_id());
        let untitled = shared
            .chats
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.id == chat_id && !c.is_pinned && c.title == "New chat");
        if untitled {
            let title = crate::app::title_from(&text);
            let _ = doc.push_command("retitle", json!({ "title": title }), &shared.device_id());
            if let Some(c) = shared
                .chats
                .lock()
                .unwrap()
                .iter_mut()
                .find(|c| c.id == chat_id)
            {
                c.title = title;
            }
        }
        doc.commit();
    }
    mark_activity(&shared, &chat_id);
    // Other people in the chat get a notification with the text (API §7c); the relay never
    // reads rows, so the writer announces. Solo projects have nobody else to tell.
    if in_people {
        let path = if shared.is_people(&chat_id) {
            format!("/chats/{chat_id}/notify")
        } else {
            format!("/projects/{project_id}/chats/{chat_id}/notify")
        };
        let body = json!({
            "message_id": message_id,
            "text": text.chars().take(600).collect::<String>(),
        });
        let announcer = shared.clone();
        tokio::spawn(async move {
            if let Err(err) = announcer.post_json(&path, body).await {
                eprintln!("announce message: {err:#}");
            }
        });
    }
    if shared.is_people(&chat_id) {
        if let Some(room) = shared.rooms.lock().unwrap().get(&chat_id) {
            let _ = room.cmds.send(RoomCmd::Typing(false));
        }
        return;
    }
    if project_asleep(&shared, &project_id) {
        send_wake(&shared, &project_id, "send");
    }
}

// ----- voice (F11) ----------------------------------------------------------

/// `PUT /blob/{project}/voice-<ulid>.wav`, then the `attachment` part that keeps the memo.
async fn upload_voice_memo(shared: &Shared, project_id: &str, wav: Arc<Vec<u8>>) -> Part {
    let key = format!("voice-{}.wav", new_id().to_lowercase());
    let put = shared
        .http
        .put(format!("{}/blob/{}/{}", shared.http_url, project_id, key))
        .bearer_auth(shared.token())
        .header("content-type", "audio/wav")
        .body(wav.as_ref().clone())
        .send()
        .await;
    let blob_ref = match put {
        Ok(r) if r.status().is_success() => Some(key),
        Ok(r) => {
            shared.notice(format!("Voice memo upload failed: HTTP {}", r.status()));
            None
        }
        Err(err) => {
            shared.notice(format!("Voice memo upload failed: {err}"));
            None
        }
    };
    Part::Attachment {
        id: new_id(),
        attachment: Attachment {
            name: "voice-memo.wav".into(),
            mime: "audio/wav".into(),
            blob_ref,
            bytes: None,
            kind: Some(AttachmentKind::Audio),
            title: None,
        },
    }
}

const STT_MODEL: &str = "gpt-4o-mini-transcribe";
const TTS_MODEL: &str = "gpt-4o-mini-tts";
const TTS_VOICE: &str = "coral";
/// OpenAI caps `input` at 4096 characters; longer replies are read up to the last sentence end.
const TTS_MAX_CHARS: usize = 4000;

/// `POST /proxy/v1/audio/transcriptions` (multipart; the relay adds the provider key, D2).
async fn transcribe(shared: Arc<Shared>, project_id: Id, request_id: u64, wav: Arc<Vec<u8>>) {
    let file = reqwest::multipart::Part::bytes(wav.as_ref().clone())
        .file_name("memo.wav")
        .mime_str("audio/wav")
        .unwrap_or_else(|_| reqwest::multipart::Part::bytes(wav.as_ref().clone()));
    let form = reqwest::multipart::Form::new()
        .text("model", STT_MODEL)
        .text("response_format", "json")
        .part("file", file);
    let resp = shared
        .http
        .post(format!("{}/proxy/v1/audio/transcriptions", shared.http_url))
        .bearer_auth(shared.token())
        .header("x-arbos-project", &project_id)
        .multipart(form)
        .send()
        .await;
    let event = match resp {
        Ok(r) => {
            let mock = r.headers().contains_key("x-arbos-voice-mock");
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            if status.is_success() {
                let text = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                SyncEvent::Transcript {
                    request_id,
                    text,
                    error: None,
                    mock,
                }
            } else {
                SyncEvent::Transcript {
                    request_id,
                    text: String::new(),
                    error: Some(format!(
                        "HTTP {status}: {}",
                        body.chars().take(200).collect::<String>()
                    )),
                    mock,
                }
            }
        }
        Err(err) => SyncEvent::Transcript {
            request_id,
            text: String::new(),
            error: Some(err.to_string()),
            mock: false,
        },
    };
    let _ = shared.events.send(event);
}

/// Cut `text` for TTS: at most `TTS_MAX_CHARS`, ending on a sentence when possible.
pub fn tts_input(text: &str) -> String {
    if text.chars().count() <= TTS_MAX_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(TTS_MAX_CHARS).collect();
    match cut.rfind(['.', '!', '?', '\n']) {
        Some(ix) if ix > TTS_MAX_CHARS / 2 => cut[..=ix].to_owned(),
        Some(_) | None => cut,
    }
}

/// `POST /proxy/v1/audio/speech` → WAV bytes.
async fn speak(shared: Arc<Shared>, project_id: Id, request_id: u64, text: String) {
    let body = json!({
        "model": TTS_MODEL,
        "voice": TTS_VOICE,
        "input": tts_input(&text),
        "response_format": "wav",
    });
    let resp = shared
        .http
        .post(format!("{}/proxy/v1/audio/speech", shared.http_url))
        .bearer_auth(shared.token())
        .header("x-arbos-project", &project_id)
        .json(&body)
        .send()
        .await;
    let event = match resp {
        Ok(r) => {
            let mock = r.headers().contains_key("x-arbos-voice-mock");
            let status = r.status();
            let mime = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("audio/wav")
                .to_owned();
            let bytes = r.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            if status.is_success() {
                SyncEvent::Speech {
                    request_id,
                    bytes: Arc::new(bytes),
                    mime,
                    error: None,
                    mock,
                }
            } else {
                SyncEvent::Speech {
                    request_id,
                    bytes: Arc::new(Vec::new()),
                    mime,
                    error: Some(format!(
                        "HTTP {status}: {}",
                        String::from_utf8_lossy(&bytes)
                            .chars()
                            .take(200)
                            .collect::<String>()
                    )),
                    mock,
                }
            }
        }
        Err(err) => SyncEvent::Speech {
            request_id,
            bytes: Arc::new(Vec::new()),
            mime: String::new(),
            error: Some(err.to_string()),
            mock: false,
        },
    };
    let _ = shared.events.send(event);
}

fn handle_command(shared: &Arc<Shared>, cmd: ClientCommand) {
    match cmd {
        ClientCommand::SendVoice {
            chat_id,
            message_id,
            text,
            wav,
        } => {
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                Vec::new(),
                Some(wav),
                "send",
                None,
                None,
            ));
        }
        ClientCommand::Transcribe {
            project_id,
            request_id,
            wav,
        } => {
            tokio::spawn(transcribe(shared.clone(), project_id, request_id, wav));
        }
        ClientCommand::Speak {
            project_id,
            request_id,
            text,
        } => {
            tokio::spawn(speak(shared.clone(), project_id, request_id, text));
        }
        ClientCommand::Pair { code } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let name = hostname();
                match shared
                    .post_json("/pair", json!({ "code": code.trim(), "name": name }))
                    .await
                {
                    Ok(v) => {
                        let token = v
                            .get("device_token")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if token.is_empty() {
                            let _ = shared
                                .events
                                .send(SyncEvent::PairingFailed("relay returned no token".into()));
                            return;
                        }
                        let creds = DeviceCredentials {
                            relay_url: shared.http_url.clone(),
                            device_id: v
                                .get("device_id")
                                .and_then(Value::as_str)
                                .unwrap_or("device")
                                .to_string(),
                            access_token: token,
                            refresh_token: None,
                            expires_at: 0,
                            user: None,
                        };
                        if let Err(err) = credentials::store(&creds) {
                            shared.notice(format!("Token saved in memory only: {err:#}"));
                        }
                        *shared.creds.write().unwrap() = Some(creds.clone());
                        let _ = shared.events.send(SyncEvent::Paired {
                            device_id: creds.device_id,
                        });
                        start_after_pairing(&shared);
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PairingFailed(format!("{err:#}")));
                    }
                }
            });
        }
        ClientCommand::SignIn { grant_code } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .post(format!("{}/auth/token", shared.http_url))
                    .json(&json!({ "grant_code": grant_code }))
                    .send()
                    .await;
                let outcome: Result<DeviceCredentials> = async {
                    let resp = resp.context("POST /auth/token")?;
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if !status.is_success() {
                        let message = serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|v| {
                                v.pointer("/error/message")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                            .unwrap_or(text);
                        return Err(anyhow!("{message}"));
                    }
                    let v: Value = serde_json::from_str(&text).context("/auth/token: bad JSON")?;
                    shared.apply_token_response(&v)
                }
                .await;
                match outcome {
                    Ok(creds) => {
                        if let Some(user) = creds.user.clone() {
                            let _ = shared.events.send(SyncEvent::Account(Some(AccountView {
                                id: user.id,
                                login: user.login,
                                name: user.name,
                                avatar_url: user.avatar_url,
                                ..AccountView::default()
                            })));
                        }
                        let _ = shared.events.send(SyncEvent::Paired {
                            device_id: creds.device_id,
                        });
                        start_after_pairing(&shared);
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PairingFailed(format!("{err:#}")));
                    }
                }
            });
        }
        ClientCommand::SignOut => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let _ = shared.post_json("/auth/logout", json!({})).await;
                shared.sign_out_locally();
            });
        }
        ClientCommand::FocusChat { chat_id } => {
            let changed = {
                let mut focused = shared.focused_chat.lock().unwrap();
                let changed = *focused != chat_id;
                *focused = chat_id;
                changed
            };
            if changed {
                // Tell every room at once rather than on the next 15 s tick.
                for room in shared.rooms.lock().unwrap().values() {
                    let _ = room.cmds.send(RoomCmd::Presence);
                }
            }
        }
        ClientCommand::OpenChat { chat_id } | ClientCommand::MarkRead { chat_id } => {
            {
                let mut chats = shared.chats.lock().unwrap();
                if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
                    c.unread = false;
                }
            }
            // Next cold launch reopens this chat first (and evicts it last);
            // selecting the agent again on this Mac opens it too.
            if let Some(project_id) = project_of(shared, &chat_id) {
                shared.with_store(|s| {
                    s.touch_doc(&chat_id);
                    s.put_json(store::KEY_SELECTION, &(project_id.clone(), chat_id.clone()));
                    let mut last: HashMap<Id, Id> =
                        s.get_json(store::KEY_LAST_CHATS).unwrap_or_default();
                    last.insert(project_id, chat_id.clone());
                    s.put_json(store::KEY_LAST_CHATS, &last);
                });
            }
            if shared.is_people(&chat_id) {
                let seq = shared
                    .room_seq
                    .lock()
                    .unwrap()
                    .get(&chat_id)
                    .copied()
                    .unwrap_or(0);
                if seq > 0 {
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        let _ = shared
                            .put_json(&format!("/chats/{chat_id}/read"), json!({ "seq": seq }))
                            .await;
                    });
                }
            }
        }
        ClientCommand::SearchPeople { query } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let q = query.trim().trim_start_matches('@');
                // Empty query: the directory (`GET /users`, `{users: [...]}`);
                // otherwise the prefix search (a bare array).
                let path = if q.is_empty() {
                    "/users?limit=200".to_string()
                } else {
                    format!("/users/search?q={}", crate::auth::url_encode(q))
                };
                match shared.get_json(&path).await {
                    Ok(v) => {
                        let results: Vec<Person> = v
                            .as_array()
                            .or_else(|| v.get("users").and_then(|u| u.as_array()))
                            .map(|a| {
                                a.iter()
                                    .filter_map(|p| {
                                        serde_json::from_value::<Person>(p.clone()).ok()
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleSearch { query, results });
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(people_error(&err)));
                    }
                }
            });
        }
        ClientCommand::StartDm { handle } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json("/chats/dm", json!({ "handle": handle }))
                    .await
                {
                    Ok(_) => {
                        if let Err(err) = refresh_registry(&shared).await {
                            shared.notice(format!("Relay: {err:#}"));
                        }
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(people_error(&err)));
                    }
                }
            });
        }
        ClientCommand::SetTyping { chat_id, active } => {
            if let Some(room) = shared.rooms.lock().unwrap().get(&chat_id) {
                let _ = room.cmds.send(RoomCmd::Typing(active));
            }
        }
        ClientCommand::AddAgent {
            chat_id,
            project_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        &format!("/chats/{chat_id}/agents"),
                        json!({ "project_id": project_id }),
                    )
                    .await
                {
                    Ok(_) => {
                        if let Err(err) = refresh_registry(&shared).await {
                            shared.notice(format!("Relay: {err:#}"));
                        }
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(people_error(&err)));
                    }
                }
            });
        }
        ClientCommand::RemoveAgent {
            chat_id,
            project_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/chats/{chat_id}/agents/{project_id}"))
                    .await
                {
                    let _ = shared
                        .events
                        .send(SyncEvent::PeopleError(people_error(&err)));
                }
                let _ = refresh_registry(&shared).await;
            });
        }
        ClientCommand::RemoveMember { chat_id, user_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/chats/{chat_id}/members/{user_id}"))
                    .await
                {
                    let _ = shared
                        .events
                        .send(SyncEvent::PeopleError(people_error(&err)));
                }
                let _ = refresh_registry(&shared).await;
            });
        }
        ClientCommand::LeaveChat { chat_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .post_json(&format!("/chats/{chat_id}/leave"), json!({}))
                    .await
                {
                    let _ = shared
                        .events
                        .send(SyncEvent::PeopleError(people_error(&err)));
                }
                shared.people.lock().unwrap().remove(&chat_id);
                shared.with_store(|s| s.delete_doc(&chat_id));
                let _ = refresh_registry(&shared).await;
            });
        }
        ClientCommand::StartCall { chat_id, kind } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let path = if shared.is_people(&chat_id) {
                    format!("/chats/{chat_id}/calls")
                } else {
                    let project = project_of(&shared, &chat_id).unwrap_or_default();
                    format!("/projects/{project}/chats/{chat_id}/calls")
                };
                let client = [("X-Arbos-Client", update::client_header())];
                match shared
                    .post_json_status(&path, json!({ "kind": kind }), &client)
                    .await
                {
                    Ok((status, v)) => match parse_call(&v) {
                        Some(call) => {
                            // 201 `{call, joined: false, reachable}` = I started it
                            // (and write the line).
                            let joined = v.get("joined").and_then(Value::as_bool);
                            let created = match joined {
                                Some(j) => !j,
                                None => status == reqwest::StatusCode::CREATED,
                            };
                            let reachable = if created {
                                v.get("reachable")
                                    .cloned()
                                    .and_then(|r| serde_json::from_value::<Vec<Reachable>>(r).ok())
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            *shared.in_call.lock().unwrap() = Some(call.call_id.clone());
                            presence_now(&shared, &call.chat_id);
                            let _ = shared.events.send(SyncEvent::CallReady {
                                call,
                                created,
                                reachable,
                            });
                        }
                        None => {
                            let _ = shared.events.send(SyncEvent::CallError {
                                chat_id,
                                message: "The relay answered without a call.".into(),
                            });
                        }
                    },
                    Err(err) => {
                        let _ = shared.events.send(SyncEvent::CallError {
                            chat_id,
                            message: call_error(&err),
                        });
                    }
                }
            });
        }
        ClientCommand::JoinCall { call_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(&format!("/calls/{call_id}/join"), json!({}))
                    .await
                {
                    Ok(v) => {
                        if let Some(call) = parse_call(&v) {
                            *shared.in_call.lock().unwrap() = Some(call.call_id.clone());
                            presence_now(&shared, &call.chat_id);
                            let _ = shared.events.send(SyncEvent::CallReady {
                                call,
                                created: false,
                                reachable: Vec::new(),
                            });
                        }
                    }
                    Err(err) => {
                        let _ = shared.events.send(SyncEvent::CallError {
                            chat_id: String::new(),
                            message: call_error(&err),
                        });
                    }
                }
            });
        }
        ClientCommand::LeaveCall { call_id } => {
            let shared = shared.clone();
            *shared.in_call.lock().unwrap() = None;
            for room in shared.rooms.lock().unwrap().values() {
                let _ = room.cmds.send(RoomCmd::Presence);
            }
            tokio::spawn(async move {
                match shared
                    .post_json(&format!("/calls/{call_id}/leave"), json!({}))
                    .await
                {
                    Ok(v) => {
                        let left: CallLeft = serde_json::from_value(v).unwrap_or_default();
                        if !left.ended {
                            // Others are still on it: refresh the list behind the Join bar.
                            if let Ok(v) = shared.get_json(&format!("/calls/{call_id}")).await {
                                if let Some(call) = parse_call(&v) {
                                    let _ = shared.events.send(SyncEvent::CallInfo(call));
                                }
                            }
                        }
                        if left.ended {
                            // I am the writer of the end line (contract §1).
                            let _ = shared.events.send(SyncEvent::CallEnded(CallEnded {
                                call_id,
                                chat_id: String::new(),
                                project_id: String::new(),
                                duration_s: left.duration_s,
                                reason: "last_left".into(),
                                writer_device: Some(shared.device_id()),
                                writer_is_me: true,
                            }));
                        }
                    }
                    Err(err) => shared.notice(format!("Leave call: {}", call_error(&err))),
                }
            });
        }
        ClientCommand::FetchCall { call_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Ok(v) = shared.get_json(&format!("/calls/{call_id}")).await {
                    if let Some(call) = parse_call(&v) {
                        let _ = shared.events.send(SyncEvent::CallInfo(call));
                    }
                }
            });
        }
        ClientCommand::SystemLine {
            chat_id,
            text,
            call_id,
        } => {
            write_system_line(shared, &chat_id, text, call_id);
        }
        ClientCommand::Greeting {
            chat_id,
            project_id,
            agent_name,
        } => {
            if write_greeting(shared, &chat_id, &project_id, &agent_name) {
                emit_snapshot(shared, &chat_id);
                let _ = shared.events.send(SyncEvent::GreetingWritten(chat_id));
            }
        }
        ClientCommand::PollNow => {
            // First login (D82): the 10–15 s polls are too slow for stage changes
            // that take seconds; the onboarding timer asks every 1.5 s instead.
            let shared = shared.clone();
            tokio::spawn(async move {
                let _ = refresh_registry(&shared).await;
                refresh_machine(&shared).await;
            });
        }
        ClientCommand::BlockUser { handle } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json("/blocks", json!({ "handle": handle }))
                    .await
                {
                    Ok(_) => shared.notice(format!("@{handle} blocked.")),
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(people_error(&err)));
                    }
                }
            });
        }
        ClientCommand::InviteToProject { project_id, handle } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        &format!("/projects/{project_id}/members"),
                        json!({ "handle": handle }),
                    )
                    .await
                {
                    Ok(_) => {
                        let _ = refresh_registry(&shared).await;
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(people_error(&err)));
                    }
                }
            });
        }
        ClientCommand::RemoveProjectMember {
            project_id,
            user_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/projects/{project_id}/members/{user_id}"))
                    .await
                {
                    let _ = shared
                        .events
                        .send(SyncEvent::PeopleError(people_error(&err)));
                }
                let _ = refresh_registry(&shared).await;
            });
        }
        ClientCommand::FetchAvatar { url } => {
            tokio::spawn(fetch_avatar(shared.clone(), url));
        }
        ClientCommand::Send {
            chat_id,
            message_id,
            text,
            attachments,
            reply_to,
        } => {
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                attachments,
                None,
                "send",
                None,
                reply_to,
            ));
        }
        ClientCommand::Steer {
            chat_id,
            message_id,
            text,
            reply_to,
        } => {
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                Vec::new(),
                None,
                "steer",
                None,
                reply_to,
            ));
        }
        ClientCommand::Answer {
            chat_id,
            message_id,
            text,
            question_id,
            value,
        } => {
            let answer = question_id.map(|qid| (qid, value.unwrap_or(json!(null))));
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                Vec::new(),
                None,
                "answer",
                answer,
                None,
            ));
        }
        ClientCommand::Stop { chat_id } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc.push_command("stop", json!({}), &shared.device_id());
                doc.commit();
            }
        }
        ClientCommand::React {
            chat_id,
            message_id,
            emoji,
        } => {
            let me = format!("user:{}", shared.me_id());
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                if let Err(e) = doc.toggle_reaction(&message_id, &emoji, &me) {
                    eprintln!("relay: reaction: {e:#}");
                }
                doc.commit();
            }
        }
        ClientCommand::Approve {
            chat_id,
            approval_id,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc.push_command(
                    "approve",
                    json!({ "approvalId": approval_id }),
                    &shared.device_id(),
                );
                doc.commit();
            }
        }
        ClientCommand::Reject {
            chat_id,
            approval_id,
            reason,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let mut payload = json!({ "approvalId": approval_id });
                if let Some(r) = reason.filter(|r| !r.trim().is_empty()) {
                    payload["reason"] = json!(r);
                }
                let _ = doc.push_command("reject", payload, &shared.device_id());
                doc.commit();
            }
        }
        ClientCommand::Retry {
            chat_id,
            message_id,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let _ = doc.push_command(
                    "retry",
                    json!({ "messageId": message_id }),
                    &shared.device_id(),
                );
                doc.commit();
            }
        }
        ClientCommand::NewChat {
            project_id,
            chat_id: local_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        &format!("/projects/{project_id}/chats"),
                        json!({ "title": "New chat" }),
                    )
                    .await
                {
                    Ok(v) => {
                        let Some(chat) = v.get("chat").and_then(parse_chat) else {
                            shared.notice("Relay returned no chat");
                            return;
                        };
                        shared.chats.lock().unwrap().push(chat.clone());
                        ensure_room(&shared, &project_id, &chat.id, &chat.id);
                        let _ = shared.events.send(SyncEvent::ChatCreated {
                            project_id: project_id.clone(),
                            local_id,
                            chat,
                        });
                    }
                    Err(err) => shared.notice(format!("New chat failed: {err:#}")),
                }
            });
        }
        ClientCommand::GroupCreate {
            project_id,
            chat_id: local_id,
            participants,
            title,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        &format!("/projects/{project_id}/groups"),
                        json!({ "title": title, "participants": participants }),
                    )
                    .await
                {
                    Ok(v) => {
                        let Some(chat) = parse_chat(&v) else {
                            shared.notice("Relay returned no group");
                            return;
                        };
                        shared.chats.lock().unwrap().push(chat.clone());
                        ensure_room(&shared, &project_id, &chat.id, &chat.id);
                        let _ = shared.events.send(SyncEvent::ChatCreated {
                            project_id: project_id.clone(),
                            local_id,
                            chat,
                        });
                    }
                    Err(err) => shared.notice(format!("New group failed: {err:#}")),
                }
            });
        }
        ClientCommand::GroupAddParticipant {
            project_id,
            chat_id,
            participant,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .patch_json(
                        &format!("/projects/{project_id}/groups/{chat_id}"),
                        json!({ "add": [participant] }),
                    )
                    .await
                {
                    Ok(v) => {
                        if let Some(chat) = parse_chat(&v) {
                            let mut chats = shared.chats.lock().unwrap();
                            match chats.iter_mut().find(|c| c.id == chat.id) {
                                Some(slot) => {
                                    slot.participants = chat.participants;
                                    slot.title = chat.title;
                                }
                                None => chats.push(chat),
                            }
                        }
                        emit_chats(&shared);
                    }
                    Err(err) => shared.notice(format!("Add to group failed: {err:#}")),
                }
            });
        }
        ClientCommand::ArchiveChat { chat_id, archived } => {
            cache_archived(shared, &chat_id, archived);
            let project_id = shared
                .chats
                .lock()
                .unwrap()
                .iter_mut()
                .find(|c| c.id == chat_id)
                .map(|c| {
                    c.archived_at = archived.then(now_ms);
                    c.project_id.clone()
                });
            let Some(project_id) = project_id else {
                return;
            };
            let shared = shared.clone();
            tokio::spawn(async move {
                match set_archived(&shared, &project_id, &chat_id, archived).await {
                    Ok(at) => {
                        if archived {
                            if let Some(at) = at {
                                if let Some(c) = shared
                                    .chats
                                    .lock()
                                    .unwrap()
                                    .iter_mut()
                                    .find(|c| c.id == chat_id)
                                {
                                    c.archived_at = Some(at);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        // Older relay without §9c: the Mac keeps its local archive.
                        if !err.to_string().contains("HTTP 404") {
                            shared.notice(format!("Archive: {}", people_error(&err)));
                        }
                    }
                }
            });
        }
        ClientCommand::SetAvatarColorIndex { project_id, index } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.avatar_color_override = index;
                    p.avatar_color_index = Some(crate::ui::avatars::index_for(&project_id, index));
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({ "avatar_color_index": index });
                match shared
                    .patch_json(&format!("/projects/{project_id}"), body)
                    .await
                {
                    Ok(_) => {}
                    Err(err) => {
                        if !err.to_string().contains("HTTP 404") {
                            shared
                                .notice(format!("Avatar colour not saved: {}", people_error(&err)));
                        }
                    }
                }
            });
        }
        ClientCommand::UpdateProfile { name, handle } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .patch_json("/me", json!({ "name": name, "login": handle }))
                    .await
                {
                    Ok(_) => refresh_me(&shared).await,
                    Err(err) => {
                        let text = format!("{err:#}");
                        let human = if text.contains("handle_taken") {
                            "That handle is already taken.".to_string()
                        } else if text.contains("bad_handle") {
                            "Handle must be 1–39 letters, numbers, or hyphens.".to_string()
                        } else {
                            format!("Profile: {text}")
                        };
                        shared.notice(human);
                        refresh_me(&shared).await;
                    }
                }
            });
        }
        ClientCommand::SetDescription {
            project_id,
            description,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.description = Some(description.clone()).filter(|d| !d.trim().is_empty());
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .patch_json(
                        &format!("/projects/{project_id}"),
                        json!({ "description": description }),
                    )
                    .await
                {
                    shared.notice(format!("Description: {err:#}"));
                }
            });
        }
        ClientCommand::SetProjectName { project_id, name } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.name = name.clone();
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .patch_json(&format!("/projects/{project_id}"), json!({ "name": name }))
                    .await
                {
                    shared.notice(format!("Rename: {err:#}"));
                }
            });
        }
        ClientCommand::SetMuted { space_id, muted } => {
            if !shared.is_people(&space_id) {
                return;
            }
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == space_id) {
                    p.muted = muted;
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .put_json(
                        &format!("/chats/{space_id}/settings"),
                        json!({ "muted": muted }),
                    )
                    .await
                {
                    shared.notice(format!("Mute: {err:#}"));
                }
            });
        }
        ClientCommand::FetchVoices => {
            let shared = shared.clone();
            tokio::spawn(async move {
                fetch_voices(&shared).await;
            });
        }
        ClientCommand::FetchVoiceSample { url } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let dir = credentials::voice_sample_cache_dir();
                let _ = tokio::fs::create_dir_all(&dir).await;
                let ext = url
                    .rsplit('/')
                    .next()
                    .and_then(|n| n.rsplit_once('.'))
                    .map(|(_, e)| e.split('?').next().unwrap_or("").to_string())
                    .filter(|e| !e.is_empty() && e.len() <= 5)
                    .unwrap_or_else(|| "mp3".into());
                let mut h = Sha256::new();
                h.update(url.as_bytes());
                let path = dir.join(format!("{:x}.{ext}", h.finalize()));
                if !path.is_file() {
                    let resp = shared.http.get(&url).send().await;
                    match resp {
                        Ok(r) if r.status().is_success() => match r.bytes().await {
                            Ok(bytes) => {
                                if let Err(err) = tokio::fs::write(&path, &bytes).await {
                                    shared.notice(format!("Voice sample: {err}"));
                                    return;
                                }
                            }
                            Err(err) => {
                                shared.notice(format!("Voice sample: {err}"));
                                return;
                            }
                        },
                        Ok(r) => {
                            shared.notice(format!("Voice sample: HTTP {}", r.status()));
                            return;
                        }
                        Err(err) => {
                            shared.notice(format!("Voice sample: {err}"));
                            return;
                        }
                    }
                }
                let _ = shared.events.send(SyncEvent::VoiceSample { url, path });
            });
        }
        ClientCommand::SetVisionEnabled {
            project_id,
            enabled,
        } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.vision_enabled = enabled;
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({ "vision_enabled": enabled });
                match shared
                    .patch_json(&format!("/projects/{project_id}"), body)
                    .await
                {
                    Ok(_) => {}
                    Err(err) => {
                        if !err.to_string().contains("HTTP 404") {
                            shared
                                .notice(format!("Camera vision not saved: {}", people_error(&err)));
                        }
                    }
                }
            });
        }
        ClientCommand::SetVoice { project_id, voice } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.voice = voice.clone();
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({ "voice": voice });
                match shared
                    .patch_json(&format!("/projects/{project_id}"), body)
                    .await
                {
                    Ok(_) => {}
                    Err(err) => shared.notice(format!("Voice not saved: {}", people_error(&err))),
                }
            });
        }
        ClientCommand::SetModel {
            project_id,
            model,
            effort,
        } => {
            // Mirror into the project doc `settings` too (crdt-sync §6.3): the
            // box reads the doc first, so both must agree.
            {
                let docs = shared.docs.lock().unwrap();
                if let Some(doc) = docs.get(&project_doc_key(&project_id)) {
                    let settings = doc.doc.get_map("settings");
                    let _ = settings.insert("agentModel", model.as_str());
                    let _ = settings.insert("agentEffort", effort_to(effort));
                    let models: loro::LoroValue = serde_json::from_value(json!({
                        "talking_model": { "id": model, "effort": effort_to(effort) }
                    }))
                    .unwrap_or(loro::LoroValue::Null);
                    let _ = settings.insert("models", models);
                    doc.commit();
                }
            }
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.model = model.clone();
                    p.effort = effort;
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            // Canonical path: the registry. Slots merge one by one.
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({
                    "models": { "talking_model": { "id": model, "effort": effort_to(effort) } }
                });
                let resp = shared
                    .http
                    .patch(format!("{}/projects/{}", shared.http_url, project_id))
                    .bearer_auth(shared.token())
                    .json(&body)
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("Model change not saved: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Model change not saved: {err}")),
                }
            });
        }
        ClientCommand::CreateProject {
            name,
            avatar_color_index,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let provider = std::env::var("ARBOS_BOX_PROVIDER").ok();
                let mut body = json!({ "name": name });
                if let Some(p) = provider {
                    body["provider"] = json!(p);
                }
                if let Some(i) = avatar_color_index {
                    body["avatar_color_index"] = json!(i);
                }
                match shared.post_json("/projects", body).await {
                    Ok(_) => {
                        if let Err(err) = refresh_registry(&shared).await {
                            shared.notice(format!("Relay: {err:#}"));
                        }
                    }
                    Err(err) => shared.notice(format!("New project failed: {err:#}")),
                }
            });
        }
        ClientCommand::MachineRestart => {
            let shared = shared.clone();
            tokio::spawn(async move {
                set_machine(
                    &shared,
                    MachineState::Waking,
                    None,
                    Some("Restarting your machine…".into()),
                );
                if let Err(err) = shared.post_json("/machine/stop", json!({})).await {
                    shared.notice(format!("Machine restart: {err:#}"));
                }
                match shared.post_json("/machine/start", json!({})).await {
                    Ok(v) => {
                        if let Some(word) = v.get("state").and_then(Value::as_str) {
                            set_machine(
                                &shared,
                                machine_state_from_word(word),
                                v.get("since").and_then(Value::as_u64).filter(|v| *v > 0),
                                None,
                            );
                        }
                    }
                    Err(err) => shared.notice(format!("Machine restart: {err:#}")),
                }
                refresh_machine(&shared).await;
            });
        }
        ClientCommand::MachineStart | ClientCommand::MachineStop => {
            let start = matches!(cmd, ClientCommand::MachineStart);
            let shared = shared.clone();
            tokio::spawn(async move {
                let verb = if start { "start" } else { "stop" };
                set_machine(
                    &shared,
                    if start {
                        MachineState::Waking
                    } else {
                        MachineState::Asleep
                    },
                    None,
                    None,
                );
                match shared
                    .post_json(&format!("/machine/{verb}"), json!({}))
                    .await
                {
                    Ok(v) => {
                        if let Some(word) = v.get("state").and_then(Value::as_str) {
                            set_machine(
                                &shared,
                                machine_state_from_word(word),
                                v.get("since").and_then(Value::as_u64).filter(|v| *v > 0),
                                None,
                            );
                        }
                    }
                    Err(err) => shared.notice(format!("Machine {verb}: {err:#}")),
                }
                refresh_machine(&shared).await;
            });
        }
        ClientCommand::FetchBlob {
            project_id,
            key,
            save,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .get(format!("{}/blob/{}/{}", shared.http_url, project_id, key))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {
                        let mime = r
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("application/octet-stream")
                            .to_string();
                        let bytes = r.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
                        let saved_to = if save {
                            let path = credentials::downloads_dir().join(safe_name(&key));
                            match tokio::fs::write(&path, &bytes).await {
                                Ok(()) => Some(path),
                                Err(err) => {
                                    shared.notice(format!("Could not save: {err}"));
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        let _ = shared.events.send(SyncEvent::Blob {
                            project_id,
                            key,
                            mime,
                            bytes: Arc::new(bytes),
                            saved_to,
                        });
                    }
                    Ok(r) => shared.notice(format!("Blob {key}: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Blob {key}: {err}")),
                }
            });
        }
        ClientCommand::FetchThumbnail { project_id, key } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match fetch_blob_bytes(&shared, &project_id, &key).await {
                    Ok((_, bytes)) => {
                        let _ = shared.events.send(SyncEvent::Thumbnail {
                            key,
                            bytes: Arc::new(bytes),
                        });
                    }
                    Err(err) => shared.notice(format!("Thumbnail {key}: {err}")),
                }
            });
        }
        ClientCommand::DownloadAttachment {
            project_id,
            key,
            name,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match fetch_blob_bytes(&shared, &project_id, &key).await {
                    Ok((_, bytes)) => {
                        let path = unique_download_path(&safe_name(&name));
                        match tokio::fs::write(&path, &bytes).await {
                            Ok(()) => {
                                let _ = shared.events.send(SyncEvent::AttachmentSaved { path });
                            }
                            Err(err) => shared.notice(format!("Could not save {name}: {err}")),
                        }
                    }
                    Err(err) => shared.notice(format!("Download {name}: {err}")),
                }
            });
        }
        ClientCommand::RefreshConnectors => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_connectors(&shared).await;
            });
        }
        ClientCommand::ConnectorConnect { provider, token } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = match token {
                    Some(t) => json!({ "token": t }),
                    None => json!({}),
                };
                match shared
                    .post_json(&format!("/connectors/{provider}/connect"), body)
                    .await
                {
                    Ok(v) => {
                        if let Some(url) = v.get("authUrl").and_then(Value::as_str) {
                            let _ = shared.events.send(SyncEvent::ConnectorAuth {
                                provider: provider.clone(),
                                auth_url: url.to_string(),
                            });
                        } else if let Some(c) = v.get("connector") {
                            let status = c["status"].as_str().unwrap_or("");
                            if status == "connected" {
                                shared.notice(format!("{provider} connected."));
                            } else {
                                shared.notice(format!(
                                    "{provider}: {status} {}",
                                    c["last_error"].as_str().unwrap_or("")
                                ));
                            }
                        }
                        // The loopback callback lands out of band: poll the list for a while.
                        for wait in [0u64, 4, 8, 15, 30, 60] {
                            sleep(Duration::from_secs(wait)).await;
                            refresh_connectors(&shared).await;
                        }
                    }
                    Err(err) => shared.notice(format!("Connect {provider}: {err:#}")),
                }
            });
        }
        ClientCommand::ConnectorDisconnect { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .delete(format!("{}/connectors/{}", shared.http_url, id))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("Disconnect: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Disconnect: {err}")),
                }
                refresh_connectors(&shared).await;
            });
        }
        ClientCommand::SecretsRefresh => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::ProjectSecretsRefresh { project_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_project_secrets(&shared, &project_id).await;
            });
        }
        ClientCommand::SecretAdd { item } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let mut body = json!({ "name": item.name, "kind": item.kind, "note": item.note, "hosts": item.hosts,
                    "inject_env": item.inject_env,
                    "agents": item.agents.iter().map(|p| json!({ "project_id": p })).collect::<Vec<_>>() });
                if item.kind == "login" {
                    let fields: serde_json::Map<String, Value> = item
                        .fields
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect();
                    body["fields"] = Value::Object(fields);
                } else if let Some((_, v)) = item.fields.iter().find(|(k, _)| k == "value") {
                    body["value"] = Value::String(v.clone());
                }
                match shared.post_json("/secrets", body).await {
                    Ok(v) => {
                        let env = v["secret"]["env"].as_str().unwrap_or(&item.name);
                        shared.notice(format!("Secrets: saved {env}."));
                    }
                    Err(err) => shared.notice(format!("Secrets: {}", relay_message(&err))),
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretPatch { id, body } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.patch_json(&format!("/secrets/{id}"), body).await {
                    shared.notice(format!("Secrets: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretReplaceValue { id, body } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared.put_json(&format!("/secrets/{id}/value"), body).await {
                    Ok(_) => shared.notice(
                        "Secrets: value replaced. The old one works for 24 hours.".to_string(),
                    ),
                    Err(err) => shared.notice(format!("Secrets: {}", relay_message(&err))),
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretDelete { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&format!("/secrets/{id}")).await {
                    shared.notice(format!("Delete secret: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretAccess {
            id,
            project_id,
            enabled,
            note,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let mut body = json!({ "enabled": enabled });
                if let Some(n) = note {
                    body["note"] = Value::String(n);
                }
                if let Err(err) = shared
                    .put_json(&format!("/secrets/{id}/access/{project_id}"), body)
                    .await
                {
                    shared.notice(format!("Secrets: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
                refresh_project_secrets(&shared, &project_id).await;
            });
        }
        ClientCommand::SecretRevokeAll { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .post_json(&format!("/secrets/{id}/revoke-all"), json!({}))
                    .await
                {
                    shared.notice(format!("Secrets: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretsPause { paused } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .put_json("/me/secrets/settings", json!({ "paused": paused }))
                    .await
                {
                    shared.notice(format!("Secrets: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretDecide {
            request_id,
            decision,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        &format!("/secrets/requests/{request_id}/decide"),
                        json!({ "decision": decision }),
                    )
                    .await
                {
                    Ok(v) => {
                        let name = v["request"]["name"].as_str().unwrap_or("the secret");
                        let agent = v["request"]["project_name"].as_str().unwrap_or("the agent");
                        shared.notice(match decision.as_str() {
                            "deny" => format!("{agent} asked for {name}. Denied."),
                            "once" => format!("{agent} may use {name} once."),
                            "chat" => format!("{agent} may use {name} in this chat."),
                            _ => format!("{agent} may use {name}."),
                        });
                    }
                    Err(err) => shared.notice(format!("Secrets: {}", relay_message(&err))),
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::VaultConnect {
            token,
            agents,
            note,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({ "provider": "onepassword", "token": token,
                    "agents": agents.iter().map(|p| json!({ "project_id": p, "note": note })).collect::<Vec<_>>() });
                match shared.post_json("/vaults", body).await {
                    Ok(v) => shared.notice(format!(
                        "Connected {}.",
                        v["vault"]["name"].as_str().unwrap_or("the vault")
                    )),
                    Err(err) => {
                        let text = format!("{err:#}");
                        shared.notice(if text.contains("bad_token") {
                            "That token didn't work. Check it was copied whole.".to_string()
                        } else if text.contains("no_vaults") {
                            "This token can read no vaults.".to_string()
                        } else if text.contains("rate_limited") {
                            "1Password rate limit reached — try again later.".to_string()
                        } else {
                            format!("Connect 1Password: {}", relay_message(&err))
                        });
                    }
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::VaultAccess {
            id,
            project_id,
            enabled,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .put_json(
                        &format!("/vaults/{id}/access/{project_id}"),
                        json!({ "enabled": enabled }),
                    )
                    .await
                {
                    shared.notice(format!("Secrets: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
                refresh_project_secrets(&shared, &project_id).await;
            });
        }
        ClientCommand::VaultItemEnv { id, reference, on } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .put_json(
                        &format!("/vaults/{id}/items/env"),
                        json!({ "ref": reference, "inject_env": on }),
                    )
                    .await
                {
                    shared.notice(format!("Secrets: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::VaultReplaceToken { id, token } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .put_json(&format!("/vaults/{id}/token"), json!({ "token": token }))
                    .await
                {
                    Ok(_) => shared.notice("Token replaced.".to_string()),
                    Err(err) => {
                        let text = format!("{err:#}");
                        shared.notice(if text.contains("bad_token") {
                            "That token didn't work. Check it was copied whole.".to_string()
                        } else {
                            format!("Replace token: {}", relay_message(&err))
                        });
                    }
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::VaultDisconnect { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&format!("/vaults/{id}")).await {
                    shared.notice(format!("Disconnect: {}", relay_message(&err)));
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::RoutinesRefresh { project_id } => {
            let shared = shared.clone();
            tokio::spawn(async move { refresh_routines(&shared, &project_id).await });
        }
        ClientCommand::LocalRefresh => {
            let shared = shared.clone();
            tokio::spawn(async move { refresh_local(&shared).await });
        }
        ClientCommand::LocalSetAllowReads { device_id, allow } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .patch(format!("{}/local/devices/{}", shared.http_url, device_id))
                    .bearer_auth(shared.token())
                    .json(&json!({ "allow_reads": allow }))
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("This Mac: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("This Mac: {err}")),
                }
                refresh_local(&shared).await;
            });
        }
        ClientCommand::LocalForget { device_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .delete(format!("{}/local/devices/{}", shared.http_url, device_id))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("Forget helper: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Forget helper: {err}")),
                }
                refresh_local(&shared).await;
            });
        }
        ClientCommand::RoutineCreate {
            project_id,
            name,
            prompt,
            chat_id,
            trigger,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({
                    "name": name, "prompt": prompt, "chatId": chat_id,
                    "trigger": serde_json::to_value(&trigger).unwrap_or(Value::Null),
                });
                match shared
                    .post_json(&format!("/projects/{project_id}/routines"), body)
                    .await
                {
                    Ok(_) => refresh_routines(&shared, &project_id).await,
                    Err(err) => shared.notice(format!("Routine not created: {err:#}")),
                }
            });
        }
        ClientCommand::RoutineSetEnabled {
            project_id,
            routine_id,
            enabled,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .patch(format!(
                        "{}/projects/{}/routines/{}",
                        shared.http_url, project_id, routine_id
                    ))
                    .bearer_auth(shared.token())
                    .json(&json!({ "enabled": enabled }))
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("Routine not updated: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Routine not updated: {err}")),
                }
                refresh_routines(&shared, &project_id).await;
            });
        }
        ClientCommand::RoutineRunNow {
            project_id,
            routine_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .post_json(
                        &format!("/projects/{project_id}/routines/{routine_id}/run"),
                        json!({}),
                    )
                    .await
                {
                    shared.notice(format!("Run now failed: {err:#}"));
                }
                refresh_routine_runs(&shared, &project_id, &routine_id).await;
                refresh_routines(&shared, &project_id).await;
            });
        }
        ClientCommand::RoutineRuns {
            project_id,
            routine_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_routine_runs(&shared, &project_id, &routine_id).await;
            });
        }
        ClientCommand::RoutineDelete {
            project_id,
            routine_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .delete(format!(
                        "{}/projects/{}/routines/{}",
                        shared.http_url, project_id, routine_id
                    ))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("Routine not deleted: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Routine not deleted: {err}")),
                }
                refresh_routines(&shared, &project_id).await;
            });
        }
        ClientCommand::SkillsRefresh => {
            let shared = shared.clone();
            tokio::spawn(async move { refresh_skills(&shared).await });
        }
        ClientCommand::SkillSave { skill } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({
                    "slug": skill.slug, "name": skill.name, "description": skill.description,
                    "tags": skill.tags, "when_to_use": skill.when_to_use, "body": skill.body,
                    "shared": skill.shared, "origin_project_id": skill.origin_project_id,
                });
                let resp = if skill.id.is_empty() {
                    shared.post_json("/skills", body).await
                } else {
                    let r = shared
                        .http
                        .put(format!("{}/skills/{}", shared.http_url, skill.id))
                        .bearer_auth(shared.token())
                        .json(&body)
                        .send()
                        .await;
                    match r {
                        Ok(r) if r.status().is_success() => Ok(Value::Null),
                        Ok(r) => Err(anyhow!(
                            "PUT /skills: HTTP {} {}",
                            r.status(),
                            r.text().await.unwrap_or_default()
                        )),
                        Err(err) => Err(err.into()),
                    }
                };
                match resp {
                    Ok(_) => shared.notice(format!("Skill \"{}\" saved", skill.name)),
                    Err(err) => shared.notice(format!("Skill not saved: {err:#}")),
                }
                refresh_skills(&shared).await;
            });
        }
        ClientCommand::SkillDelete { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let r = shared
                    .http
                    .delete(format!("{}/skills/{}", shared.http_url, id))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match r {
                    Ok(r) if r.status().is_success() || r.status().as_u16() == 404 => {}
                    Ok(r) => shared.notice(format!("Delete skill: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Delete skill: {err}")),
                }
                refresh_skills(&shared).await;
            });
        }
        ClientCommand::RefreshChannels => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_channels(&shared).await;
            });
        }
        ClientCommand::ChannelPairNew {
            project_id,
            chat_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        "/channels/pair/new",
                        json!({ "projectId": project_id, "chatId": chat_id }),
                    )
                    .await
                {
                    Ok(_) => refresh_channels(&shared).await,
                    Err(err) => shared.notice(format!("Pairing code: {err:#}")),
                }
            });
        }
        ClientCommand::ChannelUnbind { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .delete(format!("{}/channels/bindings/{}", shared.http_url, id))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => shared.notice(format!("Unbind: HTTP {}", r.status())),
                    Err(err) => shared.notice(format!("Unbind: {err}")),
                }
                refresh_channels(&shared).await;
            });
        }
    }
}

/// Settings → Channels: `GET /channels` → one view (F7).
async fn refresh_channels(shared: &Arc<Shared>) {
    let v = match shared.get_json("/channels").await {
        Ok(v) => v,
        Err(err) => {
            // Older relays have no channels; stay quiet unless it is a real failure.
            if !format!("{err:#}").contains("HTTP 404") {
                shared.notice(format!("Channels: {err:#}"));
            }
            return;
        }
    };
    let list = |key: &str| -> Vec<Value> { v[key].as_array().cloned().unwrap_or_default() };
    let platforms: Vec<ChannelPlatform> = list("platforms")
        .into_iter()
        .filter_map(|p| serde_json::from_value(p).ok())
        .collect();
    let bindings: Vec<ChannelBinding> = list("bindings")
        .into_iter()
        .filter_map(|b| serde_json::from_value(b).ok())
        .collect();
    let pairing: Option<ChannelPairing> = serde_json::from_value(v["pairing"].clone()).ok();
    let _ = shared.events.send(SyncEvent::Channels(ChannelsView {
        mock: v["mock"].as_bool().unwrap_or(false),
        platforms,
        bindings,
        pairing,
        loaded: true,
    }));
}

async fn refresh_routines(shared: &Arc<Shared>, project_id: &str) {
    match shared
        .get_json(&format!("/projects/{project_id}/routines"))
        .await
    {
        Ok(v) => {
            let routines: Vec<Routine> = serde_json::from_value(v).unwrap_or_default();
            let _ = shared.events.send(SyncEvent::Routines {
                project_id: project_id.to_string(),
                routines,
            });
        }
        Err(err) => shared.notice(format!("Routines: {err:#}")),
    }
}

async fn refresh_routine_runs(shared: &Arc<Shared>, project_id: &str, routine_id: &str) {
    match shared
        .get_json(&format!(
            "/projects/{project_id}/routines/{routine_id}/runs?limit=20"
        ))
        .await
    {
        Ok(v) => {
            let runs: Vec<RoutineRun> = serde_json::from_value(v).unwrap_or_default();
            let _ = shared.events.send(SyncEvent::RoutineRuns {
                routine_id: routine_id.to_string(),
                runs,
            });
        }
        Err(err) => shared.notice(format!("Routine runs: {err:#}")),
    }
}

/// Settings → This Mac: `GET /local/devices` (F9).
async fn refresh_local(shared: &Arc<Shared>) {
    let list = match shared.get_json("/local/devices").await {
        Ok(v) => v,
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.notice(format!("This Mac: {err:#}"));
            }
            return;
        }
    };
    let devices: Vec<LocalDevice> = list["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|d| serde_json::from_value(d.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let _ = shared.events.send(SyncEvent::LocalDevices(LocalView {
        devices,
        relay_url: shared.http_url.clone(),
        loaded: true,
    }));
}

/// Settings → Connectors: `GET /connectors/catalog` + `GET /connectors` → one view (F1).
async fn refresh_connectors(shared: &Arc<Shared>) {
    let (catalog, list) = tokio::join!(
        shared.get_json("/connectors/catalog"),
        shared.get_json("/connectors")
    );
    let (catalog, list) = match (catalog, list) {
        (Ok(c), Ok(l)) => (c, l),
        (Err(err), _) | (_, Err(err)) => {
            // Older relays have no connectors; stay quiet unless it is a real failure.
            if !format!("{err:#}").contains("HTTP 404") {
                shared.notice(format!("Connectors: {err:#}"));
            }
            return;
        }
    };
    let providers: Vec<ConnectorProvider> = catalog["providers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| serde_json::from_value(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let items: Vec<Connector> = list["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| serde_json::from_value(c.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let _ = shared.events.send(SyncEvent::Connectors(ConnectorsView {
        mock: list["mock"].as_bool().unwrap_or(false),
        providers,
        items,
        loaded: true,
    }));
}

/// The relay's `error.message` out of an HTTP error, else the whole chain.
fn relay_message(err: &anyhow::Error) -> String {
    let text = format!("{err:#}");
    if let Some(i) = text.find('{') {
        if let Ok(v) = serde_json::from_str::<Value>(&text[i..]) {
            if let Some(m) = v["error"]["message"].as_str() {
                return m.to_string();
            }
        }
    }
    text
}

/// Settings → Secrets: `GET /secrets` + `GET /secrets/requests` → one view (D83). Never values.
async fn refresh_secrets(shared: &Arc<Shared>) {
    let (overview, requests) = tokio::join!(
        shared.get_json("/secrets"),
        shared.get_json("/secrets/requests")
    );
    let (overview, requests) = match (overview, requests) {
        (Ok(o), Ok(r)) => (o, r),
        (Err(err), _) | (_, Err(err)) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.notice(format!("Secrets: {}", relay_message(&err)));
            }
            return;
        }
    };
    let list =
        |v: &Value, key: &str| -> Vec<Value> { v[key].as_array().cloned().unwrap_or_default() };
    let secrets: Vec<Secret> = list(&overview, "secrets")
        .iter()
        .filter_map(|i| serde_json::from_value(i.clone()).ok())
        .collect();
    let vaults = list(&overview, "vaults")
        .iter()
        .filter_map(|i| serde_json::from_value(i.clone()).ok())
        .collect();
    let activity: Vec<SecretActivity> = list(&overview, "activity")
        .iter()
        .filter_map(|i| serde_json::from_value(i.clone()).ok())
        .collect();
    let requests: Vec<SecretRequest> = list(&requests, "items")
        .iter()
        .filter_map(|r| serde_json::from_value(r.clone()).ok())
        .collect();
    let providers = list(&overview, "providers")
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    let _ = shared.events.send(SyncEvent::Secrets(SecretsView {
        paused: overview["paused"].as_bool().unwrap_or(false),
        secrets,
        vaults,
        providers,
        requests,
        activity,
        loaded: true,
    }));
}

/// One agent's Secrets page (`GET /projects/{id}/secrets`).
async fn refresh_project_secrets(shared: &Arc<Shared>, project_id: &str) {
    match shared
        .get_json(&format!("/projects/{project_id}/secrets"))
        .await
    {
        Ok(v) => {
            if let Ok(view) = serde_json::from_value::<ProjectSecrets>(v) {
                let _ = shared.events.send(SyncEvent::ProjectSecrets {
                    project_id: project_id.to_string(),
                    view,
                });
            }
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.notice(format!("Secrets: {}", relay_message(&err)));
            }
        }
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "arbos-mac".into())
}

impl Drop for Room {
    fn drop(&mut self) {
        self.task.abort();
    }
}
