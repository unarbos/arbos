//! Agent voice calls (Calls Phase 2, D75). The relay sits between three parties:
//!
//! * the **clients**, which start a call in a chat exactly as in calls v1 — when the chat has a
//!   project agent (its own project chat, or a people chat the agent was added to) the relay also
//!   asks LiveKit to **dispatch the voice worker** (`arbos-voice`, on the relay host) into the
//!   room; it joins as `a_<project_id>`;
//! * the **voice worker**, which talks to the relay with the admin token over loopback:
//!   `GET /voice/calls/{id}` (context pack + caps), `POST /voice/calls/{id}/tool` (hand a request
//!   to the agent on the user's machine), `POST /voice/calls/{id}/events` (transcript lines,
//!   judge decisions, joined/left) and `POST /voice/calls/{id}/usage` (minutes, tokens, $ —
//!   charged to the owner's ledger as `voice` debits with `ref = call:<id>`; the answer carries
//!   the cost stops);
//! * the **box** (agentd), which keeps one WebSocket `WS /box/voice` open per user box; the relay
//!   forwards tool calls and transcript rows over it and waits for results, the same shape as
//!   the `arbos-local` helper link (`local.rs`).
//!
//! Cost rails: voice has no limit of its own, and the one it spends against is cost-based
//! (Jacob, 2026-11-05: "all limits should be cost based"). The worker reports usage every
//! `ARBOS_VOICE_USAGE_EVERY_S` (30 s) to `POST /voice/calls/{id}/usage`, which debits the
//! difference into the caller's account ledger there and then and answers `caps.stop`; the
//! worker leaves the call on any stop. So a call is ended by the account's credits, its one
//! daily limit or the kill switch, mid-call, within one reporting interval of the money
//! running out. `CALL_LEAK_GUARD_MINUTES` is not a budget: it is the leak guard for a call
//! nobody is on any more, set far above any real call. Owner setting per agent: chattiness
//! (`quiet` | `balanced` | `chatty`), wake name and `vision_enabled` (may the agent see cameras),
//! `GET/PUT /projects/{id}/voice`.
//!
//! Camera vision: the worker reports the image part of the bill (`image_in_tokens`, `vision_usd`,
//! camera-on `vision_seconds`, `frames`, `looks`) inside the same usage report; it is debited
//! within the same caps as a second `voice` ledger line with `ref = call:<id>#vision`. A `look`
//! handed to the agent carries `attachments` (blobs the worker uploaded) through `voice_tool`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use arbos_core::api::{VoiceCatalogDto, VoiceDto};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db::Project;
use crate::db_calls::CallRow;
use crate::db_voice::{VoiceDecision, VoiceUsage, CHATTINESS_LEVELS};
use crate::notify::{self, ChatRef, Sender};
use crate::proxy::start_of_utc_day_ms;
use crate::signin::api_error;
use crate::state::AppState;

const PING_EVERY: Duration = Duration::from_secs(20);
/// How long a tool call may take on the box before the worker is told to carry on without it.
const TOOL_WAIT: Duration = Duration::from_secs(240);
/// The context pack is cheap; a slow box must not delay the agent's first words.
const CONTEXT_WAIT: Duration = Duration::from_secs(4);
const FRAME_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Leak guard, not a budget: the ceiling that catches a call whose room or socket is stuck
/// after everyone has gone, so a wedged call cannot bill forever. A call's cost is bounded by
/// the account's own limit, charged every reporting interval, so this sits far above any real
/// call (6 h) and should never be what ends one.
pub const CALL_LEAK_GUARD_MINUTES: i64 = 360;

/// Voice configuration from the environment.
#[derive(Debug, Clone)]
pub struct VoiceCfg {
    /// LiveKit agent name the worker registers with (`ARBOS_VOICE_AGENT_NAME`).
    pub agent_name: String,
    /// `ARBOS_VOICE=0` turns dispatch off while leaving people calls alone.
    pub enabled: bool,
    /// Speech model the worker is expected to use (for the admin view and the docs).
    pub model: String,
    /// Directory with `voices.json` and `<id>.mp3` samples (`ARBOS_VOICES_DIR`, default
    /// `<data>/voices`), made once by `voice/tools/make_voice_samples.py`.
    pub voices_dir: std::path::PathBuf,
    /// Voice used when a project has none (`ARBOS_VOICE_VOICE`, default `marin`).
    pub default_voice: String,
}

impl Default for VoiceCfg {
    fn default() -> Self {
        Self {
            agent_name: "arbos-voice".to_owned(),
            enabled: true,
            model: "gpt-realtime-2.1".to_owned(),
            voices_dir: std::path::PathBuf::from("voices"),
            default_voice: "marin".to_owned(),
        }
    }
}

pub fn cfg_from_env(data_dir: &std::path::Path) -> VoiceCfg {
    let d = VoiceCfg::default();
    let cfg = VoiceCfg {
        voices_dir: std::env::var("ARBOS_VOICES_DIR")
            .ok()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| data_dir.join("voices")),
        default_voice: std::env::var("ARBOS_VOICE_VOICE")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or(d.default_voice),
        agent_name: std::env::var("ARBOS_VOICE_AGENT_NAME")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or(d.agent_name),
        enabled: std::env::var("ARBOS_VOICE")
            .map(|v| v != "0")
            .unwrap_or(true),
        model: std::env::var("ARBOS_VOICE_MODEL")
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or(d.model),
    };
    tracing::info!(
        agent = %cfg.agent_name, model = %cfg.model,
        enabled = cfg.enabled, voices = %cfg.voices_dir.display(),
        default_voice = %cfg.default_voice, "agent voice calls"
    );
    cfg
}

// ---- voices catalogue ---------------------------------------------------------------------------

/// `voices.json` as written by `voice/tools/make_voice_samples.py`: `{ default, voices: [{id, name,
/// description, seconds, line, model}] }`. Read on every call (it is tiny and changes only on deploy).
pub fn voice_catalogue(state: &AppState) -> Vec<Value> {
    let path = state.voice.voices_dir.join("voices.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    v["voices"].as_array().cloned().unwrap_or_default()
}

/// Is `id` a voice the catalogue offers?
pub fn valid_voice(state: &AppState, id: &str) -> bool {
    voice_catalogue(state)
        .iter()
        .any(|v| v["id"].as_str() == Some(id))
}

/// The voice the worker should use for a project: its own choice or the relay default.
pub fn voice_for(state: &AppState, project_id: &str) -> String {
    state
        .db
        .voice_agent_settings(project_id)
        .ok()
        .and_then(|s| s.voice)
        .filter(|v| valid_voice(state, v))
        .unwrap_or_else(|| state.voice.default_voice.clone())
}

/// Build the `GET /voices` catalogue DTO (also used for fixtures).
pub(crate) fn voices_catalog_dto(state: &AppState) -> VoiceCatalogDto {
    let base = state.cfg.public_url.trim_end_matches('/');
    let voices: Vec<VoiceDto> = voice_catalogue(state)
        .into_iter()
        .map(|v| {
            let id = v["id"].as_str().unwrap_or_default().to_owned();
            VoiceDto {
                name: v["name"].as_str().unwrap_or(&id).to_owned(),
                description: v["description"].as_str().map(str::to_owned),
                seconds: v["seconds"].as_f64(),
                sample_url: format!("{base}/voices/{id}/sample.mp3"),
                id,
            }
        })
        .collect();
    VoiceCatalogDto {
        default_voice: state.voice.default_voice.clone(),
        voices,
    }
}

/// `GET /voices` (no auth) → `{ default, voices: [{ id, name, description, seconds, sample_url }] }`.
pub async fn list_voices(State(state): State<AppState>) -> Response {
    let mut resp = Json(voices_catalog_dto(&state)).into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    resp
}

/// `GET /voices/{id}/sample.mp3` (no auth): the pre-made sample, cached for a year.
pub async fn voice_sample(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || !valid_voice(&state, &id)
    {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such voice");
    }
    let path = state.voice.voices_dir.join(format!("{id}.mp3"));
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("audio/mpeg")),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no sample for this voice",
        ),
    }
}

// ---- call inbox: the chat, live, for the call (#1832) -----------------------------------------------
//
// Jacob, 2026-10-25: on a call with Bubble he asked for pizza delivery; the agent's long turn
// posted the answer in the chat 45 s later, and the call never heard of it — the voice session
// only got the first finished bubble back from `POST …/tool`, and nothing carried later bubbles
// (or anything else landing in the chat) to the call, so the voice model answered "still
// running" from memory. Now every finished bubble and every message that `notify::announce`
// sees for a chat with a live agent call is queued here per (call, agent); the worker long-polls
// `GET /voice/calls/{id}/inbox` and feeds it into the model's context. The main voice agent is
// the only speaker: nothing here is spoken by anyone else.

/// One queued chat update for a live call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallUpdate {
    pub seq: u64,
    pub at: i64,
    /// `result` (a turn's answer), `job` (a background job's outcome), `quick` (a status line),
    /// `message` (a person wrote in the chat).
    pub kind: String,
    pub sender: Value,
    pub message_id: String,
    pub text: String,
}

#[derive(Default)]
struct InboxQueue {
    seq: u64,
    items: VecDeque<CallUpdate>,
    notify: Arc<Notify>,
}

/// The queued updates of every live (call, agent), keyed `call:project`. At most
/// `INBOX_KEEP` items are kept per key; a call that ended is dropped.
#[derive(Clone, Default)]
pub struct VoiceInbox {
    queues: Arc<Mutex<HashMap<String, InboxQueue>>>,
}

const INBOX_KEEP: usize = 40;
const INBOX_TEXT_MAX: usize = 2_000;
const INBOX_WAIT_MAX_S: u64 = 25;

impl VoiceInbox {
    fn key(call_id: &str, project_id: &str) -> String {
        format!("{call_id}:{project_id}")
    }

    /// Queue `update` for (call, agent); wakes a waiting long-poll.
    pub async fn push(&self, call_id: &str, project_id: &str, mut update: CallUpdate) -> u64 {
        let mut queues = self.queues.lock().await;
        let q = queues.entry(Self::key(call_id, project_id)).or_default();
        q.seq += 1;
        update.seq = q.seq;
        update.text = update.text.chars().take(INBOX_TEXT_MAX).collect();
        q.items.push_back(update);
        while q.items.len() > INBOX_KEEP {
            q.items.pop_front();
        }
        q.notify.notify_waiters();
        q.seq
    }

    /// Items newer than `after` (and the latest seq), without waiting.
    pub async fn take(
        &self,
        call_id: &str,
        project_id: &str,
        after: u64,
    ) -> (Vec<CallUpdate>, u64) {
        let queues = self.queues.lock().await;
        match queues.get(&Self::key(call_id, project_id)) {
            Some(q) => (
                q.items.iter().filter(|u| u.seq > after).cloned().collect(),
                q.seq,
            ),
            None => (Vec::new(), after),
        }
    }

    /// `take`, waiting up to `wait` for something newer than `after`.
    pub async fn wait(
        &self,
        call_id: &str,
        project_id: &str,
        after: u64,
        wait: Duration,
    ) -> (Vec<CallUpdate>, u64) {
        let notify = {
            let mut queues = self.queues.lock().await;
            queues
                .entry(Self::key(call_id, project_id))
                .or_default()
                .notify
                .clone()
        };
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let (items, seq) = self.take(call_id, project_id, after).await;
            if !items.is_empty() {
                return (items, seq);
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return (items, seq);
            }
            let _ = tokio::time::timeout(left, notify.notified()).await;
        }
    }

    /// The call ended: drop its queues (every agent of it).
    pub async fn close_call(&self, call_id: &str) {
        let mut queues = self.queues.lock().await;
        let prefix = format!("{call_id}:");
        let gone: Vec<String> = queues
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for k in gone {
            if let Some(q) = queues.remove(&k) {
                q.notify.notify_waiters();
            }
        }
    }
}

/// `notify::announce` saw a message in `chat`: queue it for every live agent call there.
/// Transcript lines of the call itself never come this way (agentd writes them as rows, no
/// notify), so a call does not hear its own words back.
pub async fn call_update(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    b: &crate::notify::NotifyBody,
) {
    let text = b
        .voice_text
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&b.text);
    if text.trim().is_empty() {
        return;
    }
    let kind = match (sender.kind, b.kind.as_deref()) {
        ("agent", Some(k)) if matches!(k, "result" | "job" | "quick") => k.to_owned(),
        ("agent", _) => "result".to_owned(),
        _ => "message".to_owned(),
    };
    let Ok(Some(call)) = state.db.active_call_in_chat(&chat.chat_id) else {
        return;
    };
    {
        for project_id in state.db.call_agents_live(&call.id).unwrap_or_default() {
            let seq = state
                .voice_inbox
                .push(
                    &call.id,
                    &project_id,
                    CallUpdate {
                        seq: 0,
                        at: now_ms(),
                        kind: kind.clone(),
                        sender: sender.json(),
                        message_id: b.message_id.clone(),
                        text: text.to_owned(),
                    },
                )
                .await;
            tracing::info!(call = %call.id, project = %project_id, kind = %kind, seq, message = %b.message_id, "voice: call update queued");
        }
    }
}

#[derive(Deserialize, Default)]
pub struct InboxQuery {
    #[serde(default)]
    pub project_id: Option<String>,
    /// Last seq the worker has seen (0 at join).
    #[serde(default)]
    pub after: u64,
    /// Long-poll seconds (≤ 25).
    #[serde(default)]
    pub wait: u64,
}

/// `GET /voice/calls/{id}/inbox?project_id=&after=&wait=` (worker) → `{ items: [{seq, at, kind,
/// sender, message_id, text}], seq, live }`. Holds up to `wait` s for something new; `live` is
/// false once the call ended (the worker stops polling).
pub async fn call_inbox(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<InboxQuery>,
) -> Response {
    if let Err(r) = worker_only(&principal) {
        return *r;
    }
    let (call, project, _) = match load(&state, &id, q.project_id.as_deref()) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !call.active() {
        return Json(json!({ "items": [], "seq": q.after, "live": false })).into_response();
    }
    let wait = Duration::from_secs(q.wait.min(INBOX_WAIT_MAX_S));
    let (items, seq) = state
        .voice_inbox
        .wait(&call.id, &project.id, q.after, wait)
        .await;
    let live = state
        .db
        .call(&call.id)
        .ok()
        .flatten()
        .is_some_and(|c| c.active());
    Json(json!({ "items": items, "seq": seq, "live": live })).into_response()
}

// ---- box link ------------------------------------------------------------------------------------

/// One live box socket.
pub struct VoiceConn {
    pub tx: mpsc::Sender<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
}

pub type VoiceConns = Arc<Mutex<HashMap<String, Arc<VoiceConn>>>>;

/// `WS /box/voice` — the user's box keeps this open; one per box.
pub async fn ws_box_voice(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    principal: Principal,
) -> Response {
    let Principal::Box { user_id } = principal else {
        return (StatusCode::FORBIDDEN, "box token required").into_response();
    };
    ws.max_message_size(FRAME_MAX_BYTES)
        .on_upgrade(move |socket| run_box(socket, state, user_id))
}

async fn run_box(socket: WebSocket, state: AppState, user_id: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<String>(256);
    let conn = Arc::new(VoiceConn {
        tx,
        pending: Mutex::new(HashMap::new()),
    });
    let previous = state
        .voice_conns
        .lock()
        .await
        .insert(user_id.clone(), conn.clone());
    if let Some(prev) = previous {
        fail_pending(&prev, "box reconnected").await;
    }
    tracing::info!(user = %user_id, "voice link: box connected");
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => break,
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
                };
                let Ok(frame) = serde_json::from_str::<Value>(&text) else { continue };
                match frame["type"].as_str().unwrap_or_default() {
                    "result" => {
                        if let Some(id) = frame["id"].as_str() {
                            if let Some(waiter) = conn.pending.lock().await.remove(id) {
                                let _ = waiter.send(frame.clone());
                            }
                        }
                    }
                    "hello" | "pong" => {}
                    other => tracing::debug!(user = %user_id, kind = other, "voice link: unknown frame"),
                }
            }
            out = rx.recv() => {
                let Some(text) = out else { break };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            _ = ping.tick() => {
                if sink.send(Message::Text(json!({ "type": "ping" }).to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    let mut conns = state.voice_conns.lock().await;
    if conns.get(&user_id).is_some_and(|c| Arc::ptr_eq(c, &conn)) {
        conns.remove(&user_id);
    }
    drop(conns);
    fail_pending(&conn, "box disconnected").await;
    tracing::info!(user = %user_id, "voice link: box disconnected");
}

async fn fail_pending(conn: &VoiceConn, why: &str) {
    for (_, waiter) in conn.pending.lock().await.drain() {
        let _ = waiter.send(json!({ "type": "result", "ok": false, "error": why }));
    }
}

/// Is the user's box on the voice link right now?
pub async fn box_online(state: &AppState, user_id: &str) -> bool {
    state.voice_conns.lock().await.contains_key(user_id)
}

/// Send `frame` to the box and wait for its `result` with the same id.
pub async fn box_request(
    state: &AppState,
    user_id: &str,
    mut frame: Value,
    wait: Duration,
) -> Result<Value, String> {
    let conn = state.voice_conns.lock().await.get(user_id).cloned();
    let Some(conn) = conn else {
        return Err("the agent's machine is not connected".to_owned());
    };
    let id = ulid();
    frame["id"] = json!(id);
    let (tx, rx) = oneshot::channel();
    conn.pending.lock().await.insert(id.clone(), tx);
    if conn.tx.send(frame.to_string()).await.is_err() {
        conn.pending.lock().await.remove(&id);
        return Err("the agent's machine link closed".to_owned());
    }
    match tokio::time::timeout(wait, rx).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err("the agent's machine disconnected".to_owned()),
        Err(_) => {
            conn.pending.lock().await.remove(&id);
            Err(format!(
                "no answer from the agent within {} s",
                wait.as_secs()
            ))
        }
    }
}

/// Send `frame` to the box without waiting.
pub async fn box_notify(state: &AppState, user_id: &str, frame: Value) -> Result<(), String> {
    let conn = state.voice_conns.lock().await.get(user_id).cloned();
    let Some(conn) = conn else {
        return Err("the agent's machine is not connected".to_owned());
    };
    conn.tx
        .send(frame.to_string())
        .await
        .map_err(|_| "the agent's machine link closed".to_owned())
}

// ---- who is in the chat ---------------------------------------------------------------------------

fn chat_ref_of(call: &CallRow) -> ChatRef {
    match &call.project_id {
        Some(p) => ChatRef::project(p, &call.chat_id),
        None => ChatRef::people(&call.chat_id),
    }
}

/// The project agents that belong to a chat: its own project for a project chat, the
/// `agent:<project>` members of a people chat.
pub fn agents_in_chat(state: &AppState, chat: &ChatRef) -> Vec<Project> {
    let ids: Vec<String> = match &chat.project_id {
        Some(pid) => vec![pid.clone()],
        None => state
            .db
            .members_of(&chat.chat_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|m| m.project_id().map(str::to_owned))
            .collect(),
    };
    ids.iter()
        .filter_map(|id| state.db.project(id).ok().flatten())
        .collect()
}

/// Humans of the chat as `sender{}` + role (`owner` of the agent's project, `guest`, `member`).
fn humans_json(state: &AppState, chat: &ChatRef, agent: &Project) -> Vec<Value> {
    notify::recipients(state, chat)
        .iter()
        .map(|u| {
            let mut v = Sender::user(state, u).json();
            v["role"] = json!(if *u == agent.user_id {
                "owner"
            } else if chat.project_id.is_some() {
                "guest"
            } else {
                "member"
            });
            v
        })
        .collect()
}

/// Best-effort where a human is calling from (`ios` / `mac` / …) for the voice prompt.
fn calling_from_of(state: &AppState, call: &CallRow, user_id: &str) -> Option<String> {
    if call.started_by == user_id {
        if let Some(p) = call.platform.as_ref().filter(|p| !p.is_empty()) {
            return Some(p.clone());
        }
    }
    let rows = state.db.device_reach_rows(user_id).ok()?;
    // VoIP / iOS push → iPhone; otherwise a live device with no iOS push is treated as Mac.
    if rows
        .iter()
        .any(|(_, _, _, _, platform, _, voip)| *voip || platform.as_deref() == Some("ios"))
    {
        return Some("ios".to_owned());
    }
    if rows.iter().any(|(_, _, _, seen, _, _, _)| seen.is_some()) {
        return Some("mac".to_owned());
    }
    None
}

fn enrich_humans_calling_from(state: &AppState, call: &CallRow, humans: &mut [Value]) {
    let live: std::collections::HashSet<String> = state
        .db
        .call_participants(&call.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.left_at.is_none())
        .map(|p| p.user_id)
        .collect();
    for h in humans.iter_mut() {
        let Some(id) = h.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        if !live.contains(&id) && call.started_by != id {
            continue;
        }
        if let Some(platform) = calling_from_of(state, call, &id) {
            h["platform"] = json!(platform);
            h["calling_from"] = json!(platform);
        }
    }
}

/// What the voice agent should call this chat: project chat title, or people-chat title / handles.
pub(crate) fn voice_chat_title(state: &AppState, chat: &ChatRef) -> String {
    match &chat.project_id {
        Some(pid) => state
            .db
            .chat(pid, &chat.chat_id)
            .ok()
            .flatten()
            .map(|c| c.title)
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "this chat".to_owned()),
        None => {
            if let Some(t) = state
                .db
                .people_chat(&chat.chat_id)
                .ok()
                .flatten()
                .and_then(|c| c.title)
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
            {
                return t;
            }
            // Bare handles, no `@`: this title is spoken aloud, and "at user41, at user43" is
            // not what anyone says. The other three surfaces agree (see `group_name_tests`).
            let handles: Vec<String> = state
                .db
                .members_of(&chat.chat_id)
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.user_id())
                .map(|u| Sender::user(state, u).handle)
                .filter(|h| !h.is_empty())
                .collect();
            if handles.is_empty() {
                "this chat".to_owned()
            } else {
                handles.join(", ")
            }
        }
    }
}

/// Identity the worker joins with.
pub fn agent_identity(project_id: &str) -> String {
    format!("a_{project_id}")
}

pub fn project_of_identity(identity: &str) -> Option<&str> {
    identity.strip_prefix("a_")
}

/// Live agents of a call as `sender{}` (for `participants[]`).
pub fn agent_participants_json(state: &AppState, call_id: &str) -> Vec<Value> {
    state
        .db
        .call_agents_live(call_id)
        .unwrap_or_default()
        .iter()
        .filter_map(|pid| state.db.project(pid).ok().flatten())
        .map(|p| Sender::agent(state, &p).json())
        .collect()
}

// ---- dispatch -----------------------------------------------------------------------------------

/// A dispatch younger than this still counts as "on its way" (the worker joins within ~1 s).
const DISPATCH_FRESH_MS: i64 = 30_000;

/// A call began in a chat with agents: ask LiveKit to send the voice worker for each of them.
/// `ring` — when `Some`, only agents whose project id is in the set are dispatched (a selective
/// group start). `None` = every agent in the chat. Returns the agents dispatched (as `sender{}`).
pub async fn dispatch_agents(
    state: &AppState,
    call: &CallRow,
    ring: Option<&std::collections::HashSet<&str>>,
) -> Vec<Value> {
    dispatch_missing(state, call, ring).await
}

/// Someone joined a live call: any agent of the chat that is not in the room (it left when it
/// was alone, or its job died) is dispatched again.
pub async fn ensure_agents(state: &AppState, call: &CallRow) {
    if call.active() {
        let _ = dispatch_missing(state, call, None).await;
    }
}

async fn dispatch_missing(
    state: &AppState,
    call: &CallRow,
    ring: Option<&std::collections::HashSet<&str>>,
) -> Vec<Value> {
    let (Some(lk), true) = (&state.livekit, state.voice.enabled) else {
        return Vec::new();
    };
    let chat = chat_ref_of(call);
    let live = state.db.call_agents_live(&call.id).unwrap_or_default();
    let now = now_ms();
    let mut out = Vec::new();
    for project in agents_in_chat(state, &chat) {
        if let Some(want) = ring {
            if !want.contains(project.id.as_str()) {
                continue;
            }
        }
        if live.contains(&project.id) {
            continue;
        }
        let key = format!("{}:{}", call.id, project.id);
        {
            let mut recent = state.voice_dispatched.lock().await;
            recent.retain(|_, at| now - *at < DISPATCH_FRESH_MS);
            if recent.contains_key(&key) {
                continue;
            }
            recent.insert(key, now);
        }
        let payer = project.user_id.clone();
        if let Err(stop) = stops_for(state, &payer) {
            tracing::info!(call = %call.id, project = %project.id, stop, "voice: agent not dispatched (cost stop)");
            continue;
        }
        let settings = state.db.voice_agent_settings(&project.id).ok();
        let meta = json!({
            "call_id": call.id,
            "chat_id": call.chat_id,
            "chat_project": chat.project_label(),
            "project_id": project.id,
            "room": call.room,
            "agent": Sender::agent(state, &project).json(),
            "identity": agent_identity(&project.id),
            "chattiness": settings.as_ref().map(|s| s.chattiness.clone()).unwrap_or_else(|| "balanced".to_owned()),
            "vision_enabled": settings.as_ref().map(|s| s.vision_enabled).unwrap_or(true),
            "wake_name": settings.and_then(|s| s.wake_name),
            "voice": voice_for(state, &project.id),
            "started_at": call.started_at,
        });
        match lk
            .create_dispatch(&state.http, &call.room, &state.voice.agent_name, &meta)
            .await
        {
            Ok(id) => {
                tracing::info!(call = %call.id, project = %project.id, dispatch = %id, "voice: agent dispatched");
                out.push(Sender::agent(state, &project).json());
            }
            Err(e) => {
                tracing::warn!(call = %call.id, project = %project.id, error = %e, "voice: dispatch failed")
            }
        }
    }
    out
}

// ---- cost stops ----------------------------------------------------------------------------------

/// Why the agent must not (keep) talk(ing): `kill_switch`, `credits`, `daily_cap`. Voice spends
/// the caller's account like any other source, so it has no cap of its own to check.
pub fn stops_for(state: &AppState, payer: &str) -> Result<(), &'static str> {
    if crate::credits::check_read_only(state).is_err() {
        return Err("kill_switch");
    }
    match crate::credits::check(state, payer) {
        Ok(()) => {}
        Err(crate::db_accounts::DebitRefused::DailyCap { .. }) => return Err("daily_cap"),
        Err(crate::db_accounts::DebitRefused::ReadOnly { .. }) => return Err("kill_switch"),
        Err(crate::db_accounts::DebitRefused::Insufficient { .. }) => return Err("credits"),
    }
    Ok(())
}

fn caps_json(state: &AppState, payer: &str, call_id: &str, project_id: &str) -> Value {
    let spent = state
        .db
        .voice_spent_since(payer, start_of_utc_day_ms())
        .unwrap_or(0.0);
    let call_usd = state
        .db
        .voice_call(call_id, project_id)
        .ok()
        .flatten()
        .map(|r| r.usage.usd)
        .unwrap_or(0.0);
    json!({
        "call_spent_usd": call_usd,
        "daily_spent_usd": spent,
        "call_leak_guard_minutes": CALL_LEAK_GUARD_MINUTES,
        "metered": crate::credits::metered(state),
        "stop": stops_for(state, payer).err(),
    })
}

// ---- worker endpoints (admin token, loopback) ------------------------------------------------------

fn worker_only(principal: &Principal) -> Result<(), Box<Response>> {
    if matches!(principal, Principal::Admin) {
        Ok(())
    } else {
        Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "voice worker token required",
        )))
    }
}

#[derive(Deserialize, Default)]
pub struct ProjectQuery {
    #[serde(default)]
    pub project_id: Option<String>,
}

/// The call, its agent and the context pack for the worker.
fn load(
    state: &AppState,
    call_id: &str,
    project_id: Option<&str>,
) -> Result<(CallRow, Project, ChatRef), Box<Response>> {
    let call = state.db.call(call_id).ok().flatten().ok_or_else(|| {
        Box::new(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such call",
        ))
    })?;
    let chat = chat_ref_of(&call);
    let agents = agents_in_chat(state, &chat);
    let project = match project_id {
        Some(pid) => agents.into_iter().find(|p| p.id == pid),
        None if agents.len() == 1 => agents.into_iter().next(),
        None => None,
    }
    .ok_or_else(|| {
        Box::new(api_error(
            StatusCode::NOT_FOUND,
            "no_agent",
            "this call's chat has no such agent",
        ))
    })?;
    Ok((call, project, chat))
}

/// `GET /voice/calls/{id}?project_id=` → `{ call, project, chat, humans, agents, settings, caps,
/// context, box_online }`. `context` is what the box composed for this call (voice system prompt,
/// recent lines); null when the box is offline.
pub async fn call_info(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<ProjectQuery>,
) -> Response {
    if let Err(r) = worker_only(&principal) {
        return *r;
    }
    let (call, project, chat) = match load(&state, &id, q.project_id.as_deref()) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let mut humans = humans_json(&state, &chat, &project);
    enrich_humans_calling_from(&state, &call, &mut humans);
    let settings = state
        .db
        .voice_agent_settings(&project.id)
        .map(|s| json!({ "chattiness": s.chattiness, "wake_name": s.wake_name, "voice": voice_for(&state, &project.id), "vision_enabled": s.vision_enabled }))
        .unwrap_or_else(|_| json!({ "chattiness": "balanced", "wake_name": null, "voice": state.voice.default_voice, "vision_enabled": true }));
    let online = box_online(&state, &project.user_id).await;
    let context = if online {
        box_request(
            &state,
            &project.user_id,
            json!({
                "type": "voice_context", "call_id": call.id, "chat_id": call.chat_id,
                "chat_project": chat.project_label(), "project_id": project.id, "humans": humans,
            }),
            CONTEXT_WAIT,
        )
        .await
        .ok()
        .filter(|v| v["ok"].as_bool().unwrap_or(false))
        .map(|v| v["context"].clone())
    } else {
        None
    };
    let chat_kind = match &chat.project_id {
        Some(pid) => state
            .db
            .chat(pid, &chat.chat_id)
            .ok()
            .flatten()
            .map(|c| c.kind)
            .unwrap_or_else(|| "chat".to_owned()),
        None => state
            .db
            .people_chat(&chat.chat_id)
            .ok()
            .flatten()
            .map(|c| c.kind)
            .unwrap_or_else(|| "dm".to_owned()),
    };
    let chat_title = voice_chat_title(&state, &chat);
    let agents: Vec<Value> = agents_in_chat(&state, &chat)
        .iter()
        .map(|p| Sender::agent(&state, p).json())
        .collect();
    Json(json!({
        "call": crate::calls::call_json_public(&state, &call),
        "project": {
            "id": project.id, "name": project.name, "description": project.description,
            "instructions": project.instructions, "user_id": project.user_id,
            "model": project.models.get("talking_model").and_then(|m| m.get("id")).cloned().unwrap_or(Value::Null),
            "sender": Sender::agent(&state, &project).json(),
            "identity": agent_identity(&project.id),
        },
        "chat": {
            "chat_id": chat.chat_id,
            "project_id": chat.project_label(),
            "kind": chat_kind,
            "title": chat_title,
        },
        "humans": humans,
        "agents": agents,
        "settings": settings,
        "caps": caps_json(&state, &project.user_id, &call.id, &project.id),
        "context": context,
        "box_online": online,
        "model": state.voice.model,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct ToolBody {
    #[serde(default)]
    pub project_id: Option<String>,
    /// Who asked (`sender{}` + `role`), as the worker heard it.
    #[serde(default)]
    pub speaker: Value,
    /// What they asked for, in the agent's words.
    pub request: String,
    /// `duplex` (1:1) or `group`.
    #[serde(default)]
    pub mode: Option<String>,
    /// The worker's id for this request (returned as is).
    #[serde(default)]
    pub request_id: Option<String>,
    /// Camera frames for the agent: `[{ name, mime, ref }]`, already in the chat's blob slot.
    #[serde(default)]
    pub attachments: Vec<Value>,
}

/// `POST /voice/calls/{id}/tool` → runs the request as a turn of the agent on the user's machine
/// (pi main session + local MCP, the same limits as text) and answers `{ ok, text, request_id }`.
/// 504 when the box did not finish within `TOOL_WAIT` (the worker says it will report later);
/// 503 when the box is offline.
pub async fn call_tool(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ToolBody>,
) -> Response {
    if let Err(r) = worker_only(&principal) {
        return *r;
    }
    let (call, project, chat) = match load(&state, &id, b.project_id.as_deref()) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if b.request.trim().is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "bad_request", "request is empty");
    }
    if !call.active() {
        return api_error(StatusCode::GONE, "call_ended", "this call has ended");
    }
    if let Err(stop) = stops_for(&state, &project.user_id) {
        return api_error(
            StatusCode::PAYMENT_REQUIRED,
            stop,
            "voice cost stop reached",
        );
    }
    let frame = json!({
        "type": "voice_tool", "call_id": call.id, "chat_id": call.chat_id,
        "chat_project": chat.project_label(), "project_id": project.id,
        "speaker": b.speaker, "request": b.request, "mode": b.mode,
        "attachments": b.attachments,
    });
    let t0 = std::time::Instant::now();
    match box_request(&state, &project.user_id, frame, TOOL_WAIT).await {
        Ok(v) => {
            let ok = v["ok"].as_bool().unwrap_or(false);
            tracing::info!(call = %call.id, project = %project.id, ok, ms = t0.elapsed().as_millis() as i64, "voice: tool call answered");
            let mut out = json!({
                "ok": ok, "text": v["text"], "error": v["error"], "request_id": b.request_id,
                "ms": t0.elapsed().as_millis() as i64,
            });
            if let Some(msg) = v.get("message_id") {
                out["message_id"] = msg.clone();
            }
            Json(out).into_response()
        }
        Err(e) if e.starts_with("no answer") => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({ "ok": false, "error": e, "request_id": b.request_id, "pending": true })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": e, "request_id": b.request_id })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct EventsBody {
    #[serde(default)]
    pub project_id: Option<String>,
    pub events: Vec<Value>,
}

/// (call, agent) pairs in which a human has said something the worker transcribed. An agent
/// line before that is its greeting — call audio, not a chat row (#1430). Entries leave when
/// the agent leaves the call.
static VOICE_HEARD: LazyLock<std::sync::Mutex<HashSet<String>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

fn heard_key(call_id: &str, project_id: &str) -> String {
    format!("{call_id}:{project_id}")
}

/// Which of the worker's events become chat rows on the box. A call is a call event, not a
/// chat message (#1430, Jacob 2026-10-23): what people and the agent **say to each other** on
/// the call is kept in the chat (spec §4); the agent's greeting (anything it says before a
/// human has spoken), its goodbye, and "joined / left / muted" notes are not. `human_lines`
/// is the number of human transcript lines seen earlier in this (call, agent); the return
/// carries the rows and the updated count.
pub fn chat_rows_of(events: &[Value], mut human_lines: usize) -> (Vec<Value>, usize) {
    let mut rows = Vec::new();
    let mut leaving = false;
    for ev in events {
        match ev["kind"].as_str().unwrap_or_default() {
            "left" => leaving = true,
            "transcript" => {
                let speaker = &ev["speaker"];
                let is_agent = speaker.is_null() || speaker["kind"].as_str() == Some("agent");
                if is_agent {
                    if human_lines > 0 && !leaving {
                        rows.push(ev.clone());
                    }
                } else {
                    human_lines += 1;
                    rows.push(ev.clone());
                }
            }
            // "system" (older workers: "<agent> joined the call") and "call_note" are call
            // events: logged, never rows.
            _ => {}
        }
    }
    (rows, human_lines)
}

/// `POST /voice/calls/{id}/events` — what people and the agent say to each other goes to the
/// box as chat rows (`chat_rows_of`); judge decisions are kept for the admin view and the
/// owner's Details drawer; joined/left keep the participant list honest; call notes (joined,
/// left, muted, stop) are logged. Answers `{ accepted, forwarded }`.
pub async fn call_events(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<EventsBody>,
) -> Response {
    if let Err(r) = worker_only(&principal) {
        return *r;
    }
    let (call, project, chat) = match load(&state, &id, b.project_id.as_deref()) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let key = heard_key(&call.id, &project.id);
    let heard_before = VOICE_HEARD
        .lock()
        .map(|h| h.contains(&key))
        .unwrap_or(false);
    let (rows, heard_now) = chat_rows_of(&b.events, usize::from(heard_before));
    if heard_now > 0 && !heard_before {
        if let Ok(mut h) = VOICE_HEARD.lock() {
            h.insert(key.clone());
        }
    }
    let mut accepted = 0usize;
    for ev in &b.events {
        accepted += 1;
        let at = ev["at"].as_i64().unwrap_or_else(now_ms);
        match ev["kind"].as_str().unwrap_or_default() {
            "transcript" => {}
            "system" | "call_note" => {
                tracing::info!(call = %call.id, project = %project.id, note = %ev["text"], "voice: call note");
            }
            "decision" => {
                let d = VoiceDecision {
                    speaker: ev["speaker"].as_str().map(str::to_owned),
                    text: ev["text"].as_str().map(str::to_owned),
                    decision: ev["decision"].as_str().unwrap_or("silent").to_owned(),
                    forced: ev["forced"].as_bool().unwrap_or(false),
                    p_speak: ev["p_speak"].as_f64(),
                    p_act: ev["p_act"].as_f64(),
                    p_silent: ev["p_silent"].as_f64(),
                    provider: ev["provider"].as_str().map(str::to_owned),
                    latency_ms: ev["latency_ms"].as_i64(),
                    cost_usd: ev["cost_usd"].as_f64(),
                };
                let _ = state
                    .db
                    .voice_decision_insert(&call.id, &project.id, at, &d);
            }
            "joined" => {
                let _ = state.db.call_agent_join(&call.id, &project.id);
                let _ = state.db.touch_call(&call.id);
            }
            "left" => {
                let _ = state.db.call_agent_leave(&call.id, &project.id);
                if let Ok(mut h) = VOICE_HEARD.lock() {
                    h.remove(&key);
                }
            }
            "metric" | "tool" | "vision" => {
                tracing::info!(call = %call.id, project = %project.id, event = %ev, "voice event");
            }
            other => tracing::debug!(call = %call.id, kind = other, "voice: unknown event"),
        }
    }
    let mut forwarded = false;
    if !rows.is_empty() {
        forwarded = box_notify(
            &state,
            &project.user_id,
            json!({
                "type": "voice_rows", "call_id": call.id, "chat_id": call.chat_id,
                "chat_project": chat.project_label(), "project_id": project.id, "rows": rows,
            }),
        )
        .await
        .is_ok();
    }
    Json(json!({ "accepted": accepted, "forwarded": forwarded })).into_response()
}

#[derive(Deserialize)]
pub struct UsageBody {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(flatten)]
    pub usage: VoiceUsage,
}

/// Debit one usage report into the payer's account ledger and say what was charged.
///
/// This is where the money of a voice call is taken, and it runs on every report the worker
/// sends — every `ARBOS_VOICE_USAGE_EVERY_S` (30 s) while the call is up, not only on the
/// final one. Only the difference since the last report is charged, so the cumulative figures
/// the worker sends are safe to replay. The camera-vision share goes in as its own line
/// (`ref = call:<id>#vision`) so the breakdown survives.
///
/// Charging as the call runs is what lets the account's one limit end a call: the caller reads
/// the stop straight after this and hands it to the worker. A call charged only at hang-up
/// could not be stopped by any limit, however the limit was written.
fn charge_report(
    state: &AppState,
    call_id: &str,
    project_id: &str,
    payer: &str,
    started_at: i64,
    usage: &VoiceUsage,
    model: &str,
) -> Result<f64, String> {
    let billing = crate::db_voice::VoiceBilling {
        metered: crate::credits::metered(state),
        legacy: crate::db_accounts::LegacyCreditDefaults {
            limit: state.cfg.default_account_limit,
            grant_usd: state.cfg.default_grant_usd,
        },
    };
    let settled = state
        .db
        .settle_voice_usage(call_id, project_id, payer, started_at, usage, &billing)
        .map_err(|error| error.to_string())?;
    if settled.reported_delta <= 0.000_001 {
        return Ok(settled.reported_total);
    }
    for (usd, charge) in settled.charges {
        crate::credits::note_postfact_charge(state, payer, usd, "voice", charge);
    }
    let _ = state.db.record_usage(
        project_id,
        "voice",
        model,
        usage.audio_in_tokens + usage.text_in_tokens,
        usage.audio_out_tokens + usage.text_out_tokens,
        usage.cached_tokens,
        0,
        settled.reported_delta,
        (usage.seconds * 1000.0) as i64,
        usage.ttfa_ms_p50,
    );
    Ok(settled.reported_total)
}

/// `POST /voice/calls/{id}/usage` — cumulative usage for this agent in this call. Debits the
/// difference since the last report into the owner's account ledger ([`charge_report`]) and
/// answers the caps: `{ caps: { stop: null | reason, … }, usd_charged }`. The worker leaves the
/// call on any `stop`, so this is the path a cost limit ends a call through, mid-call.
pub async fn call_usage(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<UsageBody>,
) -> Response {
    if let Err(r) = worker_only(&principal) {
        return *r;
    }
    let (call, project, _) = match load(&state, &id, b.project_id.as_deref()) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let payer = project.user_id.clone();
    let charged = match charge_report(
        &state,
        &call.id,
        &project.id,
        &payer,
        call.started_at,
        &b.usage,
        &state.voice.model,
    ) {
        Ok(v) => v,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e),
    };
    if b.usage.ended {
        tracing::info!(
            call = %call.id, project = %project.id, seconds = b.usage.seconds, usd = b.usage.usd,
            tool_calls = b.usage.tool_calls, decisions = b.usage.decisions, turns = b.usage.turns_spoken,
            ttfa_p50 = ?b.usage.ttfa_ms_p50, barge_p50 = ?b.usage.barge_ms_p50, stop = ?b.usage.stop_reason,
            frames = b.usage.frames, looks = b.usage.looks, vision_seconds = b.usage.vision_seconds,
            vision_usd = b.usage.vision_usd,
            "voice: call usage final"
        );
        let _ = state.db.call_agent_leave(&call.id, &project.id);
        // Tell the box to drop any still-running voice turn so typed chat after hang-up works.
        let _ = box_notify(
            &state,
            &project.user_id,
            json!({
                "type": "voice_cancel",
                "call_id": call.id,
                "chat_id": call.chat_id,
                "project_id": project.id,
            }),
        )
        .await;
    }
    Json(json!({
        "caps": caps_json(&state, &payer, &call.id, &project.id),
        "usd_charged": charged,
    }))
    .into_response()
}

// ---- owner settings -------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct AgentSettingsBody {
    #[serde(default)]
    pub chattiness: Option<String>,
    /// `Some(None)` clears it (JSON `null`).
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub wake_name: Option<Option<String>>,
    /// May the agent see cameras on calls? Default true.
    #[serde(default)]
    pub vision_enabled: Option<bool>,
}

fn deserialize_double_option<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

fn settings_json(s: &crate::db_voice::VoiceAgentSettings) -> Value {
    json!({ "project_id": s.project_id, "chattiness": s.chattiness, "wake_name": s.wake_name, "voice": s.voice, "vision_enabled": s.vision_enabled, "levels": CHATTINESS_LEVELS })
}

/// `GET /projects/{id}/voice` → `{ chattiness, wake_name, voice, vision_enabled }` (owner, guests, the box, the admin).
pub async fn get_agent_settings(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if !state.allows(&principal, &id) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "not your project");
    }
    let Some(p) = state.db.project(&id).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such project");
    };
    match state.db.voice_agent_settings(&p.id) {
        Ok(s) => Json(settings_json(&s)).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `PUT /projects/{id}/voice {chattiness?, wake_name?, vision_enabled?}` (owner or admin).
pub async fn put_agent_settings(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<AgentSettingsBody>,
) -> Response {
    // Someone else's agent reads exactly like one that is not there, so the pair of answers
    // cannot be used to ask which names are taken (reported 2026-11-02). A name is matched
    // against the caller's own agents first, because names are not unique across owners.
    let own = |p: &Project| match &principal {
        Principal::Admin => true,
        Principal::Device { user_id, .. } => *user_id == p.user_id,
        Principal::Box { .. } => false,
    };
    let found = principal
        .user_id()
        .and_then(|u| {
            state
                .db
                .project_of_user_by_name(u, id.trim())
                .ok()
                .flatten()
        })
        .or_else(|| state.db.project(id.trim()).ok().flatten());
    let Some(p) = found.filter(own) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such project");
    };
    if let Some(c) = b.chattiness.as_deref() {
        if !CHATTINESS_LEVELS.contains(&c) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "chattiness must be quiet, balanced or chatty",
            );
        }
    }
    let wake = b.wake_name.as_ref().map(|w| {
        w.as_deref()
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(|w| w.chars().take(32).collect::<String>())
    });
    if let Some(v) = b.vision_enabled {
        if let Err(e) = state.db.set_voice_agent_vision(&p.id, v) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
    }
    match state.db.set_voice_agent_settings(
        &p.id,
        b.chattiness.as_deref(),
        wake.as_ref().map(|w| w.as_deref()),
    ) {
        Ok(s) => Json(settings_json(&s)).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `POST /voice/worker/hello {sha, version, model}` (admin token): the worker says which build
/// it runs; shown as `voice.worker_sha` / `voice.worker` in `GET /admin/calls`.
pub async fn worker_hello(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<Value>,
) -> Response {
    if let Err(r) = worker_only(&principal) {
        return *r;
    }
    let info = json!({
        "sha": b["sha"], "version": b["version"], "model": b["model"], "hello_at": now_ms(),
    });
    tracing::info!(sha = %b["sha"], version = %b["version"], "voice worker hello");
    let _ = state.db.set_voice_worker(&info);
    *state.voice_worker.lock().await = Some(info.clone());
    Json(info).into_response()
}

/// The voice part of `GET /admin/calls`: config and the last agent calls with their cost log.
pub fn admin_json(state: &AppState) -> Value {
    let recent: Vec<Value> = state
        .db
        .voice_calls_recent(20)
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let minutes = r.usage.seconds / 60.0;
            let vision_minutes = r.usage.vision_seconds / 60.0;
            json!({
                "call_id": r.call_id, "project_id": r.project_id, "payer": r.payer, "mode": r.usage.mode,
                "started_at": r.started_at, "ended_at": r.ended_at, "minutes": (minutes * 100.0).round() / 100.0,
                "audio_in_tokens": r.usage.audio_in_tokens, "audio_out_tokens": r.usage.audio_out_tokens,
                "text_in_tokens": r.usage.text_in_tokens, "text_out_tokens": r.usage.text_out_tokens,
                "cached_tokens": r.usage.cached_tokens, "stt_seconds": r.usage.stt_seconds,
                "decisions": r.usage.decisions, "tool_calls": r.usage.tool_calls, "turns_spoken": r.usage.turns_spoken,
                "usd": r.usage.usd, "usd_charged": r.usd_charged,
                "usd_per_minute": if minutes > 0.0 { Some(r.usage.usd / minutes) } else { None },
                "ttfa_ms_p50": r.usage.ttfa_ms_p50, "ttfa_ms_p95": r.usage.ttfa_ms_p95,
                "barge_ms_p50": r.usage.barge_ms_p50, "barge_ms_max": r.usage.barge_ms_max,
                "stop_reason": r.usage.stop_reason,
                "vision": {
                    "minutes": (vision_minutes * 100.0).round() / 100.0,
                    "usd": r.usage.vision_usd, "usd_charged": r.vision_charged,
                    "image_in_tokens": r.usage.image_in_tokens, "frames": r.usage.frames, "looks": r.usage.looks,
                    "usd_per_camera_minute": if vision_minutes > 0.0 { Some(r.usage.vision_usd / vision_minutes) } else { None },
                },
                "decisions_log": state.db.voice_decisions_of(&r.call_id, 50).unwrap_or_default(),
            })
        })
        .collect();
    let (vision_minutes_today, vision_usd_today, frames_today) = state
        .db
        .voice_vision_totals(start_of_utc_day_ms())
        .unwrap_or((0.0, 0.0, 0));
    let (vision_minutes_all, vision_usd_all, frames_all) =
        state.db.voice_vision_totals(0).unwrap_or((0.0, 0.0, 0));
    // Memory first, else the last hello persisted before a relay restart.
    let worker = state
        .voice_worker
        .try_lock()
        .ok()
        .and_then(|w| w.clone())
        .or_else(|| state.db.voice_worker().ok().flatten())
        .unwrap_or(Value::Null);
    json!({
        "enabled": state.voice.enabled && state.livekit.is_some(),
        "worker_sha": worker["sha"].clone(),
        "worker": worker,
        "agent_name": state.voice.agent_name,
        "model": state.voice.model,
        "default_voice": state.voice.default_voice,
        "voices": voice_catalogue(state).len(),
        "call_leak_guard_minutes": CALL_LEAK_GUARD_MINUTES,
        "boxes_linked": state.voice_conns.try_lock().map(|c| c.len()).unwrap_or(0),
        "vision": {
            "today": { "minutes": (vision_minutes_today * 100.0).round() / 100.0, "usd": vision_usd_today, "frames": frames_today },
            "all": { "minutes": (vision_minutes_all * 100.0).round() / 100.0, "usd": vision_usd_all, "frames": frames_all },
        },
        "recent": recent,
    })
}

#[cfg(test)]
mod cost_stop_tests {
    //! What ends a call is the account's one cost-based limit, charged as the call runs. The
    //! regression to guard: the mid-call charge must never end an unlimited account's call.

    use crate::credits::SpendLimit;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    use super::stops_for;

    fn report(seconds: f64, usd: f64) -> crate::db_voice::VoiceUsage {
        crate::db_voice::VoiceUsage {
            seconds,
            usd,
            ..Default::default()
        }
    }

    /// The money of a call is taken while the call is up, not at hang-up. Three interim reports
    /// (`ended` false, as the worker sends them every 30 s) each leave their own ledger row for
    /// the difference, and the stop the worker is handed flips as soon as the account's limit is
    /// reached — which is the whole reason a cost limit can end a call at all.
    #[test]
    fn each_interim_report_is_charged_and_the_limit_bites_mid_call() {
        let state = test_state(AuthMode::Github);
        state.db.set_kill_switch(&Default::default()).unwrap();
        state.db.grant_credits("u_mid", 100.0, "t", "t").unwrap();
        state
            .db
            .set_user_limits("u_mid", Some(SpendLimit::Daily(3.0).stored()), None)
            .unwrap();

        let mut charged = Vec::new();
        for (secs, usd) in [(30.0, 1.0), (60.0, 2.0), (90.0, 3.5)] {
            assert!(!report(secs, usd).ended, "an interim report, not the last");
            charged.push(
                super::charge_report(
                    &state,
                    "call_mid",
                    "p_mid",
                    "u_mid",
                    0,
                    &report(secs, usd),
                    "test-model",
                )
                .expect("charged"),
            );
            if stops_for(&state, "u_mid").is_err() {
                break;
            }
        }
        assert_eq!(
            charged,
            vec![1.0, 2.0, 3.5],
            "each report charges the difference as the call runs"
        );

        let by_kind = state.db.credits("u_mid", 0).unwrap().today_by_kind;
        assert_eq!(by_kind.len(), 1, "one source: {by_kind:?}");
        assert_eq!(by_kind[0].kind, "voice");
        assert_eq!(by_kind[0].usd, 3.5);
        assert_eq!(
            by_kind[0].calls, 3,
            "three ledger rows mid-call, not one at the end"
        );
        assert_eq!(
            stops_for(&state, "u_mid"),
            Err("daily_cap"),
            "the worker's next report is answered with a stop and it leaves the call"
        );
    }

    /// A long, expensive call on an unlimited account keeps going. Every pass of the worker's
    /// usage loop asks this, so a limit that read an unset value as zero would hang up on the
    /// first report instead of never.
    #[test]
    fn an_unlimited_account_is_never_hung_up_on() {
        let state = test_state(AuthMode::Github);
        state.db.set_kill_switch(&Default::default()).unwrap();
        state
            .db
            .grant_credits("u_voice", 1000.0, "test", "test")
            .unwrap();
        state
            .db
            .set_user_limits("u_voice", Some(SpendLimit::Unlimited.stored()), None)
            .unwrap();
        // Two hours of voice at the going rate, charged report by report as the call ran.
        for _ in 0..240 {
            state
                .db
                .debit("u_voice", 0.25, "voice", Some("call:c1"), 0)
                .unwrap()
                .expect("an unlimited account is charged, never refused");
        }
        assert_eq!(
            stops_for(&state, "u_voice"),
            Ok(()),
            "no limit means no reason to leave the call"
        );
    }

    /// A limit set by hand does end the call, and voice counts against the same one limit as
    /// every other source rather than one of its own.
    #[test]
    fn a_hand_set_limit_ends_the_call_mid_way() {
        let state = test_state(AuthMode::Github);
        state.db.set_kill_switch(&Default::default()).unwrap();
        state
            .db
            .grant_credits("u_voice", 1000.0, "test", "test")
            .unwrap();
        state
            .db
            .set_user_limits("u_voice", Some(SpendLimit::Daily(2.0).stored()), None)
            .unwrap();
        state
            .db
            .debit("u_voice", 1.0, "inference", None, 0)
            .unwrap()
            .expect("under the limit");
        assert_eq!(
            stops_for(&state, "u_voice"),
            Ok(()),
            "half spent, the call carries on"
        );
        state
            .db
            .debit("u_voice", 1.0, "voice", Some("call:c1"), 0)
            .unwrap()
            .expect("still under at the moment of charging");
        assert_eq!(
            stops_for(&state, "u_voice"),
            Err("daily_cap"),
            "the account's limit ends the call, reached by voice and inference together"
        );
    }
}

#[cfg(test)]
mod chat_row_tests {
    //! #1430: a call is a call event. Only what people and the agent say to each other
    //! becomes chat rows; the greeting, the goodbye and the join/leave/mute notes do not.

    use serde_json::json;

    use super::chat_rows_of;

    fn agent(text: &str) -> serde_json::Value {
        json!({ "kind": "transcript", "speaker": { "kind": "agent" }, "text": text })
    }

    fn human(text: &str) -> serde_json::Value {
        json!({ "kind": "transcript", "speaker": { "kind": "user", "id": "u1", "handle": "alice" }, "text": text })
    }

    #[test]
    fn greeting_and_notes_are_not_rows() {
        let events = vec![
            json!({ "kind": "joined", "mode": "duplex" }),
            json!({ "kind": "system", "text": "Home joined the call" }),
            json!({ "kind": "call_note", "text": "Home joined the call" }),
            agent("Hey — Home here. What do you need?"),
            json!({ "kind": "metric", "ttfa_ms": 700 }),
        ];
        let (rows, heard) = chat_rows_of(&events, 0);
        assert!(rows.is_empty(), "{rows:?}");
        assert_eq!(heard, 0);
    }

    #[test]
    fn what_people_and_the_agent_say_to_each_other_stays() {
        let events = vec![
            agent("Hey — Home here. What do you need?"),
            human("What is the weather in Lisbon?"),
            agent("Sunny, twenty-two degrees."),
            json!({ "kind": "call_note", "text": "Home muted" }),
        ];
        let (rows, heard) = chat_rows_of(&events, 0);
        assert_eq!(heard, 1);
        let texts: Vec<&str> = rows.iter().map(|r| r["text"].as_str().unwrap()).collect();
        assert_eq!(
            texts,
            vec![
                "What is the weather in Lisbon?",
                "Sunny, twenty-two degrees."
            ]
        );
    }

    #[test]
    fn a_later_batch_remembers_that_someone_spoke_and_the_goodbye_is_audio_only() {
        let (rows, heard) = chat_rows_of(&[agent("Anything else?")], 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(heard, 2);
        let (rows, _) = chat_rows_of(
            &[
                json!({ "kind": "left", "reason": "asked" }),
                agent("Okay, leaving. Bye."),
            ],
            2,
        );
        assert!(rows.is_empty(), "the goodbye is call audio: {rows:?}");
    }
}

#[cfg(test)]
mod inbox_tests {
    //! #1860: what lands in a chat with a live agent call reaches the call. A turn's answer
    //! (`result`), a job's outcome (`job`), a status line (`quick`) and a person's message
    //! (`message`) are queued per (call, agent) from `notify::announce`; the worker's long-poll
    //! gets them in order; the queue goes with the call.

    use std::time::Duration;

    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::Json;
    use serde_json::{json, Value};

    use super::*;
    use crate::calls::{start_people_call, StartBody};
    use crate::notify::NotifyBody;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn body_json(r: Response) -> Value {
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    /// A box's announcement: never a cross-chat reply (`reply_ref` is a person's, #1834).
    fn notify(
        message_id: &str,
        text: &str,
        kind: Option<&str>,
        voice_text: Option<&str>,
    ) -> NotifyBody {
        NotifyBody {
            message_id: message_id.into(),
            text: text.into(),
            project_id: None,
            reply_ref: None,
            kind: kind.map(str::to_owned),
            voice_text: voice_text.map(str::to_owned),
            mentions: Vec::new(),
        }
    }

    #[tokio::test]
    async fn a_job_result_in_the_chat_reaches_the_live_call_and_only_that_call() {
        let mut state = test_state(AuthMode::Github);
        state.livekit = Some(crate::livekit::LiveKit::new(
            "wss://lk.test",
            "APIkey",
            "secret123",
            None,
        ));
        let (alice, alice_dev) = person(&state, 1);
        let (_bob, _) = person(&state, 2);
        let bubble = project_of(&state, &alice, "Bubble");
        let r = crate::people::chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(crate::people::CreateGroupBody {
                kind: "group".into(),
                members: vec!["user2".into()],
                agent_ids: vec![bubble.clone()],
                title: Some("Pizza".into()),
                avatar_disc_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let chat = state
            .db
            .chats_of_member(&format!("user:{alice}"))
            .unwrap()
            .into_iter()
            .find(|cm| cm.chat.kind == "group")
            .expect("the group")
            .chat
            .id;
        let chat_ref = ChatRef::people(&chat);
        let agent = Sender::agent(&state, &state.db.project(&bubble).unwrap().unwrap());

        // Before the call: a bubble is nobody's business here.
        call_update(
            &state,
            &chat_ref,
            &agent,
            &notify("m0", "earlier", Some("result"), None),
        )
        .await;

        let r = start_people_call(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            HeaderMap::new(),
            Some(Json(StartBody {
                kind: Some("voice".into()),
                ring: None,
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::CREATED);
        let call_id = body_json(r).await["call"]["call_id"]
            .as_str()
            .unwrap()
            .to_owned();
        // The worker joined (its `joined` event).
        state.db.call_agent_join(&call_id, &bubble).unwrap();

        // The ask ran as a long turn: a status line, then the job's outcome, then a person types.
        call_update(
            &state,
            &chat_ref,
            &agent,
            &notify("q1", "Still checking Santa Teresa.", Some("quick"), None),
        )
        .await;
        call_update(
            &state,
            &chat_ref,
            &agent,
            &notify("j1", "Most of the numbers come from Onda Teresa's guide.", Some("job"), Some("| Place | Order |\n| Pizza Tomate | WhatsApp |\n\nMost of the numbers come from Onda Teresa's guide.")),
        )
        .await;
        let alice_sender = Sender::user(&state, &alice);
        call_update(
            &state,
            &chat_ref,
            &alice_sender,
            &notify("h1", "thanks!", None, None),
        )
        .await;

        let (items, seq) = state.voice_inbox.take(&call_id, &bubble, 0).await;
        assert_eq!(seq, 3);
        let kinds: Vec<&str> = items.iter().map(|u| u.kind.as_str()).collect();
        assert_eq!(kinds, vec!["quick", "job", "message"]);
        assert_eq!(items[1].message_id, "j1");
        assert!(
            items[1].text.starts_with("| Place |"),
            "the call gets the whole prose, not the push body: {}",
            items[1].text
        );
        assert_eq!(items[2].sender["kind"], "user");
        assert!(
            items.iter().all(|u| u.message_id != "m0"),
            "nothing from before the call"
        );

        // The worker's long-poll: after=2 → the person's line only; after=3 with a short wait → empty.
        let r = call_inbox(
            State(state.clone()),
            Principal::Admin,
            Path(call_id.clone()),
            Query(InboxQuery {
                project_id: Some(bubble.clone()),
                after: 2,
                wait: 0,
            }),
        )
        .await;
        let v = body_json(r).await;
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["kind"], "message");
        assert_eq!(v["seq"], 3);
        assert_eq!(v["live"], true);
        let t0 = std::time::Instant::now();
        let (none, _) = state
            .voice_inbox
            .wait(&call_id, &bubble, 3, Duration::from_millis(60))
            .await;
        assert!(none.is_empty() && t0.elapsed() >= Duration::from_millis(50));

        // A push wakes a waiter.
        let inbox = state.voice_inbox.clone();
        let (c, p) = (call_id.clone(), bubble.clone());
        let waiter =
            tokio::spawn(async move { inbox.wait(&c, &p, 3, Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        call_update(
            &state,
            &chat_ref,
            &agent,
            &notify("r2", "Done: three options.", Some("result"), None),
        )
        .await;
        let (woken, seq) = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seq, 4);
        assert_eq!(woken.len(), 1);
        assert_eq!(woken[0].kind, "result");

        // The call ends: the queue goes with it, and the poll says so.
        state.voice_inbox.close_call(&call_id).await;
        assert!(state
            .voice_inbox
            .take(&call_id, &bubble, 0)
            .await
            .0
            .is_empty());
        let _ = json!({});
    }
}

#[cfg(test)]
mod charge_receipt_regressions {
    use crate::credits::SpendLimit;
    use crate::db_voice::VoiceUsage;
    use crate::state::test_support::test_state;
    use crate::state::{AppState, AuthMode};

    const USER: &str = "fixture-voice-payer";
    const CALL: &str = "fixture-voice-call";
    const PROJECT: &str = "fixture-voice-project";

    fn fixture() -> AppState {
        let state = test_state(AuthMode::Github);
        state.db.set_kill_switch(&Default::default()).unwrap();
        state
            .db
            .grant_credits(USER, 100.0, "synthetic grant", "fixture")
            .unwrap();
        state
            .db
            .set_user_limits(USER, Some(SpendLimit::Unlimited.stored()), None)
            .unwrap();
        state
    }

    fn report(usd: f64, vision_usd: f64) -> VoiceUsage {
        VoiceUsage {
            seconds: usd * 30.0,
            usd,
            vision_usd,
            ..Default::default()
        }
    }

    fn charge(state: &AppState, usage: &VoiceUsage) -> Result<f64, String> {
        super::charge_report(state, CALL, PROJECT, USER, 0, usage, "synthetic-model")
    }

    fn spent(state: &AppState) -> f64 {
        state
            .db
            .credits(USER, 0)
            .unwrap()
            .today_by_kind
            .iter()
            .find(|row| row.kind == "voice")
            .map_or(0.0, |row| row.usd)
    }

    fn skipped_legacy_metadata_remains_retryable(trigger_clause: &str) {
        let mut state = test_state(AuthMode::Github);
        let cfg = std::sync::Arc::get_mut(&mut state.cfg).unwrap();
        cfg.default_account_limit = SpendLimit::Daily(0.5);
        cfg.default_grant_usd = 100.0;
        state
            .db
            .with(|connection| {
                connection.execute_batch(&format!(
                    "CREATE TRIGGER skip_legacy_metadata {trigger_clause} ON users
                 BEGIN SELECT RAISE(IGNORE); END;"
                ))
            })
            .unwrap();
        let first = charge(&state, &report(1.0, 0.0));
        let user_after_failure = state.db.user(USER).unwrap();
        let spent_after_failure = spent(&state);
        let row_after_failure = state.db.voice_call(CALL, PROJECT).unwrap();
        state
            .db
            .with(|connection| connection.execute_batch("DROP TRIGGER skip_legacy_metadata"))
            .unwrap();
        assert_eq!(charge(&state, &report(1.0, 0.0)).unwrap(), 1.0);
        assert_eq!(charge(&state, &report(2.0, 0.0)).unwrap(), 2.0);
        assert!(
            first.is_err(),
            "skipped required legacy metadata must fail settlement"
        );
        assert!(
            user_after_failure.is_none(),
            "legacy initialization must roll back"
        );
        assert!(
            row_after_failure.is_none(),
            "unsettled observation must roll back"
        );
        assert_eq!(spent_after_failure, 0.0);
        assert_eq!(
            state.db.user(USER).unwrap().unwrap().daily_cap_usd,
            Some(0.5)
        );
        assert_eq!(
            spent(&state),
            1.0,
            "preserve the daily soft cap after successful retry"
        );
        assert_eq!(state.db.credits(USER, 0).unwrap().granted_usd, 100.0);
    }

    #[test]
    fn skipped_legacy_cap_update_cannot_commit_a_charge() {
        skipped_legacy_metadata_remains_retryable("BEFORE UPDATE OF spend_cap_usd");
    }

    #[test]
    fn skipped_legacy_user_insert_cannot_commit_a_charge() {
        skipped_legacy_metadata_remains_retryable("BEFORE INSERT");
    }

    #[test]
    fn a_failed_charge_receipt_cannot_double_debit_a_replayed_report() {
        let state = fixture();
        state
            .db
            .with(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_voice_receipt BEFORE UPDATE OF usd_charged ON voice_calls
             BEGIN SELECT RAISE(ABORT, 'synthetic receipt failure'); END;",
                )
            })
            .unwrap();
        let first = charge(&state, &report(1.0, 0.0));
        let after_failure = spent(&state);
        state
            .db
            .with(|connection| connection.execute_batch("DROP TRIGGER fail_voice_receipt"))
            .unwrap();
        let retry = charge(&state, &report(1.0, 0.0)).unwrap();
        assert_eq!(
            spent(&state),
            1.0,
            "replaying one cumulative dollar must debit one dollar total"
        );
        assert_eq!(retry, 1.0);
        assert!(first.is_err(), "failed durable settlement must be reported");
        assert_eq!(
            after_failure, 0.0,
            "failed settlement must roll back its ledger debit"
        );
    }

    #[test]
    fn a_failed_ledger_write_cannot_mark_an_unpaid_report_settled() {
        let state = fixture();
        state
            .db
            .with(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_voice_debit BEFORE INSERT ON credit_ledger
             WHEN NEW.kind = 'voice' AND NEW.usd < 0
             BEGIN SELECT RAISE(ABORT, 'synthetic ledger failure'); END;",
                )
            })
            .unwrap();
        let first = charge(&state, &report(1.0, 0.0));
        state
            .db
            .with(|connection| connection.execute_batch("DROP TRIGGER fail_voice_debit"))
            .unwrap();
        let retry = charge(&state, &report(1.0, 0.0)).unwrap();
        assert_eq!(
            spent(&state),
            1.0,
            "a retry must settle a report whose ledger debit failed"
        );
        assert_eq!(retry, 1.0);
        assert!(
            first.is_err(),
            "ledger failure must not look like a settled report"
        );
    }

    #[test]
    fn replayed_and_reordered_cumulative_reports_keep_voice_vision_totals() {
        let state = fixture();
        for usage in [
            report(1.0, 0.25),
            report(1.0, 0.25),
            report(0.5, 0.1),
            report(2.0, 0.5),
        ] {
            charge(&state, &usage).unwrap();
        }
        let row = state.db.voice_call(CALL, PROJECT).unwrap().unwrap();
        assert_eq!(spent(&state), 2.0);
        assert_eq!(row.usd_charged, 2.0);
        assert_eq!(row.vision_charged, 0.5);
        let credits = state.db.credits(USER, 0).unwrap();
        assert_eq!(
            credits
                .today_by_kind
                .iter()
                .find(|row| row.kind == "voice")
                .unwrap()
                .calls,
            4
        );
    }

    #[test]
    fn the_existing_balance_clamp_keeps_reported_and_debited_amounts_distinct() {
        let state = fixture();
        assert_eq!(charge(&state, &report(150.0, 0.0)).unwrap(), 150.0);
        assert_eq!(charge(&state, &report(150.0, 0.0)).unwrap(), 150.0);
        assert_eq!(spent(&state), 100.0);
        assert_eq!(
            state
                .db
                .voice_call(CALL, PROJECT)
                .unwrap()
                .unwrap()
                .usd_charged,
            150.0
        );
        assert_eq!(state.db.credits(USER, 0).unwrap().balance_usd, 0.0);
    }
    #[test]
    fn preserved_zero_delta_does_not_initialize_a_legacy_account() {
        let state = test_state(AuthMode::Github);
        assert!(state.db.user(USER).unwrap().is_none());
        assert_eq!(charge(&state, &report(0.0, 0.0)).unwrap(), 0.0);
        assert!(state.db.user(USER).unwrap().is_none());
        assert_eq!(spent(&state), 0.0);
    }

    #[test]
    fn preserved_unmetered_reports_settle_without_a_grant_or_ledger_debit() {
        let state = test_state(AuthMode::Paircode);
        assert_eq!(charge(&state, &report(1.0, 0.25)).unwrap(), 1.0);
        assert!(state.db.user(USER).unwrap().is_none());
        assert_eq!(spent(&state), 0.0);
        let row = state.db.voice_call(CALL, PROJECT).unwrap().unwrap();
        assert_eq!((row.usd_charged, row.vision_charged), (1.0, 0.25));
    }

    #[test]
    fn preserved_total_and_per_part_thresholds_initialize_only_chargeable_usage() {
        for (usd, vision_usd, settled, chargeable) in [
            (0.9e-6, 0.0, 0.0, false),
            (1e-6, 0.0, 0.0, false),
            (1.2e-6, 0.6e-6, 1.2e-6, false),
            (2e-6, 1e-6, 2e-6, false),
            (2.2e-6, 1.1e-6, 2.2e-6, true),
        ] {
            let state = test_state(AuthMode::Github);
            assert_eq!(charge(&state, &report(usd, vision_usd)).unwrap(), settled);
            assert_eq!(state.db.user(USER).unwrap().is_some(), chargeable);
            assert_eq!(spent(&state), if chargeable { usd } else { 0.0 });
        }
    }

    #[test]
    fn preserved_first_metered_part_grants_a_legacy_account_only_once() {
        let state = test_state(AuthMode::Github);
        assert!(state.db.user(USER).unwrap().is_none());
        assert_eq!(charge(&state, &report(1.0, 0.25)).unwrap(), 1.0);
        assert_eq!(charge(&state, &report(1.0, 0.25)).unwrap(), 1.0);
        let credits = state.db.credits(USER, 0).unwrap();
        assert_eq!(credits.granted_usd, state.cfg.default_grant_usd);
        assert_eq!(credits.balance_usd, state.cfg.default_grant_usd - 1.0);
        assert_eq!(spent(&state), 1.0);
    }

    #[test]
    fn preserved_zero_default_grant_creates_an_unfunded_legacy_account() {
        let mut state = test_state(AuthMode::Github);
        std::sync::Arc::get_mut(&mut state.cfg)
            .unwrap()
            .default_grant_usd = 0.0;
        assert_eq!(charge(&state, &report(1.0, 0.0)).unwrap(), 1.0);
        assert!(state.db.user(USER).unwrap().is_some());
        let credits = state.db.credits(USER, 0).unwrap();
        assert_eq!(
            (credits.granted_usd, credits.balance_usd, spent(&state)),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            state
                .db
                .voice_call(CALL, PROJECT)
                .unwrap()
                .unwrap()
                .usd_charged,
            1.0
        );
    }

    #[test]
    fn preserved_voice_part_precedes_vision_at_the_daily_cap_boundary() {
        let state = fixture();
        state
            .db
            .set_user_limits(USER, Some(SpendLimit::Daily(1.0).stored()), None)
            .unwrap();
        assert_eq!(charge(&state, &report(1.25, 0.25)).unwrap(), 1.25);
        // Voice reaches the soft cap first; vision is a policy refusal, not a failed Tx.
        assert_eq!(spent(&state), 1.0);
        let row = state.db.voice_call(CALL, PROJECT).unwrap().unwrap();
        assert_eq!((row.usd_charged, row.vision_charged), (1.25, 0.25));
        assert_eq!(charge(&state, &report(1.25, 0.25)).unwrap(), 1.25);
        assert_eq!(spent(&state), 1.0);
    }

    #[test]
    fn concurrent_cumulative_reports_have_one_durable_settlement() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, Condvar, Mutex};

        struct Snapshots {
            arrivals: Mutex<usize>,
            ready: Condvar,
            outside_reads: AtomicUsize,
            timed_out: AtomicBool,
        }
        impl Snapshots {
            fn meet(&self) {
                let Ok(mut arrived) = self.arrivals.lock() else {
                    self.timed_out.store(true, Ordering::Relaxed);
                    return;
                };
                *arrived += 1;
                if *arrived >= 2 {
                    self.ready.notify_all();
                    return;
                }
                match self.ready.wait_timeout_while(
                    arrived,
                    std::time::Duration::from_secs(2),
                    |count| *count < 2,
                ) {
                    Ok((count, _)) if *count >= 2 => {}
                    _ => self.timed_out.store(true, Ordering::Relaxed),
                }
            }
        }

        // SAFETY: SQLite owns the statement for this callback; registered Arc contexts
        // stay alive until unregistered under their connection lock. No panic or write
        // occurs in the callback. The rendezvous is bounded and only applies outside Tx.
        unsafe extern "C" fn pause_unsettled_snapshot(
            kind: u32,
            context: *mut std::ffi::c_void,
            statement: *mut std::ffi::c_void,
            _elapsed: *mut std::ffi::c_void,
        ) -> i32 {
            if kind != rusqlite::ffi::SQLITE_TRACE_PROFILE as u32
                || context.is_null()
                || statement.is_null()
            {
                return 0;
            }
            // SAFETY: PROFILE supplies its live sqlite3_stmt; context has Arc ownership below.
            let (snapshot, statement) = unsafe {
                (
                    &*context.cast::<Snapshots>(),
                    statement.cast::<rusqlite::ffi::sqlite3_stmt>(),
                )
            };
            // SAFETY: querying the owning handle/autocommit and static SQL does not step or mutate it.
            let (connection, sql) = unsafe {
                (
                    rusqlite::ffi::sqlite3_db_handle(statement),
                    rusqlite::ffi::sqlite3_sql(statement),
                )
            };
            if connection.is_null() || sql.is_null() {
                return 0;
            }
            let needle = b"FROM voice_calls WHERE call_id = ?1 AND project_id = ?2";
            // SAFETY: SQLite's returned SQL is NUL-terminated for the live statement.
            let selected = unsafe { std::ffi::CStr::from_ptr(sql) }
                .to_bytes()
                .windows(needle.len())
                .any(|part| part == needle);
            // SAFETY: this is the callback's still-owned connection, not a new DB operation.
            if selected
                && unsafe { rusqlite::ffi::sqlite3_get_autocommit(connection) } != 0
                && snapshot.outside_reads.fetch_add(1, Ordering::Relaxed) < 2
            {
                snapshot.meet();
            }
            0
        }

        struct Registration {
            db: crate::db::Db,
            _context: Arc<Snapshots>,
        }
        impl Registration {
            fn install(db: &crate::db::Db, context: &Arc<Snapshots>) -> Self {
                db.with(|connection| {
                    connection.busy_timeout(std::time::Duration::from_secs(2))?;
                    // SAFETY: Registration retains both Db and Arc; Drop unregisters before either is released.
                    let code = unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(
                            connection.handle(),
                            rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,
                            Some(pause_unsettled_snapshot),
                            Arc::as_ptr(context).cast_mut().cast(),
                        )
                    };
                    assert_eq!(code, rusqlite::ffi::SQLITE_OK);
                    Ok(())
                })
                .unwrap();
                Self {
                    db: db.clone(),
                    _context: context.clone(),
                }
            }
        }
        impl Drop for Registration {
            fn drop(&mut self) {
                let _ = self.db.with(|connection| {
                    // SAFETY: Db lock waits for active callbacks; unregister before dropping Arc context.
                    unsafe {
                        rusqlite::ffi::sqlite3_trace_v2(
                            connection.handle(),
                            0,
                            None,
                            std::ptr::null_mut(),
                        )
                    };
                    Ok(())
                });
            }
        }

        for legacy in [false, true] {
            let first = if legacy {
                test_state(AuthMode::Github)
            } else {
                fixture()
            };
            let mut second = first.clone();
            second.db = crate::db::Db::open(&first.cfg.data_dir.join("relay.db")).unwrap();
            let snapshots = Arc::new(Snapshots {
                arrivals: Mutex::new(0),
                ready: Condvar::new(),
                outside_reads: AtomicUsize::new(0),
                timed_out: AtomicBool::new(false),
            });
            let registrations = [
                Registration::install(&first.db, &snapshots),
                Registration::install(&second.db, &snapshots),
            ];
            let start = Arc::new(Barrier::new(2));
            let workers: Vec<_> = [first.clone(), second]
                .into_iter()
                .map(|state| {
                    let start = start.clone();
                    std::thread::spawn(move || {
                        start.wait();
                        charge(&state, &report(1.0, 0.0))
                    })
                })
                .collect();
            let outcomes: Vec<_> = workers
                .into_iter()
                .map(|worker| worker.join().unwrap().unwrap())
                .collect();
            drop(registrations);
            assert!(
                !snapshots.timed_out.load(Ordering::Relaxed),
                "native snapshot rendezvous timed out"
            );
            assert_eq!(outcomes, vec![1.0, 1.0]);
            assert_eq!(
                spent(&first),
                1.0,
                "one cumulative report, outside-Tx snapshots={}",
                snapshots.outside_reads.load(Ordering::Relaxed)
            );
            assert_eq!(
                first
                    .db
                    .voice_call(CALL, PROJECT)
                    .unwrap()
                    .unwrap()
                    .usd_charged,
                1.0
            );
            assert_eq!(charge(&first, &report(1.0, 0.0)).unwrap(), 1.0);
            assert_eq!(spent(&first), 1.0);
            assert_eq!(
                first.db.credits(USER, 0).unwrap().granted_usd,
                first.cfg.default_grant_usd,
                "legacy grant must occur once"
            );
        }
    }

    #[test]
    fn ignored_receipt_write_cannot_double_debit_a_replayed_report() {
        let state = fixture();
        state
            .db
            .with(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_voice_receipt BEFORE UPDATE OF usd_charged ON voice_calls
             BEGIN SELECT RAISE(IGNORE); END;",
                )
            })
            .unwrap();
        let first = charge(&state, &report(1.0, 0.0));
        let after_failure = spent(&state);
        state
            .db
            .with(|connection| connection.execute_batch("DROP TRIGGER fail_voice_receipt"))
            .unwrap();
        let retry = charge(&state, &report(1.0, 0.0)).unwrap();
        assert_eq!(
            spent(&state),
            1.0,
            "replaying one cumulative dollar must debit one dollar total"
        );
        assert_eq!(retry, 1.0);
        assert!(first.is_err(), "failed durable settlement must be reported");
        assert_eq!(
            after_failure, 0.0,
            "failed settlement must roll back its ledger debit"
        );
    }

    #[test]
    fn ignored_ledger_write_cannot_mark_an_unpaid_report_settled() {
        let state = fixture();
        state
            .db
            .with(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_voice_debit BEFORE INSERT ON credit_ledger
             WHEN NEW.kind = 'voice' AND NEW.usd < 0
             BEGIN SELECT RAISE(IGNORE); END;",
                )
            })
            .unwrap();
        let first = charge(&state, &report(1.0, 0.0));
        state
            .db
            .with(|connection| connection.execute_batch("DROP TRIGGER fail_voice_debit"))
            .unwrap();
        let retry = charge(&state, &report(1.0, 0.0)).unwrap();
        assert_eq!(
            spent(&state),
            1.0,
            "a retry must settle a report whose ledger debit failed"
        );
        assert_eq!(retry, 1.0);
        assert!(
            first.is_err(),
            "ledger failure must not look like a settled report"
        );
    }
    /// Finite generated execution of the actual transaction/helpers, not a policy model.
    #[test]
    fn generated_atomic_settlement_and_replay_laws() {
        let mut seed = 0x5eed_5110_u64;
        for case in 0..32 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let clamped = case % 2 == 0;
            let usd = if clamped {
                101.0 + ((seed >> 8) % 64) as f64
            } else {
                0.25 * (1 + (seed >> 8) % 32) as f64
            };
            let usage = report(usd, usd * 0.25);
            let state = fixture();
            let trigger = match case % 4 {
                0 => "CREATE TRIGGER fixture_fault BEFORE INSERT ON credit_ledger WHEN NEW.kind='voice' AND NEW.usd<0 BEGIN SELECT RAISE(ABORT,'synthetic ledger'); END",
                1 => "CREATE TRIGGER fixture_fault BEFORE UPDATE OF usd_charged ON voice_calls BEGIN SELECT RAISE(ABORT,'synthetic receipt'); END",
                2 => "CREATE TRIGGER fixture_fault BEFORE INSERT ON credit_ledger WHEN NEW.kind='voice' AND NEW.usd<0 BEGIN SELECT RAISE(IGNORE); END",
                _ => "CREATE TRIGGER fixture_fault BEFORE UPDATE OF usd_charged ON voice_calls BEGIN SELECT RAISE(IGNORE); END",
            };
            state
                .db
                .with(|connection| connection.execute_batch(trigger))
                .unwrap();
            assert!(
                charge(&state, &usage).is_err(),
                "fault must not produce a receipt, case={case} seed={seed}"
            );
            assert_eq!(
                spent(&state),
                0.0,
                "ledger rollback, case={case} seed={seed}"
            );
            assert!(
                state.db.voice_call(CALL, PROJECT).unwrap().is_none(),
                "observation/cursor rollback, case={case} seed={seed}"
            );
            state
                .db
                .with(|connection| connection.execute_batch("DROP TRIGGER fixture_fault"))
                .unwrap();
            assert_eq!(charge(&state, &usage).unwrap(), usd);
            assert_eq!(
                charge(&state, &report(usd * 0.5, usd * 0.125)).unwrap(),
                usd
            );
            assert_eq!(charge(&state, &usage).unwrap(), usd);
            assert_eq!(
                spent(&state),
                if clamped { 100.0 } else { usd },
                "retry/replay conservation, case={case} seed={seed}"
            );
            let row = state.db.voice_call(CALL, PROJECT).unwrap().unwrap();
            assert_eq!((row.usd_charged, row.vision_charged), (usd, usd * 0.25));
            assert_eq!(state.db.credits(USER, 0).unwrap().granted_usd, 100.0);
        }
    }

    #[test]
    fn actual_usage_protocol_rejects_wrong_shapes_without_creating_credit_authority() {
        for raw in [
            r#"{"usd":[]}"#,
            r#"{"usd":{}}"#,
            r#"{"usd":"NaN"}"#,
            r#"{"usd":null}"#,
            r#"{"usd":1e400}"#,
            r#"{"project_id":[]}"#,
        ] {
            assert!(
                serde_json::from_str::<super::UsageBody>(raw).is_err(),
                "malformed synthetic usage accepted"
            );
        }
        let state = test_state(AuthMode::Github);
        let empty = serde_json::from_str::<super::UsageBody>("{}").unwrap();
        assert_eq!(charge(&state, &empty.usage).unwrap(), 0.0);
        // Negative cumulative input remains the existing MAX-at-zero observation policy.
        let negative = serde_json::from_str::<super::UsageBody>(r#"{"usd":-1}"#).unwrap();
        assert_eq!(charge(&state, &negative.usage).unwrap(), 0.0);
        for amount in [f64::NAN, f64::INFINITY, -1.0] {
            crate::credits::charge(&state, USER, amount, "voice", Some("synthetic-invalid"));
            assert!(state.db.debit(USER, amount, "voice", None, 0).is_err());
        }
        assert!(state.db.user(USER).unwrap().is_none());
        assert_eq!(spent(&state), 0.0);
    }
}
