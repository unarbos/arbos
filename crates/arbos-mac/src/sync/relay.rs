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
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use arbos_client::room::{self, RoomAction, RoomSession};
use arbos_core::api::{
    ApiErrorDto, CallDto, CallEnvelopeDto, ChannelBindingDto, ChannelPairingDto,
    ChannelPlatformDto, ChatDto, ConnectConnectorReplyDto, ConnectorsCatalogDto, ConnectorsListDto,
    CreateChatResponseDto, DecideSecretResponseDto, GetProjectResponseDto, LeaveCallResponseDto,
    LocalDevicesDto, MachineStateDto, MachineStatusDto, MeResponseDto, MeUserResponseDto,
    ModelListDto, PairResponseDto, PeopleChatDto, ProjectMembersDto, ProjectSecretsDto,
    PushEventDto, PushItemDto, RoutineDto, RoutineRunDto, SecretEnvelopeDto, SecretRequestsDto,
    SecretsOverviewDto, SkillDto, StartCallResponseDto, TokenResponseDto, TranscriptionDto,
    UnreadDto, UserCardDto, UsersPageDto, VaultEnvelopeDto, VoiceCatalogDto,
};
use arbos_core::doc::{UserAttachment, UserMessageOpts};
use arbos_core::wire::{BoxState as WireBox, PROJECT_DOC_CHAT_ID};
use arbos_matrix::{
    Error as MatrixError, Listener as MatrixListener, MatrixClient, MessageRow, Session, Snapshot,
};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::avatar_master::Master;
use super::credentials::{self, DeviceCredentials};
use super::diagnostics;
use super::doc::ChatDoc;
use super::dto::{
    account_view_from_me, apply_machine_status, chat_from_create, chat_from_dto,
    connectors_view_from_dto, credentials_from_token, local_view_from_dto, merge_chats,
    merged_unread, model_ids_from_dto, people_space_from, project_from_dto, relay_unread_for,
    remap_queued_send_targets, routine_from_dto, secrets_view_from_dto, skill_from_dto,
    transcript_text, voices_from_dto,
};
use super::store::{self, Store};
use super::user_error::{ErrorClass, Fault, UserError};
use super::{ClientCommand, DraftPicture, PictureSource, PictureTarget, SyncEvent, SyncSource};
use crate::model::{
    new_id, now_ms, trace_rows, AccountView, Attachment, AttachmentKind, BoxReport, BoxState, Call,
    CallEnded, CallLeft, CallMissed, ChannelBinding, ChannelPairing, ChannelPlatform, ChannelsView,
    Chat, Effort, Id, Link, LinkState, Machine, MachineState, Message, Part, Person, Project,
    ProjectDocView, ProjectSecrets, Reachable, ReplyRef, Role, Routine, RoutineRun, Sender,
    ServerInfo, ThreadInfo,
};
use crate::update;

const HELLO_DEADLINE: Duration = room::HELLO_DEADLINE;
const PRESENCE_EVERY: Duration = room::PRESENCE_EVERY;
const REGISTRY_POLL: Duration = Duration::from_secs(15);
/// `GET /machine` is the source of truth for the rail indicator and Settings.
const MACHINE_POLL: Duration = Duration::from_secs(10);

/// Typed relay HTTP failure: `ApiErrorDto` when the body is the current shape, else plain text.
#[derive(Debug, Clone)]
pub(crate) struct RelayError {
    pub status: u16,
    pub kind: String,
    pub message: String,
    /// `error.detail` when the relay sends one (the avatar routes: the provider's words).
    pub detail: Option<String>,
}

impl RelayError {
    pub(crate) fn from_body(status: reqwest::StatusCode, body: &str) -> Self {
        if let Ok(dto) = serde_json::from_str::<ApiErrorDto>(body) {
            let detail = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/detail")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|d| !d.is_empty());
            Self {
                status: status.as_u16(),
                kind: dto.error.type_,
                message: dto.error.message,
                detail,
            }
        } else {
            Self {
                status: status.as_u16(),
                kind: String::new(),
                message: body.trim().to_owned(),
                detail: None,
            }
        }
    }

    pub(crate) fn is_token_expired(&self) -> bool {
        self.status == 401 && self.kind == "token_expired"
    }
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            write!(f, "HTTP {}", self.status)
        } else {
            write!(f, "HTTP {} {}", self.status, self.message)
        }
    }
}

impl std::error::Error for RelayError {}

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

/// A send that arrived while `NewChat` for its local chat id is still in flight.
struct QueuedSend {
    message_id: Id,
    text: String,
    attachments: Vec<Outgoing>,
    voice_wav: Option<Arc<Vec<u8>>>,
    command: &'static str,
    answer: Option<(String, serde_json::Value)>,
    reply_to: Option<String>,
    forwarded_from: Option<String>,
    reply_ref: Option<ReplyRef>,
    mentions: Vec<String>,
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
    /// Rooms whose pushes were refused with `not_available` on the live connection.
    /// Cleared by the next ACK for that room (not by draining `unacked`, which is global).
    push_refused: Mutex<HashSet<Id>>,
    events: mpsc::UnboundedSender<SyncEvent>,
    subscriptions: Mutex<Vec<loro::Subscription>>,
    machine: Mutex<Machine>,
    refresh_lock: tokio::sync::Mutex<()>,
    /// People chat ids (rooms live under `/room/people/{id}`).
    people: Mutex<HashSet<Id>>,
    /// Highest relay seq seen per room key (read marks, D61).
    room_seq: Mutex<HashMap<Id, i64>>,
    /// When this device last sent `PUT /chats/{id}/read` (ms). Used so a
    /// registry refresh does not resurrect unread before the relay catches up.
    local_read_at: Mutex<HashMap<Id, i64>>,
    /// When this Mac last archived or renamed a chat (ms). A refresh that
    /// started earlier must not overwrite that field with the relay's older value.
    local_changed_at: Mutex<HashMap<Id, u64>>,
    /// Local chat ids whose `POST .../chats` is still in flight, and any sends
    /// that arrived before the room doc existed.
    pending_new_chats: Mutex<HashMap<Id, Vec<QueuedSend>>>,
    /// Avatars already fetched (by URL).
    avatars_done: Mutex<HashSet<String>>,
    /// `/me` user when the stored credentials carry none (env token, legacy adv_).
    me: Mutex<Option<credentials::AccountUser>>,
    /// The chat on screen in a focused window; presence frames carry it as
    /// `focused_chat` so the relay does not push this device for it (§7c).
    focused_chat: Mutex<Option<Id>>,
    /// Settings → Notifications "Read receipts": when true, people-chat presence
    /// frames carry `seen: <newest message id>` (API chat settings / §7c).
    read_receipts: AtomicBool,
    /// Calls v1: `in_call` on every presence frame while this device is in one (§8).
    in_call: Mutex<Option<String>>,
    /// On-device store (#929); `None` without credentials or with `ARBOS_NO_STORE=1`.
    store: Mutex<Option<Store>>,
    /// Room keys whose doc or cursor changed since the last save.
    dirty: Mutex<HashSet<Id>>,
    /// Silent error reports: one per signature per week (see `diagnostics.rs`).
    diagnostics: diagnostics::Diagnostics,
    /// D88: the homeserver client for servers and channels; `None` until `GET /me/matrix`
    /// answered (and when the relay runs without servers).
    matrix: Mutex<Option<Arc<MatrixClient>>>,
    /// When the Matrix session was last (re)started: a rejected token restarts it at most
    /// once a minute.
    matrix_started_at: Mutex<Option<Instant>>,
}

/// Blob slot prefix of a people chat (`people:<chat_id>`, people-chats API §2).
const PEOPLE_BLOB_PREFIX: &str = "people:";

fn project_doc_key(project_id: &str) -> String {
    format!("proj:{project_id}")
}

impl Shared {
    /// `{device, at, focused_chat?, seen?}` — every presence frame (15 s tick,
    /// typing, focus change) says which chat this device is looking at, if any.
    /// When read receipts are on and the focused chat is a people chat, `seen`
    /// is the newest message id in that chat (read-receipt Presence).
    fn presence_header(&self) -> Value {
        let mut header = json!({
            "device": self.device_id(),
            "at": now_ms(),
            "user": self.me_id(),
            "handle": self.me_handle(),
        });
        let focused = self.focused_chat.lock().unwrap().clone();
        if let Some(chat) = focused.as_deref() {
            header["focused_chat"] = json!(chat);
            if self.read_receipts.load(Ordering::Relaxed) && self.is_people(chat) {
                if let Some(seen) = self.newest_message_id(chat) {
                    header["seen"] = json!(seen);
                }
            }
        }
        if let Some(call) = self.in_call.lock().unwrap().as_deref() {
            header["in_call"] = json!(call);
        }
        header
    }

    /// Newest message id in a chat doc, if the doc is open and non-empty.
    fn newest_message_id(&self, chat_id: &str) -> Option<String> {
        let docs = self.docs.lock().unwrap();
        let doc = docs.get(chat_id)?;
        doc.read_all().last().map(|m| m.id.clone())
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
            return Err(RelayError::from_body(status, &text).into());
        }
        if !status.is_success() {
            return Err(RelayError::from_body(status, &text).into());
        }
        let v: Value = serde_json::from_str(&text).context("refresh: bad JSON")?;
        self.apply_token_response(&v)?;
        Ok(())
    }

    /// Store the body of `/auth/token` or `/auth/refresh`.
    fn apply_token_response(&self, v: &Value) -> Result<DeviceCredentials> {
        let dto: TokenResponseDto =
            serde_json::from_value(v.clone()).context("token response: bad JSON")?;
        if dto.access_token.is_empty() {
            return Err(anyhow!("relay returned no access token"));
        }
        let previous = self.creds.read().unwrap().clone();
        let creds = credentials_from_token(&dto, &self.http_url, previous.as_ref(), now_ms());
        if let Err(err) = credentials::store(&creds) {
            self.fail("save your sign-in to the keychain", &err);
        }
        *self.creds.write().unwrap() = Some(creds.clone());
        Ok(creds)
    }

    /// The newest unreported panic of a previous run (`panic_log`), once, through the
    /// silent-diagnostic feedback path — so a crash that only left a `.ips` behind on the
    /// Mac still reaches the relay with its message and backtrace.
    fn report_last_panic(self: &Arc<Self>) {
        if let Some(entry) = crate::panic_log::take_unreported() {
            self.diagnostic_always("panic", entry);
        }
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
            if let Ok(dto) = serde_json::from_value::<MeResponseDto>(v) {
                *self.me.lock().unwrap() = Some((&dto.user).into());
            }
        }
    }

    fn is_people(&self, chat_id: &str) -> bool {
        if self.people.lock().unwrap().contains(chat_id) {
            return true;
        }
        // Cold start can restore people spaces from KEY_PROJECTS before KEY_PEOPLE
        // is populated (or when that key is empty). Project kind is the source of
        // truth for Dm/Group — without this, room_loop dials `{chat}/{chat}` instead
        // of `people/{chat}` and never joins the room iOS uses.
        self.projects
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.id == chat_id && p.is_people())
    }

    /// The `/blob/{slot}/{key}` slot for a **project id** or a **people chat id**.
    /// People chats keep their blobs under `people:<chat_id>` (people-chats API §2).
    /// Project chats use the project id — pass that, not the chat id (a chat id is not a
    /// project the relay knows, so PUT/GET would 403). Every blob read and write goes
    /// through here so uploads and fetches always agree.
    fn blob_slot(&self, id: &str) -> String {
        if id.starts_with(PEOPLE_BLOB_PREFIX) || !self.is_people(id) {
            id.to_owned()
        } else {
            format!("{PEOPLE_BLOB_PREFIX}{id}")
        }
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
            return Err(RelayError::from_body(status, &text).into());
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
            return Err(RelayError::from_body(status, &text).into());
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Stop the homeserver session (sign-out, revoked token, restart with a fresh token).
    fn stop_matrix(&self) {
        if let Some(m) = self.matrix.lock().unwrap().take() {
            m.stop();
        }
    }

    /// Forget the tokens and show the sign-in screen.
    fn sign_out_locally(&self) {
        credentials::clear(&self.http_url);
        self.stop_matrix();
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

    fn device_id(&self) -> String {
        self.creds
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.device_id.clone())
            .unwrap_or_else(|| "mac:unpaired".into())
    }

    /// A short toast for something that worked or a plain status ("Forwarded to Studio",
    /// "Token replaced."). Never a failure: that is [`Self::fail`]. Never for background work —
    /// that is [`Self::diagnostic`].
    fn notice(&self, text: impl Into<String>) {
        let _ = self.events.send(SyncEvent::Notice(text.into()));
    }

    /// Something the user asked for did not happen: `verb` is what they tried ("save the
    /// secret"), `fault` the error / status / words. Classified by `user_error` and drawn by
    /// `ui::error_note` (an error toast, or the surface that owns the action). Never a raw
    /// error string — `error_ui_lint` pins it.
    fn fail<'a>(&self, verb: &str, fault: impl Into<Fault<'a>>) {
        self.fail_with(fault.into().classify(verb));
    }

    /// A failure already classified by the caller.
    fn fail_with(&self, error: UserError) {
        let _ = self.events.send(SyncEvent::Failure { error, retry: None });
    }

    /// An app error the user did not cause and cannot act on (a thumbnail that did not load, a
    /// list that did not refresh). Logged, and reported once per signature per week as a silent
    /// feedback item (`submitted_by: "app"`); nothing is shown in the chat. Transient network
    /// blips (timeout, reset, DNS, TLS decrypt, empty/truncated body) only log — they are not
    /// product bugs.
    fn diagnostic(self: &Arc<Self>, kind: &'static str, detail: impl Into<String>) {
        let detail = detail.into();
        if diagnostics::is_transient(&detail) {
            log::warn!(target: "arbos_mac::diagnostic", "{kind}: {detail}");
            return;
        }
        self.diagnostic_always(kind, detail);
    }

    /// `diagnostic` without the transient filter: for failures of an explicit user action, where
    /// a timeout is exactly what we want to hear about (once a week per signature as usual).
    fn diagnostic_always(self: &Arc<Self>, kind: &'static str, detail: impl Into<String>) {
        let detail = detail.into();
        log::warn!(target: "arbos_mac::diagnostic", "{kind}: {detail}");
        let signature = diagnostics::signature(kind, &detail);
        if !self.diagnostics.should_report(&signature) {
            return;
        }
        if self.creds.read().unwrap().is_none() {
            return;
        }
        let shared = self.clone();
        tokio::spawn(async move {
            let note = format!(
                "Automatic report from the Mac app (no user action): {kind} failed — {detail}. \
                 Signature `{signature}`; the same error is reported once a week, not per launch."
            );
            let body = json!({
                "note": note,
                "platform": "mac",
                "app_version": update::current_version(),
                "os": update::os_version(),
                "submitted_by": "app",
                "signature": signature,
                "context": {
                    "kind": kind,
                    "detail": detail,
                    "signature": signature,
                    "relay_url": shared.http_url,
                },
            });
            // Failures here only log: a diagnostic about diagnostics would loop.
            match shared
                .http
                .post(format!("{}/feedback", shared.http_url))
                .bearer_auth(shared.token())
                .json(&body)
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => shared.diagnostics.sent(&signature),
                Ok(r) => log::warn!(
                    target: "arbos_mac::diagnostic",
                    "report not taken: HTTP {}",
                    r.status()
                ),
                Err(err) => log::warn!(target: "arbos_mac::diagnostic", "report not sent: {err}"),
            }
        });
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
            if !status.is_success() {
                let err = RelayError::from_body(status, &body);
                if err.is_token_expired() && !refreshed {
                    refreshed = true;
                    self.refresh_tokens(Some(&token)).await?;
                    continue;
                }
                return Err(err.into());
            }
            // A 200 with no bytes is a cut connection / proxy blip, not valid JSON — surface it
            // so people-chats can retry and diagnostics treat it as transient.
            if body.trim().is_empty() {
                return Err(anyhow!("GET {path}: empty body"));
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
            if !status.is_success() {
                let err = RelayError::from_body(status, &text);
                if err.is_token_expired() && !refreshed {
                    refreshed = true;
                    self.refresh_tokens(Some(&token)).await?;
                    continue;
                }
                return Err(err.into());
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
        self.post_json_timeout(path, body, None).await
    }

    /// `post_json` with its own request timeout (the client's default is 30 s; an avatar
    /// generation is given 90 s so the relay's 75 s budget always answers first).
    async fn post_json_timeout(
        self: &Arc<Self>,
        path: &str,
        body: Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let mut refreshed = false;
        loop {
            let token = self.token();
            let mut req = self
                .http
                .post(format!("{}{}", self.http_url, path))
                .bearer_auth(&token)
                .json(&body);
            if let Some(t) = timeout {
                req = req.timeout(t);
            }
            let resp = req.send().await.with_context(|| format!("POST {path}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                let err = RelayError::from_body(status, &text);
                if err.is_token_expired() && !refreshed {
                    refreshed = true;
                    self.refresh_tokens(Some(&token)).await?;
                    continue;
                }
                return Err(err.into());
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
            if !status.is_success() {
                let err = RelayError::from_body(status, &text);
                if err.is_token_expired() && !refreshed {
                    refreshed = true;
                    self.refresh_tokens(Some(&token)).await?;
                    continue;
                }
                return Err(err.into());
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
                    // Every request says what runs here (`mac/0.6.30`, `mac/0.6.29 (1084)`):
                    // the relay's "installed version" signal for continuous delivery.
                    .default_headers({
                        let mut h = reqwest::header::HeaderMap::new();
                        if let Ok(v) = update::client_header().parse() {
                            h.insert("x-arbos-client", v);
                        }
                        h
                    })
                    .build()
                    .context("http client")?,
                http_url: self.http_url.clone(),
                ws_url: self.ws_url.clone(),
                creds: RwLock::new(credentials::load(&self.http_url)),
                docs: Mutex::new(HashMap::new()),
                rooms: Mutex::new(HashMap::new()),
                focused_chat: Mutex::new(None),
                read_receipts: AtomicBool::new(true),
                in_call: Mutex::new(None),
                projects: Mutex::new(Vec::new()),
                chats: Mutex::new(Vec::new()),
                unacked: Mutex::new(HashSet::new()),
                push_refused: Mutex::new(HashSet::new()),
                events: events.clone(),
                subscriptions: Mutex::new(Vec::new()),
                machine: Mutex::new(Machine::default()),
                refresh_lock: tokio::sync::Mutex::new(()),
                people: Mutex::new(HashSet::new()),
                room_seq: Mutex::new(HashMap::new()),
                local_read_at: Mutex::new(HashMap::new()),
                local_changed_at: Mutex::new(HashMap::new()),
                pending_new_chats: Mutex::new(HashMap::new()),
                avatars_done: Mutex::new(HashSet::new()),
                me: Mutex::new(None),
                store: Mutex::new(None),
                dirty: Mutex::new(HashSet::new()),
                diagnostics: diagnostics::Diagnostics::load(),
                matrix: Mutex::new(None),
                matrix_started_at: Mutex::new(None),
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

/// `GET /skills` → `SyncEvent::Skills` (F5). Decode rows one at a time so a bad
/// skill does not drop the rest of the list.
async fn refresh_skills(shared: &Arc<Shared>) {
    match shared.get_json("/skills").await {
        Ok(v) => {
            let mut skills = Vec::new();
            for item in v.as_array().cloned().unwrap_or_default() {
                match serde_json::from_value::<SkillDto>(item) {
                    Ok(d) => skills.push(skill_from_dto(&d)),
                    Err(err) => {
                        log::warn!(target: "arbos_mac::relay", "SkillDto: {err}");
                    }
                }
            }
            let _ = shared.events.send(SyncEvent::Skills(skills));
        }
        Err(err) => shared.diagnostic("skills", format!("{err:#}")),
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
    let Ok(dto) = serde_json::from_value::<MeResponseDto>(v) else {
        return;
    };
    let u: credentials::AccountUser = (&dto.user).into();
    shared.with_store(|s| s.put_json(store::KEY_ACCOUNT, &u));
    *shared.me.lock().unwrap() = Some(u);
    let view = account_view_from_me(&dto);
    let _ = shared.events.send(SyncEvent::Account(Some(view)));
    // D92 remote flags: leaf avatars for agents, and their Lottie playback (absent = on).
    crate::ui::avatars::set_leaf_avatars(dto.features.leaf_avatars.unwrap_or(true));
    crate::ui::avatars::set_leaf_lottie(dto.features.leaf_lottie.unwrap_or(true));
    // Voice mode is a relay-side kill switch; absent = off.
    let _ = shared.events.send(SyncEvent::Features {
        voice_mode: dto.features.voice_mode.unwrap_or(false),
    });
    let m = &dto.machine;
    // D89: provider and waitlist place ride along with `always_on`.
    let position = m
        .waitlisted
        .then(|| m.waitlist_position.map(|p| p as u32).unwrap_or(1));
    {
        let snapshot = {
            let mut machine = shared.machine.lock().unwrap();
            machine.always_on = m.always_on;
            if let Some(p) = m.provider.clone() {
                machine.provider = Some(p);
            }
            machine.waitlist_position = position;
            machine.clone()
        };
        let _ = shared.events.send(SyncEvent::Machine(snapshot));
    }
    if m.exists {
        let word = match m.state {
            WireBox::Awake => "awake",
            WireBox::Waking => "waking",
            WireBox::Asleep => "asleep",
            WireBox::Error => "error",
        };
        let since = (m.since > 0).then_some(m.since as u64);
        set_machine(shared, machine_state_from_word(word), since, None);
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
                    shared.diagnostic("session-refresh", format!("{err:#}"));
                }
            }
            if shared.creds.read().unwrap().is_none() {
                return;
            }
        }
    });
}

fn start_after_pairing(shared: &Arc<Shared>) {
    shared.report_last_panic();
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
            if let Ok(dto) = serde_json::from_value::<ModelListDto>(v) {
                let ids = model_ids_from_dto(&dto);
                if !ids.is_empty() {
                    let _ = models_shared.events.send(SyncEvent::Models(ids));
                }
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
        tokio::spawn(async move { start_matrix(&shared).await });
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
            match refresh_registry(&shared).await {
                Ok(()) => {
                    let _ = shared.events.send(SyncEvent::RelayLink(LinkState::Live));
                }
                Err(err) => {
                    let text = format!("{err:#}");
                    if text.contains("HTTP 401") {
                        // Token revoked or relay re-initialised: pair again.
                        shared.stop_matrix();
                        shared.drop_store(true);
                        *shared.creds.write().unwrap() = None;
                        let _ = shared.events.send(SyncEvent::NeedsPairing {
                            relay_url: shared.http_url.clone(),
                        });
                        return;
                    }
                    // No toast: the header pill is the one place this shows
                    // ([feedback connection-pill]). Only a verdict about
                    // reachability goes up; an odd error (bad JSON, a 404)
                    // says nothing about the link.
                    eprintln!("relay registry: {text}");
                    let _ = shared.events.send(SyncEvent::Link(Link::Reconnecting));
                    let fault = link_fault(&err);
                    if matches!(fault, LinkState::Offline | LinkState::RelayDown) {
                        let _ = shared.events.send(SyncEvent::RelayLink(fault));
                    }
                }
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
    // Seed the people-id set from both KEY_PEOPLE and project kinds *before*
    // ensure_room / room_loop. room_loop used to re-check is_people on every
    // dial; after RoomSession it bakes the path once — an empty HashSet on
    // cold start left Mac on the wrong room forever (iOS always dials people/).
    {
        let mut ids = shared.people.lock().unwrap();
        ids.extend(people);
        for p in &projects {
            if p.is_people() {
                ids.insert(p.id.clone());
            }
        }
    }
    *shared.projects.lock().unwrap() = projects.clone();
    *shared.chats.lock().unwrap() = chats;
    // Pictures the last run fetched paint with the first content: no leaf placeholder on
    // every agent while the fetches are out, and nothing to fetch for an unchanged picture.
    for url in picture_urls(&projects) {
        if let Some(bytes) = cached_avatar(shared, &url) {
            shared.avatars_done.lock().unwrap().insert(url.clone());
            let _ = shared.events.send(SyncEvent::Avatar {
                url,
                bytes: Arc::new(bytes),
            });
        }
    }
    let _ = shared.events.send(SyncEvent::Projects(projects.clone()));
    log_first_paint("store", projects.len());
    emit_chats(shared);
    let mut pending = pending;
    let mut restored = 0usize;
    for saved in docs {
        let doc = ChatDoc::empty(&saved.key);
        if let Err(err) = doc.core.doc.import(&saved.snapshot) {
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
    let wanted: HashSet<String> = picture_urls(&projects).into_iter().collect();
    shared.with_store(|s| {
        if projects.is_empty() {
            s.remove(store::KEY_PROJECTS);
            s.remove(store::KEY_CHATS);
        } else {
            s.put_json(store::KEY_PROJECTS, &projects);
            s.put_json(store::KEY_CHATS, &chats);
        }
        s.put_json(store::KEY_PEOPLE, &people);
        // A picture no current URL names (a changed avatar's old version, a left group) goes.
        for key in s.keys_with_prefix(store::KEY_AVATAR_PREFIX) {
            if !wanted.contains(&key[store::KEY_AVATAR_PREFIX.len()..]) {
                s.remove(&key);
            }
        }
    });
}

/// Every picture URL the registry draws: agents' pictures (my records and roster copies),
/// groups' pictures, people's avatars.
pub(crate) fn picture_urls(projects: &[Project]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in projects {
        out.extend(p.image_url.clone());
        out.extend(p.agents.iter().filter_map(|a| a.image_url.clone()));
        out.extend(
            p.people
                .iter()
                .map(|x| x.avatar_url.clone())
                .filter(|u| !u.is_empty()),
        );
    }
    out.sort();
    out.dedup();
    out
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
                    if doc.core.doc.oplog_frontiers().is_empty() {
                        continue;
                    }
                    doc.core.doc.export(loro::ExportMode::Snapshot)
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

fn effort_to(e: Effort) -> &'static str {
    match e {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
    }
}

/// `GET /me/pushes`: people_* kinds that change the chat list refresh it.
async fn poll_me_pushes(shared: &Arc<Shared>) {
    let Ok(list) = shared.get_json("/me/pushes").await else {
        return;
    };
    let mut refresh = false;
    let mut refresh_servers = false;
    for push in list.as_array().cloned().unwrap_or_default() {
        let item: PushItemDto = match serde_json::from_value(push.clone()) {
            Ok(item) => item,
            Err(err) => {
                let kind = push.get("kind").and_then(Value::as_str).unwrap_or("?");
                eprintln!("relay: skip unknown push kind={kind}: {err}");
                continue;
            }
        };
        match &item.event {
            PushEventDto::PeopleChatCreated(_)
            | PushEventDto::PeopleChatUpdated(_)
            | PushEventDto::PeopleMemberAdded(_)
            | PushEventDto::PeopleMemberRemoved(_)
            | PushEventDto::PeopleAgentAdded(_)
            | PushEventDto::PeopleAgentRemoved(_)
            | PushEventDto::PeopleProjectShared(_) => refresh = true,
            // A people chat we do not list yet: refresh. An agent's reply in a project chat
            // (`project_id` set) is already on the doc socket; nothing to fetch.
            PushEventDto::PeopleMessage(payload)
                if payload.project_id.is_none() && !shared.is_people(&payload.chat_id) =>
            {
                refresh = true
            }
            PushEventDto::CallStarted(payload) => {
                let call = Call::from(&payload.call);
                let sender = Sender::from(&payload.sender);
                if !call.call_id.is_empty() {
                    let _ = shared.events.send(SyncEvent::CallStarted { call, sender });
                }
            }
            PushEventDto::SpaceArchived(payload) => {
                let at = payload.archived_at.filter(|t| *t > 0).map(|t| t as u64);
                if payload.kind != "server" {
                    let mut projects = shared.projects.lock().unwrap();
                    if let Some(p) = projects.iter_mut().find(|p| p.id == payload.space_id) {
                        p.archived_at = at;
                    }
                }
                let _ = shared.events.send(SyncEvent::SpaceArchived {
                    space_id: payload.space_id.clone(),
                    archived_at: at,
                });
            }
            PushEventDto::ChatArchived(payload) => {
                let chat_id = payload.chat_id.clone();
                if chat_id.is_empty() {
                    continue;
                }
                let archived_at = payload.archived_at.filter(|t| *t > 0).map(|t| t as u64);
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
            PushEventDto::ReleasePublished(payload) => {
                if !payload.platform.is_empty() {
                    let _ = shared.events.send(SyncEvent::ReleasePublished {
                        platform: payload.platform.clone(),
                        version: payload.version.clone(),
                    });
                }
            }
            // I read this chat on another device: not unread here either (the relay holds the
            // one cursor), and the app drops its banners for it.
            PushEventDto::NotificationsCleared(payload) => {
                let chat_id = payload.chat_id.clone();
                if chat_id.is_empty() {
                    continue;
                }
                if let Some(c) = shared
                    .chats
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|c| c.id == chat_id)
                {
                    c.unread = false;
                    c.unread_count = 0;
                }
                shared
                    .local_read_at
                    .lock()
                    .unwrap()
                    .insert(chat_id.clone(), now_ms() as i64);
                let _ = shared.events.send(SyncEvent::NotificationsCleared {
                    chat_id,
                    badge: payload.badge.max(0) as u64,
                });
            }
            PushEventDto::FeedbackCapture(payload) => {
                if !payload.request_id.is_empty() {
                    let _ = shared.events.send(SyncEvent::FeedbackCapture {
                        request_id: payload.request_id.clone(),
                        expires_at: (payload.expires_at > 0).then_some(payload.expires_at),
                    });
                }
            }
            PushEventDto::CallMissed(payload) if !payload.call_id.is_empty() => {
                let missed = CallMissed {
                    call_id: payload.call_id.clone(),
                    chat_id: payload.chat_id.clone(),
                    project_id: payload.project_id.clone(),
                    kind: match payload.kind.as_str() {
                        "video" => crate::model::CallKind::Video,
                        _ => crate::model::CallKind::Voice,
                    },
                    from: Sender::from(&payload.from),
                    started_at: payload.started_at.max(0) as u64,
                    ended_at: (payload.ended_at > 0).then_some(payload.ended_at as u64),
                    duration_s: payload.duration_s.max(0) as u64,
                    reason: payload.reason.clone().unwrap_or_default(),
                };
                let _ = shared.events.send(SyncEvent::CallMissed(missed));
            }
            PushEventDto::CallEnded(payload) => {
                let ended = CallEnded {
                    call_id: payload.call_id.clone(),
                    chat_id: payload.chat_id.clone(),
                    project_id: payload.project_id.clone(),
                    duration_s: payload.duration_s.max(0) as u64,
                    reason: payload.reason.clone(),
                    writer_device: payload.writer_device.clone(),
                    writer_is_me: payload.writer_device.as_deref()
                        == Some(shared.device_id().as_str()),
                };
                let _ = shared.events.send(SyncEvent::CallEnded(ended));
            }
            PushEventDto::ServerUpdated(_)
            | PushEventDto::ServerMemberJoined(_)
            | PushEventDto::ServerMemberRemoved(_)
            | PushEventDto::ServerDeleted(_) => refresh_servers = true,
            // Someone's profile picture changed: mine (another device set it → re-read /me)
            // or a person I share a chat or server with (their card comes with the lists).
            PushEventDto::ProfileUpdated(payload) => {
                if payload.user_id == shared.me_id() {
                    refresh_me(shared).await;
                }
                refresh = true;
                refresh_servers = true;
            }
            // An agent's avatar changed (the agents' `profile_updated`): the new picture and
            // stamp land in my record and every roster copy now; the re-read brings the rest.
            PushEventDto::AgentProfileUpdated(payload) => {
                let mut projects = shared.projects.lock().unwrap();
                super::agent_avatar::apply_push(
                    &mut projects,
                    &shared.http_url,
                    &payload.project_id,
                    payload.image_key.as_deref(),
                    payload.avatar_version,
                );
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
                refresh = true;
            }
            PushEventDto::PeopleMessage(_)
            | PushEventDto::PeopleChatDeleted(_)
            | PushEventDto::ProjectMemberRemoved(_)
            | PushEventDto::PeopleInviteAccepted(_)
            | PushEventDto::MachineReady(_)
            | PushEventDto::SecretRequest(_)
            | PushEventDto::SecretsPaused(_)
            | PushEventDto::SecretExpiring(_)
            | PushEventDto::MessageRedacted(_)
            | PushEventDto::CallMissed(_) => {}
        }
    }
    if refresh {
        if let Err(err) = refresh_registry(shared).await {
            shared.diagnostic("refresh", format!("{err:#}"));
        }
    }
    if refresh_servers {
        refresh_servers_list(shared).await;
    }
}

// ---------------------------------------------------------------------------
// Servers and channels (D88 C1)
// ---------------------------------------------------------------------------

/// Forwards what the homeserver client learns to the UI as sync events. Handlers run on the
/// client's sync task, so they only post; a rejected token asks for a fresh session.
struct MatrixBridge {
    events: mpsc::UnboundedSender<SyncEvent>,
    shared: std::sync::Weak<Shared>,
}

impl MatrixListener for MatrixBridge {
    fn on_rooms(&self, snapshot: Snapshot) {
        let _ = self.events.send(SyncEvent::MatrixRooms(snapshot));
    }

    fn on_timeline(&self, room_id: String, rows: Vec<MessageRow>) {
        let _ = self
            .events
            .send(SyncEvent::MatrixTimeline { room_id, rows });
    }

    fn on_connection(&self, connected: bool, error: Option<String>) {
        let rejected = error
            .as_deref()
            .is_some_and(|e| e.contains("M_UNKNOWN_TOKEN") || e.contains("401"));
        let _ = self
            .events
            .send(SyncEvent::MatrixConnection { connected, error });
        if !rejected {
            return;
        }
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let due = shared
            .matrix_started_at
            .lock()
            .unwrap()
            .is_none_or(|t| t.elapsed() > Duration::from_secs(60));
        if due && shared.creds.read().unwrap().is_some() {
            tokio::spawn(async move {
                shared.stop_matrix();
                start_matrix(&shared).await;
            });
        }
    }
}

/// `GET /me/matrix` → start the homeserver client; 503 `servers_off` means this relay has no
/// servers (no rows, no menu items). Then the first `GET /servers`.
async fn start_matrix(shared: &Arc<Shared>) {
    *shared.matrix_started_at.lock().unwrap() = Some(Instant::now());
    let session = match shared.get_json("/me/matrix").await {
        Ok(v) => match serde_json::from_value::<Session>(v) {
            Ok(s) => s,
            Err(err) => {
                shared.diagnostic("matrix-session", format!("bad /me/matrix: {err}"));
                return;
            }
        },
        Err(err) => {
            let text = format!("{err:#}");
            if text.contains("servers_off") || text.contains("HTTP 503") {
                let _ = shared.events.send(SyncEvent::ServersAvailable(false));
            } else if text.contains("HTTP 404") {
                // Older relay without the route.
                let _ = shared.events.send(SyncEvent::ServersAvailable(false));
            } else {
                shared.diagnostic("matrix-session", text);
            }
            return;
        }
    };
    let bridge = Arc::new(MatrixBridge {
        events: shared.events.clone(),
        shared: Arc::downgrade(shared),
    });
    match MatrixClient::start(session, bridge).await {
        Ok(client) => {
            let previous = shared.matrix.lock().unwrap().replace(client);
            if let Some(p) = previous {
                p.stop();
            }
            let _ = shared.events.send(SyncEvent::ServersAvailable(true));
        }
        Err(err) => shared.diagnostic("matrix-start", err.to_string()),
    }
    refresh_servers_list(shared).await;
}

/// `GET /servers` → `SyncEvent::Servers`.
async fn refresh_servers_list(shared: &Arc<Shared>) {
    match shared.get_json("/servers").await {
        Ok(v) => {
            let servers: Vec<ServerInfo> = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| serde_json::from_value::<ServerInfo>(s.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            let servers = servers
                .into_iter()
                .map(|s| with_server_picture(shared, s))
                .collect();
            let _ = shared.events.send(SyncEvent::Servers(servers));
        }
        Err(err) => {
            let text = format!("{err:#}");
            if text.contains("servers_off") || text.contains("HTTP 503") {
                let _ = shared.events.send(SyncEvent::ServersAvailable(false));
            } else {
                shared.diagnostic("servers", text);
            }
        }
    }
}

fn parse_server(v: Value) -> Result<ServerInfo> {
    serde_json::from_value::<ServerInfo>(v).context("bad server JSON")
}

/// A server write failed: the sheet shows it inline and a toast says it too.
/// A server action the user asked for (Create / Join / Add members / invite links) failed. The
/// sheet and the banner get plain words; the full chain goes to the log and — unlike background
/// polls — to a diagnostic even when the cause is a timeout, so a "timed out" nobody can
/// reproduce still leaves a signature (Jacob, 2026-10-23 14:34 UTC).
fn server_error(shared: &Arc<Shared>, verb: &str, err: &anyhow::Error) {
    let chain = format!("{err:#}");
    log::warn!(target: "arbos_mac::relay", "server action failed: {chain}");
    shared.diagnostic_always("server-action", chain);
    let _ = shared
        .events
        .send(SyncEvent::ServerError(UserError::classify(verb, err)));
}

/// The invite code from what the user pasted: a bare code or an `/s/{code}` link.
fn invite_code(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    match trimmed.rsplit_once("/s/") {
        Some((_, code)) => code
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_string(),
        None => trimmed.rsplit('/').next().unwrap_or(trimmed).to_string(),
    }
}

fn matrix_client(shared: &Arc<Shared>) -> Option<Arc<MatrixClient>> {
    shared.matrix.lock().unwrap().clone()
}

/// Registry and channel commands; each runs on its own task.
fn handle_server_command(shared: &Arc<Shared>, cmd: &ClientCommand) -> bool {
    match cmd {
        ClientCommand::ServersRefresh => {
            let shared = shared.clone();
            tokio::spawn(async move { refresh_servers_list(&shared).await });
        }
        ClientCommand::ServerCreate {
            name,
            leaf,
            picture,
        } => {
            let shared = shared.clone();
            let name = name.clone();
            let leaf = *leaf;
            let picture = picture.clone();
            tokio::spawn(async move {
                let mut body = json!({ "name": name });
                if let Some(l) = leaf {
                    body["avatar_leaf"] = json!(l.leaf);
                    body["avatar_leaf_color"] = json!(l.color);
                }
                match shared
                    .post_json("/servers", body)
                    .await
                    .and_then(parse_server)
                {
                    Ok(mut server) => {
                        if let Some(picture) = picture {
                            let id = server.id.clone();
                            if adopt_draft_picture(
                                &shared,
                                &format!("servers:{id}"),
                                &format!("/servers/{id}"),
                                "server-",
                                picture,
                            )
                            .await
                            {
                                if let Ok(fresh) = shared
                                    .get_json(&format!("/servers/{id}"))
                                    .await
                                    .and_then(parse_server)
                                {
                                    server = fresh;
                                }
                            }
                        }
                        let _ = shared
                            .events
                            .send(SyncEvent::ServerOpened(with_server_picture(
                                &shared, server,
                            )));
                        refresh_servers_list(&shared).await;
                    }
                    Err(err) => server_error(&shared, "create the server", &err),
                }
            });
        }
        ClientCommand::ServerJoin { code } => {
            let shared = shared.clone();
            let code = invite_code(code);
            tokio::spawn(async move {
                match shared
                    .post_json("/servers/join", json!({ "code": code }))
                    .await
                    .and_then(parse_server)
                {
                    Ok(server) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::ServerOpened(with_server_picture(
                                &shared, server,
                            )));
                        refresh_servers_list(&shared).await;
                    }
                    Err(err) => server_error(&shared, "join the server", &err),
                }
            });
        }
        ClientCommand::ServerFetch { server_id } => {
            let shared = shared.clone();
            let path = format!("/servers/{server_id}");
            tokio::spawn(async move {
                match shared.get_json(&path).await.and_then(parse_server) {
                    Ok(server) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::ServerDetail(with_server_picture(
                                &shared, server,
                            )));
                    }
                    Err(err) => shared.diagnostic("server-detail", format!("{err:#}")),
                }
            });
        }
        ClientCommand::ServerRename { server_id, name } => {
            let shared = shared.clone();
            let path = format!("/servers/{server_id}");
            let name = name.clone();
            tokio::spawn(async move {
                match shared
                    .patch_json(&path, json!({ "name": name }))
                    .await
                    .and_then(parse_server)
                {
                    Ok(server) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::ServerDetail(with_server_picture(
                                &shared, server,
                            )));
                    }
                    Err(err) => server_error(&shared, "rename the server", &err),
                }
            });
        }
        ClientCommand::ServerDelete { server_id } => {
            let shared = shared.clone();
            let path = format!("/servers/{server_id}");
            tokio::spawn(async move {
                // A brief send/timeout on DELETE is common on flaky links; retry twice
                // before surfacing. If the first attempt already removed the server but
                // the reply was cut, a retry gets 404 — the server is still gone.
                let result = {
                    let mut attempt = 0u32;
                    loop {
                        match shared.delete(&path).await {
                            Ok(()) => break Ok(()),
                            Err(err) => {
                                let text = format!("{err:#}");
                                if text.contains("HTTP 404") {
                                    break Ok(());
                                }
                                if diagnostics::is_transient(&text) && attempt < 2 {
                                    attempt += 1;
                                    sleep(Duration::from_millis(400 * u64::from(attempt))).await;
                                    continue;
                                }
                                break Err(err);
                            }
                        }
                    }
                };
                match result {
                    Ok(()) => refresh_servers_list(&shared).await,
                    Err(err) => server_error(&shared, "delete the server", &err),
                }
            });
        }
        ClientCommand::ServerLeave { server_id } => {
            let shared = shared.clone();
            let path = format!("/servers/{server_id}/leave");
            tokio::spawn(async move {
                match shared.post_json(&path, json!({})).await {
                    Ok(_) => refresh_servers_list(&shared).await,
                    Err(err) => server_error(&shared, "leave the server", &err),
                }
            });
        }
        ClientCommand::ServerInviteCreate {
            server_id,
            expires,
            max_uses,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let body = json!({ "expires": expires, "max_uses": max_uses });
            tokio::spawn(async move {
                let path = format!("/servers/{server_id}/invites");
                if let Err(err) = shared.post_json(&path, body).await {
                    server_error(&shared, "make the invite", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ServerInviteRevoke {
            server_id,
            invite_id,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let path = format!("/servers/{server_id}/invites/{invite_id}");
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&path).await {
                    server_error(&shared, "revoke the invite", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ServerSetRole {
            server_id,
            user_id,
            role,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let path = format!("/servers/{server_id}/members/{user_id}");
            let role = role.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.put_json(&path, json!({ "role": role })).await {
                    server_error(&shared, "change their role", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ServerRemoveMember {
            server_id,
            user_id,
            ban,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let path = if *ban {
                format!("/servers/{server_id}/members/{user_id}?ban=1")
            } else {
                format!("/servers/{server_id}/members/{user_id}")
            };
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&path).await {
                    server_error(&shared, "remove them", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ChannelCreate {
            server_id,
            name,
            restricted,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let body = json!({ "name": name, "restricted": restricted });
            tokio::spawn(async move {
                let path = format!("/servers/{server_id}/channels");
                if let Err(err) = shared.post_json(&path, body).await {
                    server_error(&shared, "create the channel", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ChannelPatch {
            server_id,
            channel_id,
            name,
            topic,
            restricted,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let path = format!("/servers/{server_id}/channels/{channel_id}");
            let mut body = serde_json::Map::new();
            if let Some(name) = name {
                body.insert("name".into(), json!(name));
            }
            if let Some(topic) = topic {
                body.insert("topic".into(), json!(topic));
            }
            if let Some(restricted) = restricted {
                body.insert("restricted".into(), json!(restricted));
            }
            tokio::spawn(async move {
                if let Err(err) = shared.patch_json(&path, Value::Object(body)).await {
                    server_error(&shared, "save the channel", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ChannelDelete {
            server_id,
            channel_id,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let path = format!("/servers/{server_id}/channels/{channel_id}");
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&path).await {
                    server_error(&shared, "delete the channel", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ServerAddMembers {
            server_id,
            user_ids,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let user_ids = user_ids.clone();
            tokio::spawn(async move {
                // All picks at once: one slow answer costs one timeout budget, not one per person;
                // the first failure is shown once the rest are done.
                let path = format!("/servers/{server_id}/members");
                let calls = user_ids
                    .into_iter()
                    .map(|uid| shared.post_json(&path, json!({ "user_id": uid })));
                let first_err = futures::future::join_all(calls)
                    .await
                    .into_iter()
                    .find_map(Result::err);
                if let Some(err) = first_err {
                    server_error(&shared, "add members", &err);
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ChannelArchive {
            server_id,
            channel_id,
            archived,
        } => {
            let shared = shared.clone();
            let server_id = server_id.clone();
            let path = format!("/servers/{server_id}/channels/{channel_id}/archive");
            let body = json!({ "archived": archived });
            tokio::spawn(async move {
                if let Err(err) = shared.post_json(&path, body).await {
                    server_error(&shared, "archive the channel", &err);
                    return;
                }
                fetch_server_detail(&shared, &server_id).await;
            });
        }
        ClientCommand::ChannelOpen { room_id } => {
            let Some(client) = matrix_client(shared) else {
                return true;
            };
            let shared = shared.clone();
            let room_id = room_id.clone();
            tokio::spawn(async move {
                match client.open_room(&room_id).await {
                    Ok(rows) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::MatrixTimeline { room_id, rows });
                    }
                    // Gone / not yet in the memory store after sync — wait for the next
                    // snapshot; not a product bug.
                    Err(MatrixError::NoRoom(_)) => {}
                    Err(err) => shared.diagnostic("channel-open", err.to_string()),
                }
            });
        }
        ClientCommand::ChannelLoadOlder { room_id } => {
            let Some(client) = matrix_client(shared) else {
                return true;
            };
            let shared = shared.clone();
            let room_id = room_id.clone();
            tokio::spawn(async move {
                match client.load_older(&room_id).await {
                    Ok(Some(rows)) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::MatrixTimeline { room_id, rows });
                    }
                    Ok(None) => {
                        let _ = shared.events.send(SyncEvent::ChannelHistoryEnd { room_id });
                    }
                    Err(MatrixError::NoRoom(_)) => {}
                    Err(err) => shared.diagnostic("channel-history", err.to_string()),
                }
            });
        }
        ClientCommand::ChannelSend {
            room_id,
            local_id,
            text,
        } => {
            let shared = shared.clone();
            let room_id = room_id.clone();
            let local_id = local_id.clone();
            let text = text.clone();
            let client = matrix_client(&shared);
            tokio::spawn(async move {
                let result = match client {
                    Some(client) => client
                        .send_text(&room_id, &text, &local_id)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                    None => Err("Servers are not connected".to_string()),
                };
                if let Err(error) = result {
                    let _ = shared.events.send(SyncEvent::ChannelSendFailed {
                        room_id,
                        local_id,
                        error: UserError::from_text("send your message", error),
                    });
                }
            });
        }
        ClientCommand::ChannelMarkRead { room_id } => {
            let Some(client) = matrix_client(shared) else {
                return true;
            };
            let room_id = room_id.clone();
            tokio::spawn(async move {
                let _ = client.mark_read(&room_id).await;
            });
        }
        _ => return false,
    }
    true
}

/// `GET /servers/{id}` → `ServerDetail`, then the list (previews, counts, roles).
async fn fetch_server_detail(shared: &Arc<Shared>, server_id: &str) {
    match shared
        .get_json(&format!("/servers/{server_id}"))
        .await
        .and_then(parse_server)
    {
        Ok(server) => {
            let _ = shared
                .events
                .send(SyncEvent::ServerDetail(with_server_picture(shared, server)));
        }
        Err(err) => shared.diagnostic("server-detail", format!("{err:#}")),
    }
    refresh_servers_list(shared).await;
}

/// Waits between picture fetch attempts. A cold launch fires the first fetch before the
/// network is up (a Mac that just woke, the relay mid-restart), and until 2026-10-26 one failed
/// attempt left the URL marked done forever: every surface drew the leaf placeholder for the
/// rest of the session — Jacob's "old picture after relaunch" (#1813 / #1862 reopen).
const AVATAR_RETRY_SECS: [u64; 6] = [2, 5, 10, 20, 40, 60];

/// The store-cached bytes for a picture URL, if the last run fetched it.
fn cached_avatar(shared: &Arc<Shared>, url: &str) -> Option<Vec<u8>> {
    shared
        .with_store(|s| s.get(&format!("{}{url}", store::KEY_AVATAR_PREFIX)))
        .flatten()
}

async fn fetch_avatar(shared: Arc<Shared>, url: String) {
    // `file://<path>` is a picked file (the avatar picker's Upload preview): read and mastered
    // here, off the UI thread, so the preview shows exactly what the upload will send — never
    // the raw 4000 px file through the GPU sampler (Jacob, 2026-10-26).
    if let Some(path) = url.strip_prefix("file://") {
        if !shared.avatars_done.lock().unwrap().insert(url.clone()) {
            return;
        }
        let path = PathBuf::from(path);
        let mastered = tokio::task::spawn_blocking(move || picture_master(&path)).await;
        match mastered {
            Ok(Ok(master)) => {
                let _ = shared.events.send(SyncEvent::Avatar {
                    url,
                    bytes: Arc::new(master.bytes),
                });
            }
            Ok(Err(err)) => shared.fail("read the picture", &err),
            Err(_) => {}
        }
        return;
    }
    // `/avatars/<user>.jpg?v=…` is the relay's own picture store (an Arbos
    // override of the GitHub avatar); everything else is absolute.
    let full = if url.starts_with('/') {
        format!("{}{}", shared.http_url, url)
    } else {
        url.clone()
    };
    if url.is_empty() || !full.starts_with("http") {
        return;
    }
    if !shared.avatars_done.lock().unwrap().insert(url.clone()) {
        return;
    }
    // The last run's bytes serve (the URL names the picture's version, so a cached copy is
    // never stale; a changed picture is a new URL and a new fetch).
    if let Some(bytes) = cached_avatar(&shared, &url) {
        let _ = shared.events.send(SyncEvent::Avatar {
            url: url.clone(),
            bytes: Arc::new(bytes),
        });
        return;
    }
    let mut attempt = 0usize;
    loop {
        let fetched = match shared.http.get(&full).send().await {
            Ok(resp) if resp.status().is_success() => resp.bytes().await.ok(),
            // 404 is "no picture": the leaf is right, nothing to retry.
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => break,
            Ok(_) | Err(_) => None,
        };
        if let Some(bytes) = fetched {
            let bytes = bytes.to_vec();
            shared.with_store(|s| s.put(&format!("{}{url}", store::KEY_AVATAR_PREFIX), &bytes));
            let _ = shared.events.send(SyncEvent::Avatar {
                url,
                bytes: Arc::new(bytes),
            });
            return;
        }
        let Some(wait) = AVATAR_RETRY_SECS.get(attempt) else {
            break;
        };
        attempt += 1;
        sleep(Duration::from_secs(*wait)).await;
    }
    // Gave up (or no picture): the next `want_avatar` for this URL may ask again.
    shared.avatars_done.lock().unwrap().remove(&url);
    let _ = shared.events.send(SyncEvent::AvatarMissing { url });
}

/// `GET /voices` → `SyncEvent::Voices` (a relay without it just leaves the list empty).
async fn fetch_voices(shared: &Arc<Shared>) {
    match shared.get_json("/voices").await {
        Ok(v) => match serde_json::from_value::<VoiceCatalogDto>(v) {
            Ok(dto) => {
                let (default, mut voices) = voices_from_dto(&dto);
                for x in &mut voices {
                    // Relative sample paths live on the relay.
                    if x.sample_url.starts_with('/') {
                        x.sample_url = format!("{}{}", shared.http_url, x.sample_url);
                    } else if x.sample_url.is_empty() {
                        x.sample_url = format!("{}/voices/{}/sample.mp3", shared.http_url, x.id);
                    }
                }
                let _ = shared.events.send(SyncEvent::Voices { default, voices });
            }
            Err(err) => shared.diagnostic("voices", format!("bad /voices JSON: {err}")),
        },
        Err(err) => {
            if !err.to_string().contains("HTTP 404") {
                shared.diagnostic("voices", people_error(&err));
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
    if let Ok(env) = serde_json::from_value::<CallEnvelopeDto>(v.clone()) {
        let call = Call::from(&env.call);
        return (!call.call_id.is_empty()).then_some(call);
    }
    serde_json::from_value::<CallDto>(v.clone())
        .ok()
        .map(|dto| Call::from(&dto))
        .filter(|c| !c.call_id.is_empty())
}

fn call_error(verb: &str, err: &anyhow::Error) -> UserError {
    let text = people_error(err);
    let said = if text.contains("calls_off") || text.contains("HTTP 503") {
        Some("Calls are not available on this relay yet.")
    } else if text.contains("call_ended") || text.contains("HTTP 410") {
        Some("That call has ended.")
    } else if text.contains("call_full") || text.contains("HTTP 409") {
        Some("The call is full (16 people).")
    } else if text.contains("HTTP 404") {
        Some("Calls are not available on this relay yet.")
    } else {
        None
    };
    match said {
        Some(why) => UserError::of(
            verb,
            ErrorClass::Refused {
                message: Some(why.to_owned()),
            },
            format!("{err:#}"),
        ),
        None => UserError::classify(verb, err),
    }
}

/// Calls v1 (contract §1): the client writes the system lines. One Loro row,
/// role `system`, text, `callId`; the room syncs it like any message.
fn write_system_line(shared: &Arc<Shared>, chat_id: &str, text: String, call_id: Option<String>) {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(chat_id) else {
        return;
    };
    let device = shared.device_id();
    doc.core.transact(|d| {
        let _ = d.append_system_line(&new_id(), &text, &device, call_id.as_deref());
    });
}

/// D82: the agent's first line in a brand-new project's chat, written by the
/// device that created the project. Idempotent: a chat that already holds a
/// `greeting: true` row (ours or another device's) gets nothing.
fn write_greeting(shared: &Arc<Shared>, chat_id: &str, project_id: &str, agent_name: &str) -> bool {
    let docs = shared.docs.lock().unwrap();
    let Some(doc) = docs.get(chat_id) else {
        return false;
    };
    if doc.core.has_greeting() {
        return true;
    }
    let text =
        format!("Hi, I’m {agent_name}. Your computer is ready — what should we build first?");
    let device = shared.device_id();
    doc.core.transact(|d| {
        let _ = d.append_greeting(&new_id(), &text, &device, project_id, agent_name);
    });
    true
}

fn people_error(err: &anyhow::Error) -> String {
    if let Some(e) = err.downcast_ref::<RelayError>() {
        return if e.message.is_empty() {
            format!("HTTP {}", e.status)
        } else {
            e.message.clone()
        };
    }
    format!("{err:#}")
}

/// `GET /projects` then `GET /projects/{id}` for chats, models and spend.
/// `GET /me/unread` → chat id → unread message count, for every chat the relay counts
/// unread in. The list is the user's one read cursor (shared by all their devices; own
/// messages never count), so a chat it does not name has nothing unread. `None` when the
/// relay did not answer it (older relay, transport error): the per-chat flag decides then.
async fn fetch_unread_map(shared: &Arc<Shared>) -> Option<HashMap<Id, u32>> {
    match shared.get_json("/me/unread").await {
        Ok(v) => match serde_json::from_value::<UnreadDto>(v) {
            Ok(dto) => Some(
                dto.chats
                    .into_iter()
                    .filter(|c| c.unread > 0)
                    .map(|c| (c.chat_id, c.unread as u32))
                    .collect(),
            ),
            Err(err) => {
                log::warn!(target: "arbos_mac::relay", "UnreadDto: {err}");
                None
            }
        },
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("me-unread", format!("{err:#}"));
            }
            None
        }
    }
}

/// Rig: `ARBOS_OPEN_CHAT=<chat id>` opens that chat (people or agent) once the registry lists
/// it, once per launch — the iPhone's hook of the same name; `ARBOS_FIXTURE_OPEN` is the
/// fixture source's.
fn apply_open_chat_hook(shared: &Arc<Shared>) {
    static APPLIED: std::sync::Once = std::sync::Once::new();
    let Ok(chat_id) = std::env::var("ARBOS_OPEN_CHAT") else {
        return;
    };
    let project_id = shared
        .chats
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.id == chat_id)
        .map(|c| c.project_id.clone());
    if let Some(project_id) = project_id {
        APPLIED.call_once(|| {
            let _ = shared.events.send(SyncEvent::RestoreSelection {
                project_id,
                chat_id,
            });
        });
    }
}

async fn refresh_registry(shared: &Arc<Shared>) -> Result<()> {
    shared.ensure_me().await;
    // Snapshot time: field edits after this must win over the relay's older list.
    let refresh_started_at = now_ms();
    // Per-chat unread counts (people + agent). Older relays answer 404.
    let unread_by_chat = fetch_unread_map(shared).await;
    let list = shared.get_json("/projects").await?;
    let items = list.as_array().cloned().unwrap_or_default();
    let mut projects: Vec<Project> = Vec::new();
    let mut all_chats: Vec<Chat> = Vec::new();
    let me_id = shared.me_id();
    for item in items {
        // `GET /projects` overwrites `owner` with the user-id string (not a
        // ProjectOwnerDto object), so the list row is not a full ProjectDto.
        // Only id + list extras are read here; detail fills the rest.
        let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let list_shared = item.get("shared").and_then(Value::as_bool);
        let list_owner_handle = item
            .get("owner_handle")
            .and_then(Value::as_str)
            .filter(|h| !h.is_empty())
            .map(str::to_owned);
        let detail_val = match shared.get_json(&format!("/projects/{id}")).await {
            Ok(v) => v,
            Err(err) => {
                // One bad project must not abort the whole refresh (reconnect loop).
                log::warn!(
                    target: "arbos_mac::relay",
                    "GET /projects/{id}: {err:#}; keeping last good entry"
                );
                if let Some(prev) = shared
                    .projects
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|x| x.id == id)
                    .cloned()
                {
                    projects.push(prev);
                }
                let prev_chats: Vec<Chat> = shared
                    .chats
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|c| c.project_id == id)
                    .cloned()
                    .collect();
                all_chats.extend(prev_chats);
                continue;
            }
        };
        let detail: GetProjectResponseDto = match serde_json::from_value(detail_val) {
            Ok(d) => d,
            Err(err) => {
                log::warn!(target: "arbos_mac::relay", "GetProjectResponseDto {id}: {err}");
                continue;
            }
        };
        // Who else is in this project (D72 step 3); older relays answer 404.
        let members = match shared.get_json(&format!("/projects/{id}/members")).await {
            Ok(v) => match serde_json::from_value::<ProjectMembersDto>(v) {
                Ok(m) => Some(m),
                Err(err) => {
                    log::warn!(target: "arbos_mac::relay", "ProjectMembersDto {id}: {err}");
                    None
                }
            },
            Err(_) => None,
        };
        let previous = shared
            .projects
            .lock()
            .unwrap()
            .iter()
            .find(|x| x.id == id)
            .cloned();
        let mut project = project_from_dto(&detail, members.as_ref());
        // D92 amendment: the agent's uploaded / generated picture — the public versioned route.
        project.image_url = project_image_url(
            &shared.http_url,
            &id,
            detail.project.image_key.as_deref(),
            project.avatar_version,
        );
        // list_projects extras (shared / owner_handle) live on the list row.
        if let Some(shared_flag) = list_shared {
            project.shared = shared_flag;
        }
        if project.owner_handle.is_none() {
            project.owner_handle = list_owner_handle;
        }
        project.people.retain(|p| p.id != me_id);
        if let Some(prev) = &previous {
            project.state_detail = prev.state_detail.clone();
            project.muted = prev.muted;
            if project.model.is_empty() {
                project.model = prev.model.clone();
            }
            if project.avatar.is_none() {
                project.avatar = prev.avatar.clone();
            }
            if project.avatar_color.is_none() {
                project.avatar_color = prev.avatar_color.clone();
            }
        }
        if project.model.is_empty() {
            project.model = "anthropic/claude-haiku-5.5".into();
        }
        if project.avatar_color_index.is_none() {
            project.avatar_color_index = Some(crate::ui::avatars::color_index(&id));
        }
        let chats: Vec<Chat> = detail.chats.iter().map(chat_from_dto).collect();
        projects.push(project);
        all_chats.extend(chats);
    }

    // People chats (D72) share the rail: one space + one pinned chat each.
    let people_ids: Vec<Id> = projects
        .iter()
        .filter(|p| p.is_people())
        .map(|p| p.id.clone())
        .collect();
    // A brief connection timeout on GET /chats is common on flaky networks; retry twice
    // before keeping the cached list (and only then file a diagnostic for non-transient errors).
    let people_list = {
        let mut attempt = 0u32;
        loop {
            match shared.get_json("/chats").await {
                Ok(list) => break Ok(list),
                Err(err) => {
                    let text = format!("{err:#}");
                    if text.contains("HTTP 404") {
                        break Err(err);
                    }
                    if diagnostics::is_transient(&text) && attempt < 2 {
                        attempt += 1;
                        sleep(Duration::from_millis(400 * u64::from(attempt))).await;
                        continue;
                    }
                    break Err(err);
                }
            }
        }
    };
    match people_list {
        Ok(list) => {
            let mut ids = shared.people.lock().unwrap();
            let me_handle = shared.me_handle();
            let token = shared.token();
            for item in list.as_array().cloned().unwrap_or_default() {
                let dto: PeopleChatDto = match serde_json::from_value(item) {
                    Ok(d) => d,
                    Err(err) => {
                        log::warn!(target: "arbos_mac::relay", "PeopleChatDto: {err}");
                        continue;
                    }
                };
                let (space, chat) =
                    people_space_from(&dto, &me_id, &me_handle, &shared.http_url, &token);
                ids.insert(space.id.clone());
                projects.push(space);
                all_chats.push(chat);
            }
        }
        Err(err) => {
            // Older relays have no /chats; keep whatever we already knew.
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("people-chats", format!("{err:#}"));
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
    // Keep local-only tabs (minted, not yet created) and recent local edits.
    // Unread comes from GET /me/unread (and people `dto.unread`) merged with local read marks.
    let local_reads = shared.local_read_at.lock().unwrap().clone();
    let local_changed = shared.local_changed_at.lock().unwrap().clone();
    let pending_ids: HashSet<Id> = shared
        .pending_new_chats
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    {
        let mut local = shared.chats.lock().unwrap();
        let mut merged = merge_chats(
            &all_chats,
            &local,
            refresh_started_at,
            &local_changed,
            &pending_ids,
        );
        for c in &mut merged {
            let relay_count =
                relay_unread_for(unread_by_chat.as_ref(), &c.id, c.unread, c.unread_count);
            let local_read = local_reads.get(&c.id).copied();
            let count = merged_unread(relay_count, local_read, c.last_message_at as i64);
            c.unread_count = count;
            c.unread = count > 0;
        }
        *local = merged;
    }
    // One copy of each agent's avatar everywhere: the newest stamp wins over any roster copy
    // that arrived older (Jacob, 2026-10-25).
    super::agent_avatar::reconcile(&mut projects);
    *shared.projects.lock().unwrap() = projects.clone();
    let _ = shared.events.send(SyncEvent::Projects(projects.clone()));
    log_first_paint("relay", projects.len());
    emit_chats(shared);
    apply_open_chat_hook(shared);
    save_registry(shared);
    refresh_blocks(shared).await;
    for p in projects.iter().filter(|p| !p.is_people()) {
        ensure_room(shared, &p.id, &project_doc_key(&p.id), PROJECT_DOC_CHAT_ID);
    }
    for c in &all_chats {
        ensure_room(shared, &c.project_id, &c.id, &c.id);
    }
    Ok(())
}

/// The relay's block list is the truth for the person pane's Block user / Unblock row
/// (#1545): read it after every registry refresh and after each block / unblock, so the row
/// is right on pane re-open and app relaunch. Older relays without `/blocks` answer 404 and
/// the list is left as it was; other errors are logged, never shown.
async fn refresh_blocks(shared: &Arc<Shared>) {
    match shared.get_json("/blocks").await {
        Ok(list) => {
            let people: Vec<Person> = list
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|item| serde_json::from_value::<Person>(item).ok())
                .collect();
            let _ = shared.events.send(SyncEvent::Blocked(people));
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                log::warn!(target: "arbos_mac::relay", "GET /blocks: {err:#}");
            }
        }
    }
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
                shared.diagnostic("archive-sync", people_error(&err));
            }
        }
    }
    if all_ok {
        shared.with_store(|s| s.put_json(store::KEY_ARCHIVE_MIGRATED, &true));
    }
}

/// `PUT /chats/{id}/settings {archived}` (§9c). The relay echoes a project chat as `ChatDto`
/// and a people chat as `PeopleChatDto`; answers the stored `archived_at` (None = unarchived).
async fn set_archived(
    shared: &Arc<Shared>,
    project_id: &str,
    chat_id: &str,
    archived: bool,
) -> Result<Option<u64>> {
    let body = json!({ "archived": archived });
    let v = shared
        .put_json(&format!("/chats/{chat_id}/settings"), body)
        .await?;
    let at = if shared.is_people(project_id) {
        serde_json::from_value::<PeopleChatDto>(v)
            .context("decode people chat")?
            .archived_at
    } else {
        serde_json::from_value::<ChatDto>(v)
            .context("decode chat")?
            .archived_at
            .flatten()
    };
    Ok(at.filter(|t| *t > 0).map(|t| t as u64))
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
    if trace_rows() {
        let last = messages
            .last()
            .map(|m| format!("{:?}/{}", m.role, m.id))
            .unwrap_or_default();
        eprintln!(
            "rows: snapshot chat={chat_id} n={} last={last} at={}",
            messages.len(),
            now_ms()
        );
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
    let threads_v = to_json(doc.core.doc.get_map("threads").get_deep_value());
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
    let memory = to_json(doc.core.doc.get_map("memory").get_deep_value());
    let view = ProjectDocView {
        threads,
        status: to_json(doc.core.doc.get_map("status").get_deep_value()),
        settings: to_json(doc.core.doc.get_map("settings").get_deep_value()),
        memory_t1: memory
            .get("t1")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        model_note: view_settings_note(&doc.core.doc),
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

    let sub = doc.core.doc.subscribe_root(Arc::new(move |event| {
        let touched = ChatDoc::touched(&event);
        if is_project_doc || touched.list_changed || !touched.messages.is_empty() {
            let _ = change_tx.send(touched);
        }
    }));
    let local_sub = doc
        .core
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

/// True when `lower` has `@token` as a whole mention (not a longer `@token…` handle).
fn text_has_at_token(lower: &str, token: &str) -> bool {
    let t = format!("@{token}");
    lower.match_indices(&t).any(|(i, _)| {
        let after = lower[i + t.len()..].chars().next();
        after.is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
    })
}

/// `@slug` / `@handle` in the text → `agent:<project>` / `user:<id>` (API §3);
/// `@here` / `@agents` expand to the chat's members (`sync::expand_collectives`).
/// Who a people-chat send mentions: the composer's committed chips (`tokens`,
/// member ids, #1596) first, then every `@handle` the text names that no chip
/// already accounts for — so a chip for one of two entities sharing a handle
/// (#1538) is never second-guessed by the text scan, and pasted plain
/// `@handle`s still resolve.
fn mentions_for(shared: &Shared, chat_id: &str, text: &str, tokens: &[String]) -> Vec<String> {
    let lower = text.to_lowercase();
    let projects = shared.projects.lock().unwrap();
    let Some(space) = projects.iter().find(|p| p.id == chat_id) else {
        return tokens.to_vec();
    };
    // Chips first (collective markers expanded into the members), then the text.
    let mut out = crate::sync::expand_collectives(space, &lower, tokens, &shared.me_id());
    let claimed = crate::sync::claimed_handles(space, tokens);
    let me_handle = shared.me_handle();
    for a in &space.agents {
        let handle = a.handle.to_lowercase();
        let slug = Project::mention_slug(&a.name);
        if claimed.contains(&handle) || claimed.contains(&slug) {
            continue;
        }
        // Match cached handle or the live name slug so a mid-group rename
        // still encodes `agent:<id>` before membership is refreshed.
        if (!handle.is_empty() && text_has_at_token(&lower, &handle))
            || (!slug.is_empty() && text_has_at_token(&lower, &slug))
        {
            let id = format!("agent:{}", a.project_id);
            if !out.iter().any(|x| x == &id) {
                out.push(id);
            }
        }
    }
    for p in &space.people {
        let handle = p.handle.to_lowercase();
        if claimed.contains(&handle) || !text_has_at_token(&lower, &handle) {
            continue;
        }
        // Typed resolution (#1538): a slug both a person and an agent carry means the agent
        // when it is my own handle (nobody @-mentions themselves) and the person otherwise.
        let agent_has_it = space.agents.iter().any(|a| {
            a.handle.eq_ignore_ascii_case(&handle) || Project::mention_slug(&a.name) == handle
        });
        if agent_has_it && handle.eq_ignore_ascii_case(&me_handle) {
            continue;
        }
        let id = format!("user:{}", p.id);
        if !out.iter().any(|x| x == &id) {
            out.push(id);
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

/// `POST /machine/start|stop` reply → the machine view.
fn set_machine_from_reply(shared: &Shared, dto: &MachineStateDto) {
    let state = match dto.state {
        WireBox::Awake => MachineState::Awake,
        WireBox::Waking => MachineState::Waking,
        WireBox::Asleep => MachineState::Asleep,
        WireBox::Error => MachineState::Error,
    };
    set_machine(
        shared,
        state,
        (dto.since > 0).then_some(dto.since as u64),
        None,
    );
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
        serde_json::to_value(doc.core.doc.get_map("status").get_deep_value()).unwrap_or(Value::Null)
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
    let Ok(dto) = serde_json::from_value::<MachineStatusDto>(value) else {
        return;
    };
    let snapshot = {
        let mut m = shared.machine.lock().unwrap();
        apply_machine_status(&mut m, &dto);
        m.clone()
    };
    let _ = shared.events.send(SyncEvent::Machine(snapshot));
    for ws in &dto.workspaces {
        let _ = shared.events.send(SyncEvent::Workspace {
            project_id: ws.project_id.clone(),
            running: ws.state == "running",
        });
        if let Some(state) = box_state_from_word(&ws.state) {
            set_project_state(shared, &ws.project_id, state, None);
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

/// Dial, drive [`RoomSession`], perform actions — forever, with backoff (§8.6).
/// Protocol (HELLO, catch-up, PUSH/ACK, resend) lives in `arbos_client::room`.
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
    let is_people = shared.is_people(&room_chat);
    let room = room::room_path(&project_id, &room_chat, is_people);
    let mut session = RoomSession::new(shared.device_id(), room, saved_cursor, saved_pending);
    // The first dial: the chat is about to catch up, so the pill (after its
    // grace) says "Syncing…", not "Reconnecting…".
    let _ = shared.events.send(SyncEvent::RoomLink {
        key: key.clone(),
        state: LinkState::Syncing,
    });
    loop {
        // Re-resolve on every dial. Before RoomSession, run_session re-checked
        // the people HashSet each attempt; baking the path once meant a cold
        // start with an empty set dialed `{id}/{id}` forever while iOS used
        // `people/{id}` — Mac and mobile never shared a CRDT room.
        let is_people = shared.is_people(&room_chat);
        let want = room::room_path(&project_id, &room_chat, is_people);
        if session.room() != want {
            session = RoomSession::new(
                shared.device_id(),
                want,
                session.cursor(),
                session.pending().to_vec(),
            );
        }
        let outcome = run_session(
            &shared,
            &project_id,
            &key,
            &room_chat,
            is_people,
            &mut session,
            &mut local_rx,
            &mut cmd_rx,
        )
        .await;
        // What the pill says while this room is down, from the real failure:
        // a dial that could not reach the relay is "Offline" / "Relay down";
        // a socket that was up and dropped is "Reconnecting…".
        let state = match &outcome {
            Err(err) => {
                eprintln!("relay room {project_id}/{room_chat}: {err:#}");
                if err.downcast_ref::<DialFailed>().is_some() {
                    link_fault(err)
                } else {
                    LinkState::Reconnecting
                }
            }
            Ok(()) => LinkState::Reconnecting,
        };
        let _ = shared.events.send(SyncEvent::RoomLink {
            key: key.clone(),
            state,
        });
        // If run_session returned without closing (e.g. dial failure), close now.
        if !session.is_closed() {
            session.on_close(now_ms());
        }
        let _ = perform_room_actions(
            &shared,
            &project_id,
            &key,
            &room_chat,
            is_people,
            &mut session,
            None,
        )
        .await;
        // A clean server close (1012 Service Restart / 1001 Going Away: the
        // relay is restarting for a deploy) is not a fault: redial soon with a
        // fresh backoff. The doubled backoff of a run of failures is for the
        // relay being unreachable; a deploy must never leave a client parked on
        // it (#1340 — that is what stranded Jacob on Oct 22).
        let clean_close = outcome
            .as_ref()
            .err()
            .is_some_and(|err| err.downcast_ref::<ServerRestart>().is_some());
        if clean_close {
            session = RoomSession::new(
                shared.device_id(),
                session.room().to_string(),
                session.cursor(),
                session.pending().to_vec(),
            );
            sleep(RESTART_REDIAL).await;
        } else {
            sleep(session.reconnect_after()).await;
            session.prepare_reconnect();
        }
    }
}

/// How long after a clean server close (deploy restart) the room redials.
const RESTART_REDIAL: Duration = Duration::from_millis(1_500);

/// Marker in a `run_session` error chain: the relay closed the socket on
/// purpose (close code 1012 Service Restart or 1001 Going Away).
#[derive(Debug)]
struct ServerRestart(u16);

impl std::fmt::Display for ServerRestart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server closed the socket ({})", self.0)
    }
}

impl std::error::Error for ServerRestart {}

/// Close codes a relay sends when it is going down on purpose.
fn is_restart_close(code: u16) -> bool {
    code == 1012 || code == 1001
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    shared: &Arc<Shared>,
    project_id: &str,
    key: &str,
    room_chat: &str,
    is_people: bool,
    session: &mut RoomSession,
    local_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    cmd_rx: &mut mpsc::UnboundedReceiver<RoomCmd>,
) -> Result<()> {
    let room = session.room().to_string();
    let url = format!("{}/room/{}?token={}", shared.ws_url, room, shared.token());
    let (ws, _) = timeout(HELLO_DEADLINE, connect_async(&url))
        .await
        .context("dial timed out")
        .context(DialFailed)?
        .context(DialFailed)?;
    let (mut tx, mut rx) = ws.split();

    // Socket up: catching up until RowsDone (`RoomAction::LinkLive`).
    let _ = shared.events.send(SyncEvent::RoomLink {
        key: key.to_string(),
        state: LinkState::Syncing,
    });
    session.on_open(now_ms());
    perform_room_actions(
        shared,
        project_id,
        key,
        room_chat,
        is_people,
        session,
        Some(&mut tx),
    )
    .await?;

    let mut presence = interval(PRESENCE_EVERY);
    let mut tick = interval(Duration::from_secs(1));
    // The close code the relay sent, if it sent one before the stream ended.
    let mut close_code: Option<u16> = None;
    loop {
        if session.is_closed() {
            anyhow::bail!("session closed");
        }
        tokio::select! {
            msg = rx.next() => {
                let Some(msg) = msg else {
                    session.on_close(now_ms());
                    perform_room_actions(
                        shared, project_id, key, room_chat, is_people, session, Some(&mut tx),
                    ).await?;
                    if let Some(code) = close_code.filter(|c| is_restart_close(*c)) {
                        return Err(anyhow::Error::new(ServerRestart(code)));
                    }
                    anyhow::bail!("socket closed");
                };
                let msg = msg?;
                if let WsMessage::Close(frame) = &msg {
                    close_code = frame.as_ref().map(|f| u16::from(f.code));
                    continue;
                }
                let WsMessage::Binary(bytes) = msg else { continue };
                session.on_frame(&bytes);
                perform_room_actions(
                    shared, project_id, key, room_chat, is_people, session, Some(&mut tx),
                ).await?;
            }
            bytes = local_rx.recv() => {
                let Some(bytes) = bytes else { anyhow::bail!("doc dropped") };
                session.on_local_commit(bytes);
                perform_room_actions(
                    shared, project_id, key, room_chat, is_people, session, Some(&mut tx),
                ).await?;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { anyhow::bail!("room closed") };
                match cmd {
                    RoomCmd::Wake(reason) => session.wake(reason),
                    RoomCmd::Typing(active) => {
                        session.typing(shared.presence_header(), active);
                    }
                    RoomCmd::Presence => {
                        session.presence(shared.presence_header());
                    }
                }
                perform_room_actions(
                    shared, project_id, key, room_chat, is_people, session, Some(&mut tx),
                ).await?;
            }
            _ = presence.tick() => {
                session.presence(shared.presence_header());
                perform_room_actions(
                    shared, project_id, key, room_chat, is_people, session, Some(&mut tx),
                ).await?;
            }
            _ = tick.tick() => {
                session.on_tick(now_ms());
                perform_room_actions(
                    shared, project_id, key, room_chat, is_people, session, Some(&mut tx),
                ).await?;
            }
        }
    }
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;

/// Run queued [`RoomAction`]s: send frames, HTTP checkpoint, apply bytes, emit events.
/// Returns `Some(delay)` when the session asked to reconnect.
#[allow(clippy::too_many_arguments)]
async fn perform_room_actions(
    shared: &Arc<Shared>,
    project_id: &str,
    key: &str,
    room_chat: &str,
    is_people: bool,
    session: &mut RoomSession,
    mut tx: Option<&mut WsSink>,
) -> Result<Option<Duration>> {
    let mut reconnect = None;
    // Checkpoint fetch may enqueue more actions; loop until the queue is empty.
    loop {
        let actions = session.drain_actions();
        if actions.is_empty() {
            return Ok(reconnect);
        }
        let mut abort_batch = false;
        for action in actions {
            match action {
                RoomAction::SendFrame(bytes) => {
                    if let Some(tx) = tx.as_mut() {
                        tx.send(WsMessage::Binary(bytes.into())).await?;
                    }
                }
                RoomAction::FetchCheckpoint {
                    room,
                    frontier,
                    checkpoint_seq: _,
                } => {
                    let contained = {
                        let docs = shared.docs.lock().unwrap();
                        docs.get(key)
                            .is_some_and(|doc| doc.core.contains_frontier(&frontier))
                    };
                    if contained {
                        session.on_checkpoint_skipped();
                    } else {
                        match fetch_checkpoint(shared, &room).await {
                            Ok((seq, bytes)) => {
                                let import_ok = {
                                    let docs = shared.docs.lock().unwrap();
                                    if let Some(doc) = docs.get(key) {
                                        match doc.core.doc.import(&bytes) {
                                            Ok(_) => true,
                                            Err(err) => {
                                                eprintln!(
                                                    "relay room {room}: checkpoint import failed: {err}"
                                                );
                                                false
                                            }
                                        }
                                    } else {
                                        false
                                    }
                                };
                                if import_ok {
                                    session.on_checkpoint(seq);
                                } else {
                                    session.on_checkpoint_failed();
                                }
                            }
                            Err(err) => {
                                eprintln!("relay room {room}: checkpoint fetch failed: {err:#}");
                                session.on_checkpoint_failed();
                            }
                        }
                    }
                }
                RoomAction::ApplyBytes(bytes) => {
                    if trace_rows() {
                        eprintln!(
                            "rows: row-apply room={} bytes={} at={}",
                            session.room(),
                            bytes.len(),
                            now_ms()
                        );
                    }
                    // (failed, parked): parked = Loro holds the ops until their dependencies
                    // arrive; the session keeps the persisted cursor below that row.
                    let (import_failed, parked) = {
                        let docs = shared.docs.lock().unwrap();
                        if let Some(doc) = docs.get(key) {
                            match doc.core.doc.import(&bytes) {
                                Ok(status) => (false, status.pending.is_some()),
                                Err(err) => {
                                    eprintln!(
                                        "relay room {}: import failed: {err}",
                                        session.room()
                                    );
                                    (true, false)
                                }
                            }
                        } else {
                            (false, false)
                        }
                    };
                    if parked {
                        session.apply_pending();
                    }
                    if import_failed {
                        // Skip the Cursor that follows in this drain; reconnect
                        // from the last good cursor via apply_failed.
                        session.apply_failed();
                        abort_batch = true;
                    }
                }
                RoomAction::Cursor(cursor) => {
                    shared
                        .room_seq
                        .lock()
                        .unwrap()
                        .insert(key.to_string(), cursor);
                    shared.mark_dirty(key);
                }
                RoomAction::Pending(pending) => {
                    shared.with_store(|s| s.save_pending(key, &pending));
                }
                RoomAction::Ack { .. } => {
                    let had: Vec<Id> = shared.unacked.lock().unwrap().drain().collect();
                    if !had.is_empty() && room_chat != PROJECT_DOC_CHAT_ID {
                        emit_snapshot(shared, key);
                    }
                    // Per-room refusal flag (not `unacked`: that set is global and any
                    // room's ACK drains it, so it cannot tell which chat was refused).
                    if shared.push_refused.lock().unwrap().remove(room_chat) {
                        let _ = shared.events.send(SyncEvent::PushRefused {
                            chat_id: room_chat.to_string(),
                            refused: false,
                        });
                    }
                }
                RoomAction::PushRefused { batch_id: _ } => {
                    if room_chat != PROJECT_DOC_CHAT_ID
                        && shared
                            .push_refused
                            .lock()
                            .unwrap()
                            .insert(room_chat.to_string())
                    {
                        // The chat-level note draws it (`PushRefused`); the room link stays live.
                        let _ = shared.events.send(SyncEvent::PushRefused {
                            chat_id: room_chat.to_string(),
                            refused: true,
                        });
                    }
                }
                RoomAction::BoxState {
                    state,
                    detail,
                    poll_machine,
                } => {
                    set_project_state(shared, project_id, map_box(state), detail.clone());
                    set_machine(shared, machine_from_wire(state), None, detail);
                    if poll_machine {
                        let poll = shared.clone();
                        tokio::spawn(async move { refresh_machine(&poll).await });
                    }
                }
                RoomAction::LinkLive => {
                    let _ = shared.events.send(SyncEvent::Link(Link::Live));
                    let _ = shared.events.send(SyncEvent::RoomLink {
                        key: key.to_string(),
                        state: LinkState::Live,
                    });
                    if room_chat == PROJECT_DOC_CHAT_ID {
                        emit_project_doc(shared, project_id);
                    } else {
                        emit_snapshot(shared, key);
                        let _ = shared.events.send(SyncEvent::ChatSynced(key.to_string()));
                    }
                }
                RoomAction::HealCheck { relay_vv } => {
                    // The relay told us what it holds; if this replica has more (a lost
                    // checkpoint, #1518), push the difference back once so the room heals.
                    // Always answer (even empty) so the session can release the held pending resend.
                    let missing = {
                        let docs = shared.docs.lock().unwrap();
                        match (docs.get(key), arbos_core::doc::decode_vv(&relay_vv)) {
                            (Some(doc), Some(vv)) => doc.core.updates_beyond(&vv),
                            _ => Vec::new(),
                        }
                    };
                    if !missing.is_empty() {
                        eprintln!(
                            "relay room {}: history heal, pushing {} bytes the relay lacked",
                            session.room(),
                            missing.len()
                        );
                    }
                    session.on_heal(missing);
                }
                RoomAction::ScheduleReconnect { after } => {
                    reconnect = Some(after);
                }
                RoomAction::SessionError { code, message } => {
                    if code == "restart_soon" {
                        // The relay restarts in a few seconds (#1340): the
                        // pill says so; the clean close that follows redials.
                        let _ = shared.events.send(SyncEvent::RoomLink {
                            key: key.to_string(),
                            state: LinkState::RestartSoon,
                        });
                    } else if code != "state_timeout" && code != "lagging" {
                        eprintln!("relay room {}: error {}: {}", session.room(), code, message);
                    }
                }
                RoomAction::Presence {
                    device: _,
                    handle,
                    typing,
                    in_call,
                    mine,
                    header,
                } => {
                    if let Some(call_id) = in_call {
                        if !mine
                            && shared.in_call.lock().unwrap().as_deref() != Some(call_id.as_str())
                        {
                            let _ = shared.events.send(SyncEvent::CallSeen {
                                chat_id: room_chat.to_string(),
                                call_id,
                            });
                        }
                    }
                    if let (false, Some(handle)) = (mine, handle) {
                        let seen = header
                            .get("seen")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                        let _ = shared.events.send(SyncEvent::Presence {
                            handle: handle.clone(),
                            chat_id: is_people.then(|| room_chat.to_string()),
                            seen,
                        });
                        if is_people {
                            if let Some(active) = typing {
                                let _ = shared.events.send(SyncEvent::Typing {
                                    chat_id: room_chat.to_string(),
                                    handle,
                                    active,
                                });
                            }
                        }
                    }
                }
                RoomAction::DroppedOversize { bytes } => {
                    eprintln!(
                        "relay room {}: local update {bytes} bytes exceeds row cap; dropped",
                        session.room()
                    );
                }
            }
            if abort_batch {
                break;
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
            shared.fail("wake the machine", &err);
            set_project_state(
                &shared,
                &project_id,
                BoxState::Error,
                Some(format!("{err:#}")),
            );
        }
    });
}

/// A picked picture file as the master the relay stores (`sync::avatar_master`: ≤ 1024 px,
/// Lanczos, PNG for a flat mark, JPEG q92 for a photo). PNG, JPEG, GIF and WebP decode here;
/// HEIC (Photos exports, iPhone shots) goes through macOS's own `sips` first, since the `image`
/// crate has no HEIF decoder. Anything that is not an image is an error with a plain message.
/// The blob slot a picker target's pictures live in; the draft's is my own (`users:{me}`).
fn picture_slot(shared: &Shared, target: &PictureTarget) -> String {
    match target {
        PictureTarget::Agent(id) => id.clone(),
        PictureTarget::Group(id) => format!("people:{id}"),
        PictureTarget::Server(id) => format!("servers:{id}"),
        PictureTarget::Draft => format!("users:{}", shared.me_id()),
        PictureTarget::Me(id) => format!("users:{id}"),
    }
}

/// A picker target's picture as the avatar loader fetches it: the blob in its own slot.
fn picture_url(shared: &Shared, target: &PictureTarget, key: Option<&str>) -> Option<String> {
    let key = key.map(str::trim).filter(|k| !k.is_empty())?;
    let slot = picture_slot(shared, target);
    Some(format!(
        "{}/blob/{slot}/{key}?token={}",
        shared.http_url,
        shared.token()
    ))
}

/// How long one avatar generation may take on the wire: past the relay's 75 s budget (40 s per
/// provider attempt), so the relay's own 504 reaches the sheet before this client gives up.
const AVATAR_GENERATE_TIMEOUT: Duration = Duration::from_secs(90);

/// What the picker says when a generation failed, as the shared [`UserError`] (Jacob,
/// 2026-10-26: `POST …/avatar/generate: error sending request … operation timed out` had been
/// shown verbatim across the footer). A relay without the own-slot route yet (#1789 not
/// deployed) says so instead of a 404.
fn generate_failure(target: &PictureTarget, err: &anyhow::Error) -> UserError {
    if let Some(e) = err.downcast_ref::<RelayError>() {
        if *target == PictureTarget::Draft && e.status == 404 {
            return UserError::of(
                "generate the image",
                ErrorClass::Refused {
                    message: Some(
                        "Generate is not on this relay yet; upload a picture instead.".into(),
                    ),
                },
                format!("{err:#}"),
            );
        }
    }
    UserError::classify("generate the image", err)
}

/// The answer to `PUT /me/avatar` (a JPEG body or `{image_key}`): the new `user` lands in the
/// store and `/me` is re-read so every surface shows the picture; a failure is a notice.
async fn my_avatar_saved(shared: &Arc<Shared>, put: Result<reqwest::Response, reqwest::Error>) {
    match put {
        Ok(r) if r.status().is_success() => {
            if let Ok(body) = r.json::<Value>().await {
                if let Ok(dto) = serde_json::from_value::<MeUserResponseDto>(body) {
                    let u: credentials::AccountUser = (&dto.user).into();
                    shared.with_store(|s| s.put_json(store::KEY_ACCOUNT, &u));
                    *shared.me.lock().unwrap() = Some(u);
                }
            }
            refresh_me(shared).await;
        }
        Ok(r) => {
            let status = r.status();
            let detail = r
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            shared.fail("save the picture", &detail);
        }
        Err(err) => shared.fail("save the picture", &err),
    }
}

/// The Create sheet's pending picture, once the entity exists: the file (or the bytes of the
/// picture generated into my own slot) goes into the new entity's slot as a picture master, then
/// `PATCH <patch_path> {image_key}`. `prefix` names the key (`avatar-` / `server-`). Errors are
/// a notice — the entity itself is already created and shows its neutral initial.
async fn adopt_draft_picture(
    shared: &Arc<Shared>,
    slot: &str,
    patch_path: &str,
    prefix: &str,
    picture: DraftPicture,
) -> bool {
    let master = match picture {
        DraftPicture::File(path) => {
            match tokio::task::spawn_blocking(move || picture_master(&path)).await {
                Ok(Ok(m)) => m,
                Ok(Err(err)) => {
                    shared.fail("upload the picture", &err);
                    return false;
                }
                Err(_) => return false,
            }
        }
        DraftPicture::Generated { key } => {
            let from = picture_url(shared, &PictureTarget::Draft, Some(&key));
            let Some(from) = from else { return false };
            match shared.http.get(from).send().await {
                Ok(r) if r.status().is_success() => match r.bytes().await {
                    // The relay's copy is already a master; its type travels with the bytes.
                    Ok(b) => Master {
                        png: b.starts_with(b"\x89PNG"),
                        bytes: b.to_vec(),
                    },
                    Err(err) => {
                        shared.fail("keep the generated picture", &err);
                        return false;
                    }
                },
                Ok(r) => {
                    shared.fail("keep the generated picture", r.status());
                    return false;
                }
                Err(err) => {
                    shared.fail("keep the generated picture", &err);
                    return false;
                }
            }
        }
    };
    let key = format!("{prefix}{}.{}", new_id().to_lowercase(), master.ext());
    let put = shared
        .http
        .put(format!("{}/blob/{slot}/{key}", shared.http_url))
        .bearer_auth(shared.token())
        .header("content-type", master.mime())
        .body(master.bytes)
        .send()
        .await;
    match put {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => {
            shared.fail("upload the picture", r.status());
            return false;
        }
        Err(err) => {
            shared.fail("upload the picture", &err);
            return false;
        }
    }
    match shared
        .patch_json(patch_path, json!({ "image_key": key }))
        .await
    {
        Ok(_) => true,
        Err(err) => {
            shared.fail("save the picture", &err);
            false
        }
    }
}

/// A server's `image_key` → `image_url` on every `ServerInfo` the relay client hands out.
fn with_server_picture(shared: &Shared, mut server: ServerInfo) -> ServerInfo {
    server.image_url = picture_url(
        shared,
        &PictureTarget::Server(server.id.clone()),
        server.image_key.as_deref(),
    );
    server
}

/// An agent's picture as the avatar loader fetches it: the relay's public picture route,
/// stamped with `avatar_version` (`sync::agent_avatar::picture_url`). The blob slot is the
/// owner's and the guests' only — a viewer who merely shared a group got 403 and the leaf.
fn project_image_url(
    http_url: &str,
    project_id: &str,
    key: Option<&str>,
    version: i64,
) -> Option<String> {
    super::agent_avatar::picture_url(http_url, project_id, key, version)
}

fn picture_master(path: &Path) -> Result<Master> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    match super::avatar_master::master(&bytes) {
        Ok(master) => Ok(master),
        Err(err) => match heic_to_jpeg(path) {
            Some(Ok(converted)) => super::avatar_master::master(&converted),
            Some(Err(conv)) => Err(conv),
            None => Err(err),
        },
    }
}

/// `sips -s format jpeg` for HEIC/HEIF files on macOS; `None` when the file is not one (or
/// this is not a Mac), so the original error stands.
fn heic_to_jpeg(path: &Path) -> Option<Result<Vec<u8>>> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !cfg!(target_os = "macos") || !matches!(ext.as_str(), "heic" | "heif") {
        return None;
    }
    let out = std::env::temp_dir().join(format!("arbos-picture-{}.jpg", new_id().to_lowercase()));
    let run = std::process::Command::new("sips")
        .args(["-s", "format", "jpeg"])
        .arg(path)
        .arg("--out")
        .arg(&out)
        .output();
    let result = match run {
        Ok(o) if o.status.success() => std::fs::read(&out).context("read converted picture"),
        Ok(o) => Err(anyhow!(
            "could not read this HEIC picture: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(err) => Err(anyhow!("could not convert the HEIC picture: {err}")),
    };
    let _ = std::fs::remove_file(&out);
    Some(result)
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

/// `GET /blob/{slot}/{key}` → (mime, bytes); `id` is a project id or a people chat id.
async fn fetch_blob_bytes(shared: &Arc<Shared>, id: &str, key: &str) -> Result<(String, Vec<u8>)> {
    // Same refresh-once path as `get_json`: an expired access token must not leave
    // thumbnails / downloads stuck on "No preview" and a weekly `mac:thumbnail:* http 401`
    // diagnostic while the JSON APIs quietly rotate and succeed.
    let mut refreshed = false;
    loop {
        let token = shared.token();
        let r = shared
            .http
            .get(format!(
                "{}/blob/{}/{}",
                shared.http_url,
                shared.blob_slot(id),
                key
            ))
            .bearer_auth(&token)
            .send()
            .await?;
        if !r.status().is_success() {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let err = RelayError::from_body(status, &body);
            if err.is_token_expired() && !refreshed {
                refreshed = true;
                shared.refresh_tokens(Some(&token)).await?;
                continue;
            }
            return Err(err.into());
        }
        let mime = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        return Ok((mime, r.bytes().await?.to_vec()));
    }
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
fn attachment_kind_str(kind: AttachmentKind) -> String {
    match kind {
        AttachmentKind::Image => "image".into(),
        AttachmentKind::Video => "video".into(),
        AttachmentKind::Slides => "slides".into(),
        AttachmentKind::Doc => "doc".into(),
        AttachmentKind::Sheet => "sheet".into(),
        AttachmentKind::Pdf => "pdf".into(),
        AttachmentKind::Audio => "audio".into(),
        AttachmentKind::File => "file".into(),
    }
}

fn user_attachment_from_part(att: Attachment) -> UserAttachment {
    UserAttachment {
        name: att.name,
        mime: att.mime,
        kind: att.kind.map(attachment_kind_str),
        blob_ref: att.blob_ref,
    }
}

/// An attachment on its way out: a file from disk, or a blob already on the
/// relay that is copied into this chat's slot (Forward from the media viewer).
enum Outgoing {
    Path(PathBuf),
    Copy {
        from_slot: String,
        attachment: Attachment,
    },
}

fn outgoing_paths(paths: Vec<PathBuf>) -> Vec<Outgoing> {
    paths.into_iter().map(Outgoing::Path).collect()
}

/// NewChat POST failed (or the body did not decode): drop the mint, notify, mark queued sends failed.
fn fail_pending_new_chat(shared: &Shared, local_id: &str, error: UserError) {
    shared.chats.lock().unwrap().retain(|c| c.id != local_id);
    let queued = shared
        .pending_new_chats
        .lock()
        .unwrap()
        .remove(local_id)
        .unwrap_or_default();
    if queued.is_empty() {
        shared.fail_with(error.clone());
    }
    for q in queued {
        let _ = shared.events.send(SyncEvent::MessageSendFailed {
            chat_id: local_id.to_owned(),
            message_id: q.message_id,
            error: error.clone(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_message(
    shared: Arc<Shared>,
    chat_id: Id,
    message_id: Id,
    text: String,
    attachments: Vec<Outgoing>,
    voice_wav: Option<Arc<Vec<u8>>>,
    command: &'static str,
    answer: Option<(String, serde_json::Value)>,
    reply_to: Option<String>,
    forwarded_from: Option<String>,
    reply_ref: Option<ReplyRef>,
    mentions: Vec<String>,
) {
    // No room doc yet: queue while NewChat is in flight, otherwise fail visibly.
    if !shared.docs.lock().unwrap().contains_key(&chat_id) {
        {
            let mut pending = shared.pending_new_chats.lock().unwrap();
            if let Some(queue) = pending.get_mut(&chat_id) {
                queue.push(QueuedSend {
                    message_id,
                    text,
                    attachments,
                    voice_wav,
                    command,
                    answer,
                    reply_to,
                    forwarded_from,
                    reply_ref,
                    mentions,
                });
                return;
            }
        }
        let error = UserError::from_text("send your message", "chat is not ready");
        let _ = shared.events.send(SyncEvent::MessageSendFailed {
            chat_id,
            message_id,
            error,
        });
        return;
    }

    let project_id = project_of(&shared, &chat_id).unwrap_or_default();
    // People chats keep their blobs under `people:<chat_id>`; project chats under the
    // project id (same slot agentd and the thumbnail fetch use). `blob_slot` expects a
    // project id or a people chat id — never a project chat's own id (the relay would
    // 403 the PUT and the attachment would land with no `ref`).
    let blob_slot = shared.blob_slot(if shared.is_people(&chat_id) {
        &chat_id
    } else {
        &project_id
    });
    let mut user_attachments: Vec<UserAttachment> = Vec::new();
    for outgoing in attachments {
        let (name, mime, kind, bytes) = match outgoing {
            Outgoing::Path(path) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("attachment")
                    .to_string();
                let mime = mime_for(&path).to_string();
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|err| format!("Could not read {}: {err}", path.display()));
                (name, mime, None, bytes)
            }
            Outgoing::Copy {
                from_slot,
                attachment,
            } => {
                let bytes = match attachment.blob_ref.as_deref() {
                    Some(key) => fetch_blob_bytes(&shared, &from_slot, key)
                        .await
                        .map(|(_, b)| b)
                        .map_err(|err| format!("Could not read {}: {err}", attachment.name)),
                    None => Err(format!("{} has no file on the relay", attachment.name)),
                };
                (
                    attachment.name,
                    attachment.mime,
                    attachment.kind.map(attachment_kind_str),
                    bytes,
                )
            }
        };
        let key = format!("att-{}-{}", new_id().to_lowercase(), safe_name(&name));
        let blob_ref = match bytes {
            Ok(bytes) => {
                let put = shared
                    .http
                    .put(format!("{}/blob/{}/{}", shared.http_url, blob_slot, key))
                    .bearer_auth(shared.token())
                    .header("content-type", mime.clone())
                    .body(bytes)
                    .send()
                    .await;
                match put {
                    Ok(r) if r.status().is_success() => Some(key),
                    Ok(r) => {
                        shared.fail("upload the attachment", r.status());
                        None
                    }
                    Err(err) => {
                        shared.fail("upload the attachment", &err);
                        None
                    }
                }
            }
            Err(err) => {
                shared.fail(&format!("attach {name}"), &err);
                None
            }
        };
        user_attachments.push(UserAttachment {
            name,
            mime,
            kind,
            blob_ref,
        });
    }
    if let Some(wav) = voice_wav {
        if let Part::Attachment { attachment, .. } =
            upload_voice_memo(&shared, &blob_slot, wav).await
        {
            user_attachments.push(user_attachment_from_part(attachment));
        }
    }
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
            // Room torn down between the early check and now — fail visibly.
            drop(docs);
            shared.unacked.lock().unwrap().remove(&message_id);
            let error = UserError::from_text("send your message", "chat is not ready");
            let _ = shared.events.send(SyncEvent::MessageSendFailed {
                chat_id,
                message_id,
                error,
            });
            return;
        };
        let me = shared.me_user();
        let mentions = if in_people {
            mentions_for(&shared, &chat_id, &text, &mentions)
        } else {
            Vec::new()
        };
        let opts = UserMessageOpts {
            attachments: user_attachments,
            mentions,
            reply_to,
            sender: if in_people {
                Some(format!("user:{}", shared.me_id()))
            } else {
                None
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
            answers_question_id: answer.as_ref().map(|(qid, _)| qid.clone()),
            tz: arbos_core::doc::device_time_zone(),
        };
        let payload = match &answer {
            Some((qid, value)) => {
                json!({ "messageId": message_id, "questionId": qid, "value": value, "text": text })
            }
            None => json!({ "messageId": message_id }),
        };
        let untitled = shared
            .chats
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.id == chat_id && !c.is_pinned && c.title == "New chat");
        let title = if untitled {
            Some(crate::app::title_from(&text))
        } else {
            None
        };
        let device = shared.device_id();
        doc.core.transact(|d| {
            let _ = d.append_user_message(&message_id, &text, &device, &opts);
            if let Some(by) = forwarded_from.as_deref() {
                d.set_message_fields(&message_id, &[("forwardedFrom", json!(by))]);
            }
            if let Some(r) = reply_ref.as_ref() {
                d.set_reply_ref(&message_id, r);
            }
            d.push_command(command, payload, &device);
            if let Some(ref title) = title {
                d.push_command("retitle", json!({ "title": title }), &device);
            }
        });
        if let Some(title) = title {
            if let Some(c) = shared
                .chats
                .lock()
                .unwrap()
                .iter_mut()
                .find(|c| c.id == chat_id)
            {
                c.title = title;
            }
            shared
                .local_changed_at
                .lock()
                .unwrap()
                .insert(chat_id.clone(), now_ms());
        }
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

/// `PUT /blob/{slot}/voice-<ulid>.wav`, then the `attachment` part that keeps the memo.
async fn upload_voice_memo(shared: &Shared, blob_slot: &str, wav: Arc<Vec<u8>>) -> Part {
    let key = format!("voice-{}.wav", new_id().to_lowercase());
    let put = shared
        .http
        .put(format!("{}/blob/{}/{}", shared.http_url, blob_slot, key))
        .bearer_auth(shared.token())
        .header("content-type", "audio/wav")
        .body(wav.as_ref().clone())
        .send()
        .await;
    let blob_ref = match put {
        Ok(r) if r.status().is_success() => Some(key),
        Ok(r) => {
            shared.fail("upload the voice memo", r.status());
            None
        }
        Err(err) => {
            shared.fail("upload the voice memo", &err);
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
            chart_part: None,
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
                let text = serde_json::from_str::<TranscriptionDto>(&body)
                    .map(|dto| transcript_text(&dto))
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
            let mut bytes = r.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            if status.is_success() {
                // Older relays pass OpenAI's streamed WAV through with placeholder
                // sizes; fix the header once here so both the decoder and the
                // rodio player (also hound underneath) take the cached bytes.
                arbos_core::wav::repair_sizes(&mut bytes);
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
    if handle_server_command(shared, &cmd) {
        return;
    }
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
                None,
                None,
                Vec::new(),
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
                        let dto: PairResponseDto = match serde_json::from_value(v) {
                            Ok(d) => d,
                            Err(_) => {
                                let _ = shared.events.send(SyncEvent::PairingFailed(
                                    UserError::from_text(
                                        "pair this Mac",
                                        "relay returned no token",
                                    ),
                                ));
                                return;
                            }
                        };
                        if dto.device_token.is_empty() {
                            let _ =
                                shared
                                    .events
                                    .send(SyncEvent::PairingFailed(UserError::from_text(
                                        "pair this Mac",
                                        "relay returned no token",
                                    )));
                            return;
                        }
                        let creds = DeviceCredentials {
                            relay_url: shared.http_url.clone(),
                            device_id: if dto.device_id.is_empty() {
                                "device".into()
                            } else {
                                dto.device_id
                            },
                            access_token: dto.device_token,
                            refresh_token: None,
                            expires_at: 0,
                            user: None,
                        };
                        if let Err(err) = credentials::store(&creds) {
                            shared.fail("save your sign-in to the keychain", &err);
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
                            .send(SyncEvent::PairingFailed(UserError::classify(
                                "pair this Mac",
                                &err,
                            )));
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
                            .send(SyncEvent::PairingFailed(UserError::classify(
                                "sign in", &err,
                            )));
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
                    c.unread_count = 0;
                }
            }
            shared
                .local_read_at
                .lock()
                .unwrap()
                .insert(chat_id.clone(), now_ms() as i64);
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
            // Always PUT /read so chat_events unread clears on other devices
            // (people DMs and project/agent chats). Mirrors iOS markRead /
            // markPeopleRead — local unread alone left the phone's bold dots.
            let seq = shared
                .room_seq
                .lock()
                .unwrap()
                .get(&chat_id)
                .copied()
                .unwrap_or(0);
            let shared = shared.clone();
            tokio::spawn(async move {
                let _ = shared
                    .put_json(&format!("/chats/{chat_id}/read"), json!({ "seq": seq }))
                    .await;
            });
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
                        let results: Vec<Person> = if let Ok(page) =
                            serde_json::from_value::<UsersPageDto>(v.clone())
                        {
                            page.users.iter().map(Person::from).collect()
                        } else if let Ok(cards) = serde_json::from_value::<Vec<UserCardDto>>(v) {
                            cards.iter().map(Person::from).collect()
                        } else {
                            Vec::new()
                        };
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleSearch { query, results });
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify("search", &err)));
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
                            shared.diagnostic("refresh", format!("{err:#}"));
                        }
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "start the chat",
                                &err,
                            )));
                    }
                }
            });
        }
        ClientCommand::CreatePeopleGroup {
            handles,
            agent_ids,
            title,
            leaf,
            picture,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let members: Vec<String> = handles
                    .iter()
                    .map(|h| h.trim().trim_start_matches('@').to_string())
                    .filter(|h| !h.is_empty())
                    .collect();
                // Agents go in the create body (one call, agents-only groups
                // included); a relay that ignores `agent_ids` leaves them out
                // of the answer, and they are added one by one as before.
                let mut body =
                    json!({ "kind": "group", "members": members, "agent_ids": agent_ids });
                if let Some(t) = title.filter(|t| !t.trim().is_empty()) {
                    body["title"] = json!(t.trim());
                }
                if let Some(l) = leaf {
                    body["avatar_leaf"] = json!(l.leaf);
                    body["avatar_leaf_color"] = json!(l.color);
                }
                match shared.post_json("/chats", body).await {
                    Ok(v) => {
                        let (chat_id, already) = match serde_json::from_value::<PeopleChatDto>(v) {
                            Ok(dto) => {
                                let agents: Vec<String> = dto
                                    .members
                                    .iter()
                                    .filter_map(|m| m.member.strip_prefix("agent:"))
                                    .map(str::to_owned)
                                    .collect();
                                (dto.id, agents)
                            }
                            Err(err) => {
                                log::warn!(
                                    target: "arbos_mac::relay",
                                    "CreatePeopleGroup PeopleChatDto: {err}"
                                );
                                (String::new(), Vec::new())
                            }
                        };
                        if let Some(picture) = picture.filter(|_| !chat_id.is_empty()) {
                            adopt_draft_picture(
                                &shared,
                                &format!("people:{chat_id}"),
                                &format!("/chats/{chat_id}"),
                                "avatar-",
                                picture,
                            )
                            .await;
                        }
                        for project_id in agent_ids {
                            if chat_id.is_empty() || already.contains(&project_id) {
                                continue;
                            }
                            if let Err(err) = shared
                                .post_json(
                                    &format!("/chats/{chat_id}/agents"),
                                    json!({ "project_id": project_id }),
                                )
                                .await
                            {
                                shared.fail("add the agent to the new group", &err);
                            }
                        }
                        if let Err(err) = refresh_registry(&shared).await {
                            shared.diagnostic("refresh", format!("{err:#}"));
                        }
                        if chat_id.is_empty() {
                            // The list refresh still shows the group; the sheet
                            // closes on the next Projects event by its members.
                            return;
                        }
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleGroupCreated { chat_id });
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "create the group",
                                &err,
                            )));
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
                            shared.diagnostic("refresh", format!("{err:#}"));
                        }
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "add the agent",
                                &err,
                            )));
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
                        .send(SyncEvent::PeopleError(UserError::classify(
                            "remove the agent",
                            &err,
                        )));
                }
                let _ = refresh_registry(&shared).await;
            });
        }
        ClientCommand::AddMembers {
            chat_id,
            handles,
            agent_ids,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                // One call for the whole batch (#1349): the relay adds everyone
                // and writes one "<me> added A, B and C" line in the chat. A
                // relay that only knows `{handle}` answers 4xx; then every add
                // is tried on its own as before.
                let batch = shared
                    .post_json(
                        &format!("/chats/{chat_id}/members"),
                        json!({ "handles": handles, "agent_ids": agent_ids }),
                    )
                    .await;
                let retry_singly = match &batch {
                    Ok(_) => false,
                    Err(err) => match err.downcast_ref::<RelayError>() {
                        Some(e) => matches!(e.status, 400 | 404 | 415 | 422),
                        None => false,
                    },
                };
                if !retry_singly {
                    if let Err(err) = refresh_registry(&shared).await {
                        shared.diagnostic("refresh", format!("{err:#}"));
                    }
                    if let Err(err) = batch {
                        shared.fail("add members", &err);
                    }
                    return;
                }
                // Every add is tried; the first failure is reported once the
                // rest are done, and the list is refreshed either way so the
                // pane shows exactly who made it in.
                let mut failed: Option<String> = None;
                for handle in handles {
                    if let Err(err) = shared
                        .post_json(
                            &format!("/chats/{chat_id}/members"),
                            json!({ "handle": handle }),
                        )
                        .await
                    {
                        failed.get_or_insert(format!("@{handle}: {}", people_error(&err)));
                    }
                }
                for project_id in agent_ids {
                    if let Err(err) = shared
                        .post_json(
                            &format!("/chats/{chat_id}/agents"),
                            json!({ "project_id": project_id }),
                        )
                        .await
                    {
                        failed.get_or_insert(format!("agent: {}", people_error(&err)));
                    }
                }
                if let Err(err) = refresh_registry(&shared).await {
                    shared.diagnostic("refresh", format!("{err:#}"));
                }
                if let Some(why) = failed {
                    shared.fail("add them", &why);
                }
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
                        .send(SyncEvent::PeopleError(UserError::classify(
                            "remove them",
                            &err,
                        )));
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
                        .send(SyncEvent::PeopleError(UserError::classify(
                            "leave the group",
                            &err,
                        )));
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
                    Ok((_status, v)) => match serde_json::from_value::<StartCallResponseDto>(v) {
                        Ok(resp) => {
                            let call = Call::from(&resp.call);
                            if call.call_id.is_empty() {
                                let _ = shared.events.send(SyncEvent::CallError {
                                    chat_id,
                                    error: UserError::from_text(
                                        "start the call",
                                        "The relay answered without a call.",
                                    ),
                                });
                            } else {
                                // 201 `{call, joined: false, reachable}` = I started it
                                // (and write the line).
                                let created = !resp.joined;
                                let reachable = if created {
                                    resp.reachable
                                        .unwrap_or_default()
                                        .iter()
                                        .map(Reachable::from)
                                        .collect()
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
                        }
                        Err(_) => {
                            let _ = shared.events.send(SyncEvent::CallError {
                                chat_id,
                                error: UserError::from_text(
                                    "start the call",
                                    "The relay answered without a call.",
                                ),
                            });
                        }
                    },
                    Err(err) => {
                        let _ = shared.events.send(SyncEvent::CallError {
                            chat_id,
                            error: call_error("start the call", &err),
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
                            error: call_error("join the call", &err),
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
                        let left = serde_json::from_value::<LeaveCallResponseDto>(v)
                            .map(|r| CallLeft {
                                ended: r.ended,
                                duration_s: r.duration_s.max(0) as u64,
                            })
                            .unwrap_or_default();
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
                    Err(err) => shared.fail("leave the call", &err),
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
                    Ok(_) => {
                        shared.notice(format!("@{handle} blocked."));
                        refresh_blocks(&shared).await;
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "block them",
                                &err,
                            )));
                    }
                }
            });
        }
        ClientCommand::UnblockUser { user_id, handle } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared.delete(&format!("/blocks/{user_id}")).await {
                    // 404 = already not blocked on the relay: the list below says so.
                    Ok(()) => shared.notice(format!("@{handle} unblocked.")),
                    Err(err) if format!("{err:#}").contains("HTTP 404") => {}
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "unblock them",
                                &err,
                            )));
                    }
                }
                refresh_blocks(&shared).await;
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
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "invite them",
                                &err,
                            )));
                    }
                }
            });
        }
        ClientCommand::SetMessageGrant {
            project_id,
            grantee,
            enabled,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .put_json(
                        &format!("/projects/{project_id}/message-access"),
                        json!({ "grantee": grantee, "enabled": enabled }),
                    )
                    .await
                {
                    Ok(_) => {
                        let _ = refresh_registry(&shared).await;
                    }
                    Err(err) => {
                        let _ = shared
                            .events
                            .send(SyncEvent::PeopleError(UserError::classify(
                                "give access",
                                &err,
                            )));
                    }
                }
            });
        }
        ClientCommand::RemoveMessageGrant {
            project_id,
            grantee,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/projects/{project_id}/message-access/{grantee}"))
                    .await
                {
                    let _ = shared
                        .events
                        .send(SyncEvent::PeopleError(UserError::classify(
                            "remove the access",
                            &err,
                        )));
                }
                let _ = refresh_registry(&shared).await;
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
                        .send(SyncEvent::PeopleError(UserError::classify(
                            "remove them",
                            &err,
                        )));
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
            reply_ref,
            mentions,
        } => {
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                outgoing_paths(attachments),
                None,
                "send",
                None,
                reply_to,
                None,
                reply_ref,
                mentions,
            ));
        }
        ClientCommand::Forward {
            chat_id,
            message_id,
            from_slot,
            attachment,
            text,
        } => {
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                vec![Outgoing::Copy {
                    from_slot,
                    attachment,
                }],
                None,
                "send",
                None,
                None,
                None,
                None,
                Vec::new(),
            ));
        }
        ClientCommand::ForwardText {
            chat_id,
            message_id,
            text,
            forwarded_from,
        } => {
            tokio::spawn(send_message(
                shared.clone(),
                chat_id,
                message_id,
                text,
                Vec::new(),
                None,
                "send",
                None,
                None,
                Some(forwarded_from),
                None,
                Vec::new(),
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
                None,
                None,
                Vec::new(),
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
                None,
                None,
                Vec::new(),
            ));
        }
        ClientCommand::Stop { chat_id } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let device = shared.device_id();
                doc.core.transact(|d| {
                    d.push_command("stop", json!({}), &device);
                });
            }
        }
        ClientCommand::StopJob {
            chat_id,
            job_id,
            message_id,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let device = shared.device_id();
                doc.core.transact(|d| {
                    d.push_command(
                        "stop",
                        json!({ "jobId": job_id, "messageId": message_id }),
                        &device,
                    );
                });
            }
        }
        ClientCommand::JobsRefresh { project_id } => {
            let shared = shared.clone();
            tokio::spawn(async move { refresh_project_jobs(&shared, &project_id).await });
        }
        ClientCommand::RolloutRefresh {
            project_id,
            job_id,
            after,
        } => {
            let shared = shared.clone();
            tokio::spawn(
                async move { refresh_job_rollout(&shared, &project_id, &job_id, after).await },
            );
        }
        ClientCommand::React {
            chat_id,
            message_id,
            emoji,
        } => {
            let me = format!("user:{}", shared.me_id());
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let already = doc
                    .index_of(&message_id)
                    .and_then(|ix| doc.read_message(ix))
                    .map(|m| {
                        m.reactions
                            .get(&emoji)
                            .and_then(|bucket| bucket.get(&me))
                            .copied()
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                let on = !already;
                if let Err(e) = doc
                    .core
                    .transact(|d| d.set_reaction(&message_id, &emoji, &me, on))
                {
                    eprintln!("relay: reaction: {e:#}");
                }
            }
        }
        ClientCommand::DeleteMessage {
            chat_id,
            message_id,
        } => {
            // The relay decides and edits the doc (`DELETE /room/{project}/{chat}/
            // messages/{id}`); the deletion comes back as a relay-signed row. A
            // refusal (403 `not_yours`) is told once; the message stays.
            let project_id = shared
                .chats
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.id == chat_id)
                .map(|c| c.project_id.clone())
                .unwrap_or_else(|| chat_id.clone());
            let is_people = shared.is_people(&chat_id);
            let room = room::room_path(&project_id, &chat_id, is_people);
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/room/{room}/messages/{message_id}"))
                    .await
                {
                    let error = match err.downcast_ref::<RelayError>() {
                        Some(e) if e.status == 403 => UserError::of(
                            "delete the message",
                            ErrorClass::Refused {
                                message: Some("You can only delete your own messages.".into()),
                            },
                            format!("{err:#}"),
                        ),
                        _ => UserError::classify("delete the message", &err),
                    };
                    shared.fail_with(error);
                }
            });
        }
        ClientCommand::Approve {
            chat_id,
            approval_id,
            grant,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let device = shared.device_id();
                let mut payload = json!({ "approvalId": approval_id });
                if let Some(g) = grant {
                    payload["grant"] = json!(g);
                }
                doc.core.transact(|d| {
                    d.push_command("approve", payload, &device);
                });
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
                let device = shared.device_id();
                doc.core.transact(|d| {
                    d.push_command("reject", payload, &device);
                });
            }
        }
        ClientCommand::Retry {
            chat_id,
            message_id,
        } => {
            let docs = shared.docs.lock().unwrap();
            if let Some(doc) = docs.get(&chat_id) {
                let device = shared.device_id();
                doc.core.transact(|d| {
                    d.push_command("retry", json!({ "messageId": message_id }), &device);
                });
            }
        }
        ClientCommand::RenameGroup { chat_id, title } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == chat_id) {
                    p.name = title.clone();
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            {
                let mut chats = shared.chats.lock().unwrap();
                if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
                    c.title = title.clone();
                }
            }
            emit_chats(shared);
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .patch_json(&format!("/chats/{chat_id}"), json!({ "title": title }))
                    .await
                {
                    shared.fail("rename the group", &err);
                }
            });
        }
        ClientCommand::SetMembersSeeReads { chat_id, enabled } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == chat_id) {
                    p.members_see_reads = enabled;
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .patch_json(
                        &format!("/chats/{chat_id}"),
                        json!({ "members_see_reads": enabled }),
                    )
                    .await
                {
                    shared.notice(format!(
                        "Read receipts setting not saved: {}",
                        people_error(&err)
                    ));
                }
            });
        }
        ClientCommand::SetGroupLeaf { chat_id, pick } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == chat_id) {
                    p.avatar_leaf = pick.map(|l| l.leaf);
                    p.avatar_leaf_color = pick.map(|l| l.color);
                    p.image_url = None;
                }
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                // `PATCH /chats` clears the picture on an empty key.
                let body = json!({
                    "avatar_leaf": pick.map(|l| l.leaf),
                    "avatar_leaf_color": pick.map(|l| l.color),
                    "image_key": "",
                });
                if let Err(err) = shared.patch_json(&format!("/chats/{chat_id}"), body).await {
                    shared.fail("save the group picture", &err);
                }
            });
        }
        ClientCommand::SetGroupImage { chat_id, path } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                // Same shape as the profile picture: the picture master, whatever was picked
                // (HEIC from Photos included), so every client renders it.
                let master = match tokio::task::spawn_blocking(move || picture_master(&path)).await
                {
                    Ok(Ok(m)) => m,
                    Ok(Err(err)) => {
                        shared.fail("save the group picture", &err);
                        return;
                    }
                    Err(err) => {
                        shared.fail("save the group picture", err.to_string());
                        return;
                    }
                };
                let key = format!("group-{}.{}", new_id().to_lowercase(), master.ext());
                let put = shared
                    .http
                    .put(format!(
                        "{}/blob/people:{}/{}",
                        shared.http_url, chat_id, key
                    ))
                    .bearer_auth(shared.token())
                    .header("content-type", master.mime())
                    .body(master.bytes)
                    .send()
                    .await;
                match put {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => {
                        shared.fail("upload the group picture", r.status());
                        return;
                    }
                    Err(err) => {
                        shared.fail("upload the group picture", &err);
                        return;
                    }
                }
                match shared
                    .patch_json(&format!("/chats/{chat_id}"), json!({ "image_key": key }))
                    .await
                {
                    Ok(v) => {
                        // The relay returns the chat: re-read it so the new
                        // picture URL reaches every row at once.
                        if let Ok(dto) = serde_json::from_value::<PeopleChatDto>(v) {
                            let (space, _) = people_space_from(
                                &dto,
                                &shared.me_id(),
                                &shared.me_handle(),
                                &shared.http_url,
                                &shared.token(),
                            );
                            let mut projects = shared.projects.lock().unwrap();
                            if let Some(p) = projects.iter_mut().find(|p| p.id == chat_id) {
                                p.image_url = space.image_url;
                            }
                            let snapshot = projects.clone();
                            drop(projects);
                            let _ = shared.events.send(SyncEvent::Projects(snapshot));
                        }
                    }
                    Err(err) => {
                        shared.fail("save the group picture", &err);
                    }
                }
            });
        }
        ClientCommand::Retitle {
            project_id,
            chat_id,
            title,
        } => {
            {
                let mut chats = shared.chats.lock().unwrap();
                if let Some(c) = chats.iter_mut().find(|c| c.id == chat_id) {
                    c.title = title.clone();
                }
            }
            shared
                .local_changed_at
                .lock()
                .unwrap()
                .insert(chat_id.clone(), now_ms());
            emit_chats(shared);
            {
                let docs = shared.docs.lock().unwrap();
                if let Some(doc) = docs.get(&chat_id) {
                    let device = shared.device_id();
                    doc.core.transact(|d| {
                        d.push_command("retitle", json!({ "title": title }), &device);
                    });
                }
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .patch_json(
                        &format!("/projects/{project_id}/chats/{chat_id}"),
                        json!({ "title": title }),
                    )
                    .await
                {
                    shared.fail("rename the chat", &err);
                }
            });
        }
        ClientCommand::NewChat {
            project_id,
            chat_id: local_id,
        } => {
            // Mint stays in the registry until POST answers (refresh must not drop it).
            {
                let mut chats = shared.chats.lock().unwrap();
                if !chats.iter().any(|c| c.id == local_id) {
                    chats.push(Chat {
                        id: local_id.clone(),
                        project_id: project_id.clone(),
                        title: "New chat".into(),
                        is_pinned: false,
                        is_system: false,
                        kind: crate::model::ChatKind::Chat,
                        participants: Vec::new(),
                        unread: false,
                        unread_count: 0,
                        last_message_at: now_ms(),
                        archived_at: None,
                    });
                }
            }
            shared
                .pending_new_chats
                .lock()
                .unwrap()
                .insert(local_id.clone(), Vec::new());
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
                        let chat = match serde_json::from_value::<CreateChatResponseDto>(v) {
                            Ok(dto) => chat_from_create(&dto),
                            Err(err) => {
                                log::warn!(
                                    target: "arbos_mac::relay",
                                    "CreateChatResponseDto: {err}"
                                );
                                fail_pending_new_chat(
                                    &shared,
                                    &local_id,
                                    UserError::from_text(
                                        "start the chat",
                                        "Relay returned no chat",
                                    ),
                                );
                                return;
                            }
                        };
                        {
                            let mut chats = shared.chats.lock().unwrap();
                            chats.retain(|c| c.id != local_id);
                            chats.push(chat.clone());
                        }
                        ensure_room(&shared, &project_id, &chat.id, &chat.id);
                        let queued = shared
                            .pending_new_chats
                            .lock()
                            .unwrap()
                            .remove(&local_id)
                            .unwrap_or_default();
                        let message_ids: Vec<Id> =
                            queued.iter().map(|q| q.message_id.clone()).collect();
                        let targets = remap_queued_send_targets(&chat.id, &message_ids);
                        let _ = shared.events.send(SyncEvent::ChatCreated {
                            project_id: project_id.clone(),
                            local_id,
                            chat: chat.clone(),
                        });
                        for ((real_id, _), q) in targets.into_iter().zip(queued) {
                            tokio::spawn(send_message(
                                shared.clone(),
                                real_id,
                                q.message_id,
                                q.text,
                                q.attachments,
                                q.voice_wav,
                                q.command,
                                q.answer,
                                q.reply_to,
                                q.forwarded_from,
                                q.reply_ref,
                                q.mentions,
                            ));
                        }
                    }
                    Err(err) => {
                        fail_pending_new_chat(
                            &shared,
                            &local_id,
                            UserError::classify("start the chat", &err),
                        );
                    }
                }
            });
        }
        ClientCommand::ForkChat {
            project_id,
            source_chat_id,
            chat_id: local_id,
            title,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .post_json(
                        &format!("/projects/{project_id}/chats"),
                        json!({ "title": title, "fork_from": source_chat_id }),
                    )
                    .await
                {
                    Ok(v) => {
                        let chat = match serde_json::from_value::<CreateChatResponseDto>(v) {
                            Ok(dto) => chat_from_create(&dto),
                            Err(err) => {
                                log::warn!(
                                    target: "arbos_mac::relay",
                                    "CreateChatResponseDto (fork): {err}"
                                );
                                shared.fail("fork the chat", "Relay returned no chat");
                                return;
                            }
                        };
                        shared.chats.lock().unwrap().push(chat.clone());
                        ensure_room(&shared, &project_id, &chat.id, &chat.id);
                        let _ = shared.events.send(SyncEvent::ChatCreated {
                            project_id: project_id.clone(),
                            local_id,
                            chat,
                        });
                    }
                    Err(err) => shared.fail("fork the chat", &err),
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
            shared
                .local_changed_at
                .lock()
                .unwrap()
                .insert(chat_id.clone(), now_ms());
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
                            shared.fail("archive the chat", &err);
                        }
                    }
                }
            });
        }
        ClientCommand::SetAvatarLeaf {
            project_id,
            leaf,
            color,
        } => {
            {
                // Optimistic, into every copy (sidebar record and each roster) at once; a leaf
                // pick is "not a photo": the picture goes with it.
                let mut projects = shared.projects.lock().unwrap();
                let local = super::agent_avatar::newest(&projects, &project_id)
                    .map(|f| f.version + 1)
                    .unwrap_or(1);
                super::agent_avatar::apply(
                    &mut projects,
                    &project_id,
                    &super::agent_avatar::Facts {
                        image_url: None,
                        leaf,
                        leaf_color: color,
                        version: local,
                    },
                    true,
                );
                let snapshot = projects.clone();
                drop(projects);
                let _ = shared.events.send(SyncEvent::Projects(snapshot));
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                let body =
                    json!({ "avatar_leaf": leaf, "avatar_leaf_color": color, "image_key": null });
                match shared
                    .patch_json(&format!("/projects/{project_id}"), body)
                    .await
                {
                    Ok(reply) => {
                        let stamp = super::agent_avatar::stamp_of(&reply);
                        if stamp > 0 {
                            let mut projects = shared.projects.lock().unwrap();
                            super::agent_avatar::apply(
                                &mut projects,
                                &project_id,
                                &super::agent_avatar::Facts {
                                    image_url: None,
                                    leaf,
                                    leaf_color: color,
                                    version: stamp,
                                },
                                true,
                            );
                            let snapshot = projects.clone();
                            drop(projects);
                            let _ = shared.events.send(SyncEvent::Projects(snapshot));
                        }
                    }
                    Err(err) => {
                        // An older relay (no D92 columns) ignores unknown fields and answers
                        // 200; a 404 means no such project. Anything else is worth a line.
                        if !err.to_string().contains("HTTP 404") {
                            shared.fail("save the picture", &err);
                        }
                    }
                }
            });
        }
        ClientCommand::SetProjectImage { project_id, source } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let key = match source {
                    PictureSource::Key(key) => key,
                    PictureSource::File(path) => {
                        let master = match tokio::task::spawn_blocking(move || {
                            picture_master(&path)
                        })
                        .await
                        {
                            Ok(Ok(m)) => m,
                            Ok(Err(err)) => {
                                shared.fail("upload the picture", &err);
                                return;
                            }
                            Err(_) => return,
                        };
                        let key = format!("avatar-{}.{}", new_id().to_lowercase(), master.ext());
                        let put = shared
                            .http
                            .put(format!("{}/blob/{}/{}", shared.http_url, project_id, key))
                            .bearer_auth(shared.token())
                            .header("content-type", master.mime())
                            .body(master.bytes)
                            .send()
                            .await;
                        match put {
                            Ok(r) if r.status().is_success() => key,
                            Ok(r) => {
                                shared.fail("upload the picture", r.status());
                                return;
                            }
                            Err(err) => {
                                shared.fail("upload the picture", &err);
                                return;
                            }
                        }
                    }
                };
                match shared
                    .patch_json(
                        &format!("/projects/{project_id}"),
                        json!({ "image_key": key }),
                    )
                    .await
                {
                    Ok(reply) => {
                        // The relay's stamp names the new URL; it lands in my record and in
                        // every roster copy, so the bubbles and the sidebar agree at once.
                        let mut projects = shared.projects.lock().unwrap();
                        let base =
                            super::agent_avatar::newest(&projects, &project_id).unwrap_or_default();
                        let stamp = super::agent_avatar::stamp_of(&reply).max(base.version + 1);
                        let url =
                            project_image_url(&shared.http_url, &project_id, Some(&key), stamp);
                        super::agent_avatar::apply(
                            &mut projects,
                            &project_id,
                            &super::agent_avatar::Facts {
                                image_url: url,
                                leaf: base.leaf,
                                leaf_color: base.leaf_color,
                                version: stamp,
                            },
                            true,
                        );
                        let snapshot = projects.clone();
                        drop(projects);
                        let _ = shared.events.send(SyncEvent::Projects(snapshot));
                    }
                    Err(err) => {
                        shared.fail("save the picture", &err);
                    }
                }
            });
        }
        ClientCommand::GenerateImage { target, prompt } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let path = match &target {
                    PictureTarget::Agent(id) => format!("/projects/{id}/avatar/generate"),
                    PictureTarget::Group(id) => format!("/chats/{id}/avatar/generate"),
                    PictureTarget::Server(id) => format!("/servers/{id}/avatar/generate"),
                    // The Create sheet: into my own slot (#1789's route); copied on Create.
                    PictureTarget::Draft | PictureTarget::Me(_) => "/me/avatar/generate".to_owned(),
                };
                let result = shared
                    .post_json_timeout(
                        &path,
                        json!({ "prompt": prompt }),
                        Some(AVATAR_GENERATE_TIMEOUT),
                    )
                    .await;
                let event = match result {
                    Ok(v) => {
                        let key = v
                            .get("image_key")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        match key {
                            Some(key) => SyncEvent::ImageGenerated {
                                url: picture_url(&shared, &target, Some(&key)),
                                target,
                                image_key: Some(key),
                                error: None,
                            },
                            None => SyncEvent::ImageGenerated {
                                target,
                                image_key: None,
                                url: None,
                                error: Some(UserError::from_text(
                                    "generate the image",
                                    "The relay sent no picture",
                                )),
                            },
                        }
                    }
                    Err(err) => {
                        let error = generate_failure(&target, &err);
                        SyncEvent::ImageGenerated {
                            target,
                            image_key: None,
                            url: None,
                            error: Some(error),
                        }
                    }
                };
                let _ = shared.events.send(event);
            });
        }
        ClientCommand::SetGroupImageKey { chat_id, key } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match shared
                    .patch_json(&format!("/chats/{chat_id}"), json!({ "image_key": key }))
                    .await
                {
                    Ok(v) => {
                        if let Ok(dto) = serde_json::from_value::<PeopleChatDto>(v) {
                            let (space, _) = people_space_from(
                                &dto,
                                &shared.me_id(),
                                &shared.me_handle(),
                                &shared.http_url,
                                &shared.token(),
                            );
                            let mut projects = shared.projects.lock().unwrap();
                            if let Some(p) = projects.iter_mut().find(|p| p.id == chat_id) {
                                p.image_url = space.image_url;
                            }
                            let snapshot = projects.clone();
                            drop(projects);
                            let _ = shared.events.send(SyncEvent::Projects(snapshot));
                        }
                    }
                    Err(err) => {
                        shared.fail("save the group picture", &err);
                    }
                }
            });
        }
        ClientCommand::SetServerImage { server_id, source } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let key = match source {
                    PictureSource::Key(key) => key,
                    PictureSource::File(path) => {
                        let master = match tokio::task::spawn_blocking(move || {
                            picture_master(&path)
                        })
                        .await
                        {
                            Ok(Ok(m)) => m,
                            Ok(Err(err)) => {
                                shared.fail("upload the picture", &err);
                                return;
                            }
                            Err(_) => return,
                        };
                        let key = format!("server-{}.{}", new_id().to_lowercase(), master.ext());
                        let put = shared
                            .http
                            .put(format!(
                                "{}/blob/servers:{}/{}",
                                shared.http_url, server_id, key
                            ))
                            .bearer_auth(shared.token())
                            .header("content-type", master.mime())
                            .body(master.bytes)
                            .send()
                            .await;
                        match put {
                            Ok(r) if r.status().is_success() => key,
                            Ok(r) => {
                                shared.fail("upload the picture", r.status());
                                return;
                            }
                            Err(err) => {
                                shared.fail("upload the picture", &err);
                                return;
                            }
                        }
                    }
                };
                match shared
                    .patch_json(
                        &format!("/servers/{server_id}"),
                        json!({ "image_key": key }),
                    )
                    .await
                {
                    Ok(_) => refresh_servers_list(&shared).await,
                    Err(err) => {
                        shared.fail("save the server picture", &err);
                    }
                }
            });
        }
        ClientCommand::SetServerLeaf { server_id, pick } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let body = json!({
                    "avatar_leaf": pick.map(|l| l.leaf),
                    "avatar_leaf_color": pick.map(|l| l.color),
                    "image_key": null,
                });
                match shared
                    .patch_json(&format!("/servers/{server_id}"), body)
                    .await
                {
                    Ok(_) => refresh_servers_list(&shared).await,
                    Err(err) => {
                        shared.fail("save the server picture", &err);
                    }
                }
            });
        }
        ClientCommand::SetMyAvatar { path } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                // The relay takes a JPEG or PNG of at most 3 MB: whatever was picked becomes
                // the ≤ 1024 px master (`sync::avatar_master`).
                let master = match tokio::task::spawn_blocking(move || picture_master(&path)).await
                {
                    Ok(Ok(m)) => m,
                    Ok(Err(err)) => {
                        shared.fail("save your picture", &err);
                        return;
                    }
                    Err(err) => {
                        shared.fail("save your picture", err.to_string());
                        return;
                    }
                };
                let mut put = shared
                    .http
                    .put(format!("{}/me/avatar", shared.http_url))
                    .bearer_auth(shared.token())
                    .header("content-type", master.mime())
                    .body(master.bytes.clone())
                    .send()
                    .await;
                // A relay from before the PNG master (415 "stored as JPEG"): send the same
                // picture as JPEG q92 instead of a notice.
                if master.png
                    && matches!(&put, Ok(r) if r.status() == reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE)
                {
                    if let Ok(Ok(jpeg)) =
                        tokio::task::spawn_blocking(move || master.to_jpeg()).await
                    {
                        put = shared
                            .http
                            .put(format!("{}/me/avatar", shared.http_url))
                            .bearer_auth(shared.token())
                            .header("content-type", jpeg.mime())
                            .body(jpeg.bytes)
                            .send()
                            .await;
                    }
                }
                my_avatar_saved(&shared, put).await;
            });
        }
        ClientCommand::SetMyAvatarKey { key } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let put = shared
                    .http
                    .put(format!("{}/me/avatar", shared.http_url))
                    .bearer_auth(shared.token())
                    .json(&json!({ "image_key": key }))
                    .send()
                    .await;
                my_avatar_saved(&shared, put).await;
            });
        }
        ClientCommand::RemoveMyAvatar => {
            let shared = shared.clone();
            tokio::spawn(async move {
                let resp = shared
                    .http
                    .delete(format!("{}/me/avatar", shared.http_url))
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                match resp {
                    Ok(r) if r.status().is_success() => {
                        if let Ok(body) = r.json::<Value>().await {
                            if let Ok(dto) = serde_json::from_value::<MeUserResponseDto>(body) {
                                let u: credentials::AccountUser = (&dto.user).into();
                                shared.with_store(|s| s.put_json(store::KEY_ACCOUNT, &u));
                                *shared.me.lock().unwrap() = Some(u);
                            }
                        }
                        refresh_me(&shared).await;
                    }
                    Ok(r) => shared.fail("remove your picture", r.status()),
                    Err(err) => shared.fail("remove your picture", &err),
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
                    Ok(v) => {
                        if let Ok(dto) = serde_json::from_value::<MeUserResponseDto>(v) {
                            let u: credentials::AccountUser = (&dto.user).into();
                            shared.with_store(|s| s.put_json(store::KEY_ACCOUNT, &u));
                            *shared.me.lock().unwrap() = Some(u);
                        }
                        refresh_me(&shared).await;
                    }
                    Err(err) => {
                        let said =
                            err.downcast_ref::<RelayError>()
                                .and_then(|e| match e.kind.as_str() {
                                    "handle_taken" => Some("That handle is already taken."),
                                    "bad_handle" => {
                                        Some("Handle must be 1–39 letters, numbers, or hyphens.")
                                    }
                                    _ => None,
                                });
                        let error = match said {
                            Some(why) => UserError::of(
                                "save your profile",
                                ErrorClass::Refused {
                                    message: Some(why.to_owned()),
                                },
                                format!("{err:#}"),
                            ),
                            None => UserError::classify("save your profile", &err),
                        };
                        shared.fail_with(error);
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
                    shared.fail("save the description", &err);
                }
            });
        }
        ClientCommand::SetProjectName { project_id, name } => {
            {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == project_id) {
                    p.name = name.clone();
                }
                // Keep people-group @mention handles in sync with the new name.
                let handle = Project::mention_slug(&name);
                for space in projects.iter_mut().filter(|p| p.is_people()) {
                    if let Some(a) = space.agents.iter_mut().find(|a| a.project_id == project_id) {
                        a.name = name.clone();
                        a.handle = handle.clone();
                    }
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
                    shared.fail("rename the agent", &err);
                }
            });
        }
        ClientCommand::ArchiveSpace {
            space_id,
            server,
            archived,
        } => {
            // Optimistic in the project cache the next refresh is built from
            // (servers come straight from `GET /servers`, which carries the flag).
            if !server {
                let mut projects = shared.projects.lock().unwrap();
                if let Some(p) = projects.iter_mut().find(|p| p.id == space_id) {
                    p.archived_at = archived.then(now_ms);
                }
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                let path = if server {
                    format!("/servers/{space_id}/archive")
                } else {
                    format!("/projects/{space_id}/archive")
                };
                if let Err(err) = shared
                    .put_json(&path, json!({ "archived": archived }))
                    .await
                {
                    // Older relay: the Mac keeps its local state until the next refresh.
                    if !err.to_string().contains("HTTP 404") {
                        shared.fail("archive the chat", &err);
                    }
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
                    shared.fail("change the mute setting", &err);
                }
            });
        }
        ClientCommand::SetReadReceipts { enabled, persist } => {
            shared.read_receipts.store(enabled, Ordering::Relaxed);
            // Refresh presence so peers drop or pick up our `seen` at once.
            for room in shared.rooms.lock().unwrap().values() {
                let _ = room.cmds.send(RoomCmd::Presence);
            }
            if persist {
                // Mirror onto every people chat we know (`PUT …/settings`).
                let people: Vec<Id> = shared.people.lock().unwrap().iter().cloned().collect();
                let shared = shared.clone();
                tokio::spawn(async move {
                    for chat_id in people {
                        let _ = shared
                            .put_json(
                                &format!("/chats/{chat_id}/settings"),
                                json!({ "read_receipts": enabled }),
                            )
                            .await;
                    }
                });
            }
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
                                    shared.fail("play the voice sample", &err);
                                    return;
                                }
                            }
                            Err(err) => {
                                shared.fail("play the voice sample", &err);
                                return;
                            }
                        },
                        Ok(r) => {
                            shared.fail("play the voice sample", r.status());
                            return;
                        }
                        Err(err) => {
                            shared.fail("play the voice sample", &err);
                            return;
                        }
                    }
                }
                let _ = shared.events.send(SyncEvent::VoiceSample { url, path });
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
                    Err(err) => shared.fail("save the voice", &err),
                }
            });
        }
        ClientCommand::SetVision {
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
                    Err(err) => shared.fail("save the camera setting", &err),
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
                    let effort_s = effort_to(effort);
                    doc.core.transact(|d| {
                        let _ = d.set_model_settings(&model, effort_s);
                    });
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
                    Ok(r) => shared.fail("change the model", r.status()),
                    Err(err) => shared.fail("change the model", &err),
                }
            });
        }
        ClientCommand::CreateProject {
            name,
            avatar_color_index,
            leaf,
            picture,
            source_project_id,
            description,
            color,
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
                if let Some(l) = leaf {
                    body["avatar_leaf"] = json!(l.leaf);
                    body["avatar_leaf_color"] = json!(l.color);
                }
                if let Some(src) = source_project_id {
                    body["source_project_id"] = json!(src);
                }
                if let Some(d) = description.filter(|s| !s.is_empty()) {
                    body["description"] = json!(d);
                }
                if let Some(c) = color.filter(|s| !s.is_empty()) {
                    body["color"] = json!(c);
                }
                match shared.post_json("/projects", body).await {
                    Ok(v) => {
                        // The Create sheet's picture goes into the new agent's own slot first,
                        // so the list refresh already shows it.
                        if let Some(picture) = picture {
                            if let Some(id) = v.get("id").and_then(Value::as_str) {
                                adopt_draft_picture(
                                    &shared,
                                    id,
                                    &format!("/projects/{id}"),
                                    "avatar-",
                                    picture,
                                )
                                .await;
                            }
                        }
                        if let Err(err) = refresh_registry(&shared).await {
                            shared.diagnostic("refresh", format!("{err:#}"));
                        }
                    }
                    Err(err) => shared.fail("create the agent", &err),
                }
            });
        }
        ClientCommand::DeleteProject { project_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&format!("/projects/{project_id}")).await {
                    shared.fail("delete the agent", &err);
                }
                if let Err(err) = refresh_registry(&shared).await {
                    shared.diagnostic("refresh", format!("{err:#}"));
                }
            });
        }
        ClientCommand::DeletePeopleChat { chat_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&format!("/chats/{chat_id}")).await {
                    let _ = shared
                        .events
                        .send(SyncEvent::PeopleError(UserError::classify(
                            "delete the chat",
                            &err,
                        )));
                }
                shared.people.lock().unwrap().remove(&chat_id);
                shared.with_store(|s| s.delete_doc(&chat_id));
                let _ = refresh_registry(&shared).await;
            });
        }
        ClientCommand::CloseChat {
            project_id,
            chat_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/projects/{project_id}/chats/{chat_id}"))
                    .await
                {
                    shared.fail("delete the chat", &err);
                }
                if let Err(err) = refresh_registry(&shared).await {
                    shared.diagnostic("refresh", format!("{err:#}"));
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
                    shared.fail("restart the machine", &err);
                }
                match shared.post_json("/machine/start", json!({})).await {
                    Ok(v) => {
                        if let Ok(dto) = serde_json::from_value::<MachineStateDto>(v) {
                            set_machine_from_reply(&shared, &dto);
                        }
                    }
                    Err(err) => shared.fail("restart the machine", &err),
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
                        if let Ok(dto) = serde_json::from_value::<MachineStateDto>(v) {
                            set_machine_from_reply(&shared, &dto);
                        }
                    }
                    Err(err) => shared.fail(&format!("{verb} the machine"), &err),
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
                    .get(format!(
                        "{}/blob/{}/{}",
                        shared.http_url,
                        shared.blob_slot(&project_id),
                        key
                    ))
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
                                    shared.fail("save the file", &err);
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
                    Ok(r) => shared.fail("download the file", r.status()),
                    Err(err) => shared.fail("download the file", &err),
                }
            });
        }
        ClientCommand::FetchLinkIcon { host } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                // 200 = the PNG; 204 = the site has none; anything else is treated as none
                // too (the bubble keeps its placeholder) and only logged — nobody asked.
                let resp = shared
                    .http
                    .get(format!("{}/link-icon", shared.http_url))
                    .query(&[("domain", host.as_str())])
                    .bearer_auth(shared.token())
                    .send()
                    .await;
                let png = match resp {
                    Ok(r) if r.status() == reqwest::StatusCode::OK => {
                        r.bytes().await.ok().map(|b| Arc::new(b.to_vec()))
                    }
                    Ok(r) if r.status() == reqwest::StatusCode::NO_CONTENT => None,
                    Ok(r) => {
                        log::warn!(target: "arbos_mac::diagnostic", "link icon {host}: HTTP {}", r.status());
                        None
                    }
                    Err(err) => {
                        log::warn!(target: "arbos_mac::diagnostic", "link icon {host}: {err}");
                        None
                    }
                };
                let _ = shared.events.send(SyncEvent::LinkIcon { host, png });
            });
        }
        ClientCommand::FetchThumbnail { project_id, key } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                // A brief send/timeout on an image preview is common on flaky Wi-Fi; retry
                // twice before "No preview". Transient errors only log (see is_transient).
                let result = {
                    let mut attempt = 0u32;
                    loop {
                        match fetch_blob_bytes(&shared, &project_id, &key).await {
                            Ok(ok) => break Ok(ok),
                            Err(err) => {
                                let text = format!("{err:#}");
                                if diagnostics::is_transient(&text) && attempt < 2 {
                                    attempt += 1;
                                    sleep(Duration::from_millis(400 * u64::from(attempt))).await;
                                    continue;
                                }
                                break Err(err);
                            }
                        }
                    }
                };
                match result {
                    Ok((_, bytes)) => {
                        let _ = shared.events.send(SyncEvent::Thumbnail {
                            key,
                            bytes: Arc::new(bytes),
                        });
                    }
                    Err(err) => {
                        // Nobody asked for this fetch: no toast, no bubble. The card shows
                        // "No preview". A missing blob (deleted, never uploaded, stale ref)
                        // is not a Mac bug — same quiet 404 treatment as routines / voices.
                        // Other failures still go to the silent weekly report.
                        let detail = format!("{key}: {err:#}");
                        if detail.to_ascii_lowercase().contains("http 404") {
                            log::warn!(target: "arbos_mac::diagnostic", "thumbnail: {detail}");
                        } else {
                            shared.diagnostic("thumbnail", detail);
                        }
                        let _ = shared.events.send(SyncEvent::ThumbnailFailed { key });
                    }
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
                            Err(err) => shared.fail(&format!("save {name}"), &err),
                        }
                    }
                    Err(err) => {
                        // Raw "Download …: HTTP 403" sat in the chat like a message. Keep a
                        // short dismissible toast; the status detail goes to diagnostics.
                        shared.diagnostic("download", format!("{name}: {err}"));
                        shared.fail(&format!("download {name}"), &err);
                    }
                }
            });
        }
        ClientCommand::OpenAttachment {
            project_id,
            key,
            name,
            quick_look,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                match fetch_blob_bytes(&shared, &project_id, &key).await {
                    Ok((_, bytes)) => {
                        let dir = std::env::temp_dir().join("arbos-media");
                        let _ = tokio::fs::create_dir_all(&dir).await;
                        let path = dir.join(safe_name(&name));
                        match tokio::fs::write(&path, &bytes).await {
                            Ok(()) => crate::ui::attachments::open_with_system(&path, quick_look),
                            Err(err) => shared.fail(&format!("open {name}"), &err),
                        }
                    }
                    Err(err) => {
                        shared.diagnostic("open", format!("{name}: {err}"));
                        shared.fail(&format!("open {name}"), &err);
                    }
                }
                let _ = shared.events.send(SyncEvent::AttachmentOpened { key });
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
                        match serde_json::from_value::<ConnectConnectorReplyDto>(v) {
                            Ok(ConnectConnectorReplyDto {
                                auth_url: Some(url),
                                ..
                            }) => {
                                let _ = shared.events.send(SyncEvent::ConnectorAuth {
                                    provider: provider.clone(),
                                    auth_url: url,
                                });
                            }
                            Ok(ConnectConnectorReplyDto {
                                connector: Some(c), ..
                            }) => {
                                if c.status == "connected" {
                                    shared.notice(format!("{provider} connected."));
                                } else {
                                    shared.fail(
                                        &format!("connect {provider}"),
                                        format!(
                                            "{} {}",
                                            c.status,
                                            c.last_error.as_deref().unwrap_or("")
                                        ),
                                    );
                                }
                            }
                            Ok(_) => {}
                            Err(err) => shared.diagnostic("connectors", format!("{err:#}")),
                        }
                        // The loopback callback lands out of band: poll the list for a while.
                        for wait in [0u64, 4, 8, 15, 30, 60] {
                            sleep(Duration::from_secs(wait)).await;
                            refresh_connectors(&shared).await;
                        }
                    }
                    Err(err) => shared.fail(&format!("connect {provider}"), &err),
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
                    Ok(r) => shared.fail("disconnect it", r.status()),
                    Err(err) => shared.fail("disconnect it", &err),
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
        ClientCommand::DevicesRefresh => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_devices(&shared).await;
            });
        }
        ClientCommand::DeviceRevoke { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&format!("/auth/devices/{id}")).await {
                    shared.fail("sign out that device", &err);
                }
                refresh_devices(&shared).await;
            });
        }
        ClientCommand::ProjectSecretsRefresh { project_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_project_secrets(&shared, &project_id).await;
            });
        }
        ClientCommand::ShellGrantsRefresh { project_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                refresh_shell_grants(&shared, &project_id).await;
            });
        }
        ClientCommand::ShellGrantRevoke {
            project_id,
            grant_id,
        } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/projects/{project_id}/shell-grants/{grant_id}"))
                    .await
                {
                    shared.fail("change the shell approvals", &err);
                }
                refresh_shell_grants(&shared, &project_id).await;
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
                        let env = serde_json::from_value::<SecretEnvelopeDto>(v)
                            .ok()
                            .map(|e| e.secret.env)
                            .unwrap_or_else(|| item.name.clone());
                        shared.notice(format!("Secrets: saved {env}."));
                    }
                    Err(err) => shared.fail("save the secret", &err),
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretPatch { id, body } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.patch_json(&format!("/secrets/{id}"), body).await {
                    shared.fail("save the secret", &err);
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
                    Err(err) => shared.fail("replace the value", &err),
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretDelete { id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared.delete(&format!("/secrets/{id}")).await {
                    shared.fail("delete the secret", &err);
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
                    shared.fail("change who may use it", &err);
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
                    shared.fail("revoke the access", &err);
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretPerson {
            id,
            user_id,
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
                    .put_json(&format!("/secrets/{id}/people/{user_id}"), body)
                    .await
                {
                    shared.fail("change that person's access", &err);
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretPeopleAdd { id, handles } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                for handle in handles {
                    if let Err(err) = shared
                        .post_json(
                            &format!("/secrets/{id}/people"),
                            json!({ "handle": handle }),
                        )
                        .await
                    {
                        shared.fail(&format!("add @{handle}"), &err);
                    }
                }
                refresh_secrets(&shared).await;
            });
        }
        ClientCommand::SecretPersonRemove { id, user_id } => {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(err) = shared
                    .delete(&format!("/secrets/{id}/people/{user_id}"))
                    .await
                {
                    shared.fail("remove that person", &err);
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
                    shared.fail("change the pause", &err);
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
                        let (name, agent) = serde_json::from_value::<DecideSecretResponseDto>(v)
                            .ok()
                            .map(|d| {
                                (
                                    d.request.name,
                                    d.request.project_name.unwrap_or_else(|| "the agent".into()),
                                )
                            })
                            .unwrap_or_else(|| ("the secret".into(), "the agent".into()));
                        shared.notice(match decision.as_str() {
                            "deny" => format!("{agent} asked for {name}. Denied."),
                            "once" => format!("{agent} may use {name} once."),
                            "chat" => format!("{agent} may use {name} in this chat."),
                            _ => format!("{agent} may use {name}."),
                        });
                    }
                    Err(err) => shared.fail("answer the request", &err),
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
                    Ok(v) => {
                        let name = serde_json::from_value::<VaultEnvelopeDto>(v)
                            .ok()
                            .map(|e| e.vault.name)
                            .unwrap_or_else(|| "the vault".into());
                        shared.notice(format!("Connected {name}."));
                    }
                    Err(err) => {
                        let text = format!("{err:#}");
                        let said = if text.contains("bad_token") {
                            Some("That token didn't work. Check it was copied whole.")
                        } else if text.contains("no_vaults") {
                            Some("This token can read no vaults.")
                        } else {
                            None
                        };
                        shared.fail_with(match said {
                            Some(why) => UserError::of(
                                "connect 1Password",
                                ErrorClass::Refused {
                                    message: Some(why.to_owned()),
                                },
                                text,
                            ),
                            None if text.contains("rate_limited") => {
                                UserError::of("connect 1Password", ErrorClass::RateLimited, text)
                            }
                            None => UserError::classify("connect 1Password", &err),
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
                    shared.fail("change the vault access", &err);
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
                    shared.fail("save the item name", &err);
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
                        shared.fail_with(if text.contains("bad_token") {
                            UserError::of(
                                "replace the token",
                                ErrorClass::Refused {
                                    message: Some(
                                        "That token didn't work. Check it was copied whole.".into(),
                                    ),
                                },
                                text,
                            )
                        } else {
                            UserError::classify("replace the token", &err)
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
                    shared.fail("disconnect the vault", &err);
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
        ClientCommand::MachineDebugRefresh => {
            let shared = shared.clone();
            tokio::spawn(async move { refresh_machine_debug(&shared).await });
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
                    Ok(r) => shared.fail("change the setting for this Mac", r.status()),
                    Err(err) => shared.fail("change the setting for this Mac", &err),
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
                    Ok(r) => shared.fail("forget the helper on this Mac", r.status()),
                    Err(err) => shared.fail("forget the helper on this Mac", &err),
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
                    Err(err) => shared.fail("create the routine", &err),
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
                    Ok(r) => shared.fail("update the routine", r.status()),
                    Err(err) => shared.fail("update the routine", &err),
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
                    shared.fail("run the routine", &err);
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
                    Ok(r) => shared.fail("delete the routine", r.status()),
                    Err(err) => shared.fail("delete the routine", &err),
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
                    Err(err) => shared.fail("save the skill", &err),
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
                    Ok(r) => shared.fail("delete the skill", r.status()),
                    Err(err) => shared.fail("delete the skill", &err),
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
                    Err(err) => shared.fail("get a pairing code", &err),
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
                    Ok(r) => shared.fail("unbind the channel", r.status()),
                    Err(err) => shared.fail("unbind the channel", &err),
                }
                refresh_channels(&shared).await;
            });
        }
        // D88: handled by `handle_server_command` above; listed so a new variant still fails
        // to compile here.
        ClientCommand::ServersRefresh
        | ClientCommand::ServerCreate { .. }
        | ClientCommand::ServerJoin { .. }
        | ClientCommand::ServerFetch { .. }
        | ClientCommand::ServerRename { .. }
        | ClientCommand::ServerDelete { .. }
        | ClientCommand::ServerLeave { .. }
        | ClientCommand::ServerInviteCreate { .. }
        | ClientCommand::ServerInviteRevoke { .. }
        | ClientCommand::ServerSetRole { .. }
        | ClientCommand::ServerRemoveMember { .. }
        | ClientCommand::ChannelCreate { .. }
        | ClientCommand::ChannelPatch { .. }
        | ClientCommand::ChannelDelete { .. }
        | ClientCommand::ServerAddMembers { .. }
        | ClientCommand::ChannelArchive { .. }
        | ClientCommand::ChannelOpen { .. }
        | ClientCommand::ChannelLoadOlder { .. }
        | ClientCommand::ChannelSend { .. }
        | ClientCommand::ChannelMarkRead { .. } => {}
    }
}

/// Settings → Channels: `GET /channels` → one view (F7). Decode platform and
/// binding rows one at a time so a bad row does not drop the overview.
async fn refresh_channels(shared: &Arc<Shared>) {
    match shared.get_json("/channels").await {
        Ok(v) => {
            let mock = v.get("mock").and_then(Value::as_bool).unwrap_or(false);
            let mut platforms = Vec::new();
            for item in v
                .get("platforms")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                match serde_json::from_value::<ChannelPlatformDto>(item) {
                    Ok(d) => platforms.push(ChannelPlatform::from(&d)),
                    Err(err) => {
                        log::warn!(target: "arbos_mac::relay", "ChannelPlatformDto: {err}");
                    }
                }
            }
            let mut bindings = Vec::new();
            for item in v
                .get("bindings")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                match serde_json::from_value::<ChannelBindingDto>(item) {
                    Ok(d) => bindings.push(ChannelBinding::from(&d)),
                    Err(err) => {
                        log::warn!(target: "arbos_mac::relay", "ChannelBindingDto: {err}");
                    }
                }
            }
            let pairing = match v.get("pairing") {
                None | Some(Value::Null) => None,
                Some(p) => match serde_json::from_value::<ChannelPairingDto>(p.clone()) {
                    Ok(d) => Some(ChannelPairing::from(&d)),
                    Err(err) => {
                        log::warn!(target: "arbos_mac::relay", "ChannelPairingDto: {err}");
                        None
                    }
                },
            };
            let _ = shared.events.send(SyncEvent::Channels(ChannelsView {
                mock,
                platforms,
                bindings,
                pairing,
                loaded: true,
            }));
        }
        // Older relays have no channels; stay quiet unless it is a real failure.
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("channels", format!("{err:#}"));
            }
        }
    }
}

async fn refresh_routines(shared: &Arc<Shared>, project_id: &str) {
    match shared
        .get_json(&format!("/projects/{project_id}/routines"))
        .await
    {
        Ok(v) => {
            let routines: Vec<Routine> = serde_json::from_value::<Vec<RoutineDto>>(v)
                .unwrap_or_default()
                .iter()
                .filter_map(routine_from_dto)
                .collect();
            let _ = shared.events.send(SyncEvent::Routines {
                project_id: project_id.to_string(),
                routines,
            });
        }
        Err(err) => {
            // Gone project, people space, or older relay without routines — not a bug.
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("routines", format!("{err:#}"));
            }
        }
    }
}

/// `GET /machine/debug` → the Computer tab's report. A relay without the route (404) or an
/// unreachable box is not a bug: the tab says so from the report's own fields.
/// The tasks chip's poll: `GET /projects/{id}` → `activity.jobs[]` (duplex §2.4).
/// A relay without the field or a `null` activity reads as "no open jobs" (the
/// drawer degrades to nothing); a transport error keeps the last answer rather
/// than blinking the list away on a blip.
async fn refresh_project_jobs(shared: &Arc<Shared>, project_id: &str) {
    match shared.get_json(&format!("/projects/{project_id}")).await {
        Ok(v) => {
            let _ = shared.events.send(SyncEvent::ProjectJobs {
                project_id: project_id.to_string(),
                jobs: crate::model::Job::list_from_activity(v.get("activity")),
            });
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("project-jobs", format!("{err:#}"));
            }
        }
    }
}

/// The rollout view's poll: `GET /projects/{id}/jobs/{jobId}[?after=<seq>]`. A 404 (relay
/// before the endpoint) answers an `unavailable` placeholder so the view can say so; a
/// transport error keeps the last document.
async fn refresh_job_rollout(
    shared: &Arc<Shared>,
    project_id: &str,
    job_id: &str,
    after: Option<u64>,
) {
    let path = match after {
        Some(seq) => format!("/projects/{project_id}/jobs/{job_id}?after={seq}"),
        None => format!("/projects/{project_id}/jobs/{job_id}"),
    };
    match shared.get_json(&path).await {
        Ok(v) => match serde_json::from_value::<crate::model::JobRollout>(v) {
            Ok(rollout) => {
                let _ = shared.events.send(SyncEvent::JobRollout {
                    project_id: project_id.to_string(),
                    job_id: job_id.to_string(),
                    rollout,
                });
            }
            Err(err) => {
                log::warn!(target: "arbos_mac::relay", "JobRollout {job_id}: {err}");
            }
        },
        Err(err) => {
            let text = format!("{err:#}");
            if text.contains("HTTP 404") {
                let _ = shared.events.send(SyncEvent::JobRollout {
                    project_id: project_id.to_string(),
                    job_id: job_id.to_string(),
                    rollout: crate::model::JobRollout {
                        unavailable: true,
                        ..crate::model::JobRollout::placeholder(
                            &crate::model::Job {
                                id: job_id.to_string(),
                                title: String::new(),
                                state: crate::model::JobState::Running,
                                step: String::new(),
                                elapsed_s: 0,
                                ask_message_id: None,
                                chat_id: None,
                            },
                            "",
                        )
                    },
                });
            } else {
                shared.diagnostic("job-rollout", text);
            }
        }
    }
}

async fn refresh_machine_debug(shared: &Arc<Shared>) {
    match shared.get_json("/machine/debug").await {
        Ok(v) => {
            let report = BoxReport::from_debug(&v, now_ms());
            let _ = shared.events.send(SyncEvent::BoxReport(report));
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("machine-debug", format!("{err:#}"));
            }
        }
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
            let runs: Vec<RoutineRun> = serde_json::from_value::<Vec<RoutineRunDto>>(v)
                .unwrap_or_default()
                .iter()
                .map(RoutineRun::from)
                .collect();
            let _ = shared.events.send(SyncEvent::RoutineRuns {
                routine_id: routine_id.to_string(),
                runs,
            });
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("routine-runs", format!("{err:#}"));
            }
        }
    }
}

/// Settings → This Mac: `GET /local/devices` (F9).
async fn refresh_local(shared: &Arc<Shared>) {
    let devices = shared.get_json("/local/devices").await.and_then(|v| {
        serde_json::from_value::<LocalDevicesDto>(v).context("decode GET /local/devices")
    });
    match devices {
        Ok(dto) => {
            let _ = shared
                .events
                .send(SyncEvent::LocalDevices(local_view_from_dto(
                    &dto,
                    &shared.http_url,
                )));
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("local-devices", format!("{err:#}"));
            }
        }
    }
}

/// Settings → Connectors: `GET /connectors/catalog` + `GET /connectors` → one view (F1).
async fn refresh_connectors(shared: &Arc<Shared>) {
    let (catalog, list) = tokio::join!(
        shared.get_json("/connectors/catalog"),
        shared.get_json("/connectors")
    );
    let catalog = catalog.and_then(|v| {
        serde_json::from_value::<ConnectorsCatalogDto>(v).context("decode GET /connectors/catalog")
    });
    let list = list.and_then(|v| {
        serde_json::from_value::<ConnectorsListDto>(v).context("decode GET /connectors")
    });
    match (catalog, list) {
        (Ok(catalog), Ok(list)) => {
            let _ = shared
                .events
                .send(SyncEvent::Connectors(connectors_view_from_dto(
                    &catalog, &list,
                )));
        }
        // Older relays have no connectors; stay quiet unless it is a real failure.
        (Err(err), _) | (_, Err(err)) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.diagnostic("connectors", format!("{err:#}"));
            }
        }
    }
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
                shared.fail("load your secrets", &err);
            }
            return;
        }
    };
    let Ok(overview) = serde_json::from_value::<SecretsOverviewDto>(overview) else {
        return;
    };
    let requests = serde_json::from_value::<SecretRequestsDto>(requests)
        .map(|r| r.items)
        .unwrap_or_default();
    let _ = shared.events.send(SyncEvent::Secrets(secrets_view_from_dto(
        &overview, &requests,
    )));
}

/// Settings → Devices (`GET /auth/devices`): the list, or why it could not be read.
async fn refresh_devices(shared: &Arc<Shared>) {
    let result = match shared.get_json("/auth/devices").await {
        Ok(value) => serde_json::from_value::<Vec<crate::model::DeviceView>>(value).map_err(|e| {
            UserError::from_text("load your devices", format!("unexpected reply: {e}"))
        }),
        Err(err) => Err(UserError::classify("load your devices", &err)),
    };
    let _ = shared.events.send(SyncEvent::Devices(result));
}

/// One agent's Secrets page (`GET /projects/{id}/secrets`).
async fn refresh_project_secrets(shared: &Arc<Shared>, project_id: &str) {
    match shared
        .get_json(&format!("/projects/{project_id}/secrets"))
        .await
    {
        Ok(v) => {
            if let Ok(dto) = serde_json::from_value::<ProjectSecretsDto>(v) {
                let _ = shared.events.send(SyncEvent::ProjectSecrets {
                    project_id: project_id.to_string(),
                    view: ProjectSecrets::from(&dto),
                });
            }
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.fail("load the agent's secrets", &err);
            }
        }
    }
}

/// One agent's standing shell approvals (`GET /projects/{id}/shell-grants`, #1544).
async fn refresh_shell_grants(shared: &Arc<Shared>, project_id: &str) {
    match shared
        .get_json(&format!("/projects/{project_id}/shell-grants"))
        .await
    {
        Ok(v) => {
            let items: Vec<crate::model::ShellGrant> = v
                .get("items")
                .and_then(|i| serde_json::from_value(i.clone()).ok())
                .unwrap_or_default();
            let _ = shared.events.send(SyncEvent::ShellGrants {
                project_id: project_id.to_string(),
                items,
            });
        }
        Err(err) => {
            if !format!("{err:#}").contains("HTTP 404") {
                shared.fail("load the shell approvals", &err);
            }
        }
    }
}

/// Marker in a `run_session` error chain: the WebSocket never opened (TCP,
/// TLS, DNS or the HTTP upgrade failed, or the dial timed out).
#[derive(Debug)]
struct DialFailed;

impl std::fmt::Display for DialFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dial failed")
    }
}

/// What a failed relay call says about the link, for the header pill: the
/// first cause in the chain that names a transport verdict wins; anything
/// else (a protocol error, a timeout, bad JSON) is a plain "Reconnecting…".
fn link_fault(err: &anyhow::Error) -> LinkState {
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return io_fault(io);
        }
        if let Some(ws) = cause.downcast_ref::<tokio_tungstenite::tungstenite::Error>() {
            use tokio_tungstenite::tungstenite::Error as Ws;
            return match ws {
                Ws::Io(io) => io_fault(io),
                Ws::Http(resp) if resp.status().is_server_error() => LinkState::RelayDown,
                Ws::Tls(_) => LinkState::RelayDown,
                Ws::Http(_)
                | Ws::ConnectionClosed
                | Ws::AlreadyClosed
                | Ws::Capacity(_)
                | Ws::Protocol(_)
                | Ws::WriteBufferFull(_)
                | Ws::Utf8
                | Ws::AttackAttempt
                | Ws::Url(_)
                | Ws::HttpFormat(_) => LinkState::Reconnecting,
            };
        }
        if let Some(req) = cause.downcast_ref::<reqwest::Error>() {
            if req.status().is_some_and(|s| s.is_server_error()) {
                return LinkState::RelayDown;
            }
            if req.is_connect() {
                let mut source = std::error::Error::source(req);
                while let Some(inner) = source {
                    if let Some(io) = inner.downcast_ref::<std::io::Error>() {
                        return io_fault(io);
                    }
                    source = inner.source();
                }
                return LinkState::RelayDown;
            }
            if req.is_timeout() {
                return LinkState::Reconnecting;
            }
        }
        if let Some(relay) = cause.downcast_ref::<RelayError>() {
            if (500..600).contains(&relay.status) {
                return LinkState::RelayDown;
            }
        }
    }
    LinkState::Reconnecting
}

/// Socket-level verdict: no route / no DNS is this Mac being offline; a refusal
/// or a reset is a relay that is not listening.
fn io_fault(io: &std::io::Error) -> LinkState {
    use std::io::ErrorKind as K;
    match io.kind() {
        K::NetworkUnreachable | K::HostUnreachable | K::NetworkDown => return LinkState::Offline,
        K::ConnectionRefused | K::ConnectionReset | K::ConnectionAborted => {
            return LinkState::RelayDown
        }
        _ => {}
    }
    let text = io.to_string().to_ascii_lowercase();
    // getaddrinfo failures surface as `Uncategorized` with the resolver's text
    // (macOS: "nodename nor servname provided, or not known"; glibc: "Temporary
    // failure in name resolution"; tokio: "failed to lookup address information").
    if text.contains("lookup")
        || text.contains("nodename")
        || text.contains("name resolution")
        || text.contains("no address associated")
        || text.contains("dns")
    {
        return LinkState::Offline;
    }
    LinkState::Reconnecting
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

#[cfg(test)]
mod generate_failure_tests {
    //! Jacob, 2026-10-26: the picker shows one short sentence, the raw words only as detail.
    use super::*;

    #[test]
    fn a_relay_answer_keeps_its_sentence_and_detail() {
        let err = RelayError::from_body(
            reqwest::StatusCode::GATEWAY_TIMEOUT,
            r#"{"error":{"type":"avatar_timeout","message":"Couldn't generate the image. The server took too long.","detail":"gpt-image-1-mini: operation timed out"}}"#,
        );
        assert_eq!(
            err.detail.as_deref(),
            Some("gpt-image-1-mini: operation timed out")
        );
        let e = generate_failure(&PictureTarget::Me("gh_1".into()), &anyhow::Error::from(err));
        assert_eq!(
            e.sentence(),
            "Couldn't generate the image. The server took too long."
        );
        assert!(e.raw.contains("gpt-image-1-mini: operation timed out"));
    }

    #[test]
    fn a_transport_timeout_is_the_plain_sentence_with_the_raw_words_as_detail() {
        let raw = anyhow::anyhow!("error sending request for url (https://relay.arbos.life/projects/x/avatar/generate): operation timed out")
            .context("POST /projects/x/avatar/generate");
        let e = generate_failure(&PictureTarget::Agent("x".into()), &raw);
        assert_eq!(
            e.sentence(),
            "Couldn't generate the image. The server took too long."
        );
        assert!(e.copy_text().contains("operation timed out"));
        let other = anyhow::anyhow!("dns error").context("POST /projects/x/avatar/generate");
        let e = generate_failure(&PictureTarget::Agent("x".into()), &other);
        assert_eq!(e.sentence(), "Couldn't generate the image. You're offline.");
        let draft = generate_failure(
            &PictureTarget::Draft,
            &anyhow::Error::from(RelayError::from_body(reqwest::StatusCode::NOT_FOUND, "")),
        );
        assert_eq!(
            draft.sentence(),
            "Couldn't generate the image. Generate is not on this relay yet; upload a picture instead."
        );
    }
}

#[cfg(test)]
mod link_fault_tests {
    use super::*;
    use std::io::{Error as IoError, ErrorKind as K};

    fn dial(io: IoError) -> anyhow::Error {
        anyhow::Error::from(tokio_tungstenite::tungstenite::Error::Io(io)).context(DialFailed)
    }

    #[test]
    fn no_route_or_no_dns_is_offline() {
        assert_eq!(
            link_fault(&dial(IoError::from(K::NetworkUnreachable))),
            LinkState::Offline
        );
        assert_eq!(
            link_fault(&dial(IoError::other(
                "failed to lookup address information: nodename nor servname provided, or not known"
            ))),
            LinkState::Offline
        );
        assert_eq!(
            link_fault(&dial(IoError::other(
                "Temporary failure in name resolution"
            ))),
            LinkState::Offline
        );
    }

    #[test]
    fn a_refusal_a_5xx_or_a_tls_failure_is_relay_down() {
        assert_eq!(
            link_fault(&dial(IoError::from(K::ConnectionRefused))),
            LinkState::RelayDown
        );
        let resp = tokio_tungstenite::tungstenite::http::Response::builder()
            .status(502)
            .body(None)
            .expect("response");
        assert_eq!(
            link_fault(&anyhow::Error::from(
                tokio_tungstenite::tungstenite::Error::Http(resp)
            )),
            LinkState::RelayDown
        );
        let relay = RelayError {
            status: 503,
            kind: String::new(),
            message: "bad gateway".into(),
            detail: None,
        };
        assert_eq!(
            link_fault(&anyhow::Error::from(relay)),
            LinkState::RelayDown
        );
    }

    #[test]
    fn anything_else_is_a_plain_reconnect() {
        assert_eq!(
            link_fault(&dial(IoError::from(K::TimedOut))),
            LinkState::Reconnecting
        );
        assert_eq!(
            link_fault(&anyhow::anyhow!("dial timed out").context(DialFailed)),
            LinkState::Reconnecting
        );
        assert_eq!(
            link_fault(&anyhow::Error::from(
                tokio_tungstenite::tungstenite::Error::ConnectionClosed
            )),
            LinkState::Reconnecting
        );
        let relay = RelayError {
            status: 404,
            kind: "not_found".into(),
            message: String::new(),
            detail: None,
        };
        assert_eq!(
            link_fault(&anyhow::Error::from(relay)),
            LinkState::Reconnecting
        );
    }

    #[test]
    fn the_priority_order_is_offline_relay_reconnecting_syncing_restart() {
        assert!(LinkState::Offline > LinkState::RelayDown);
        assert!(LinkState::RelayDown > LinkState::Reconnecting);
        assert!(LinkState::Reconnecting > LinkState::Syncing);
        assert!(LinkState::Syncing > LinkState::RestartSoon);
        assert!(LinkState::RestartSoon > LinkState::Live);
    }

    #[test]
    fn only_service_restart_and_going_away_are_clean_closes() {
        assert!(is_restart_close(1012));
        assert!(is_restart_close(1001));
        assert!(!is_restart_close(1000));
        assert!(!is_restart_close(1006));
        assert!(!is_restart_close(1011));
    }
}
