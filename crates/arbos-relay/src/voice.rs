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
//! Cost rails: per-call cap (`ARBOS_VOICE_CALL_CAP_USD`, default $1.50), per-user daily voice cap
//! (`ARBOS_VOICE_DAILY_CAP_USD`, default $5, `PUT /admin/users/{id}/voice` per user), the user's
//! own credits/daily cap and the global kill switch. Owner setting per agent: chattiness
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
/// Hard stop on one call with an agent in it (spec §6).
pub const CALL_MAX_MINUTES: i64 = 60;

/// Voice configuration from the environment.
#[derive(Debug, Clone)]
pub struct VoiceCfg {
    /// LiveKit agent name the worker registers with (`ARBOS_VOICE_AGENT_NAME`).
    pub agent_name: String,
    /// `ARBOS_VOICE_DAILY_CAP_USD` (default 5.0): per user, over every agent they own.
    pub daily_cap_usd: f64,
    /// `ARBOS_VOICE_CALL_CAP_USD` (default 1.50): per agent per call.
    pub call_cap_usd: f64,
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
            daily_cap_usd: 5.0,
            call_cap_usd: 1.5,
            enabled: true,
            model: "gpt-realtime-2.1".to_owned(),
            voices_dir: std::path::PathBuf::from("voices"),
            default_voice: "marin".to_owned(),
        }
    }
}

fn env_f64(k: &str, default: f64) -> f64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(default)
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
        daily_cap_usd: env_f64("ARBOS_VOICE_DAILY_CAP_USD", d.daily_cap_usd),
        call_cap_usd: env_f64("ARBOS_VOICE_CALL_CAP_USD", d.call_cap_usd),
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
        agent = %cfg.agent_name, model = %cfg.model, daily_cap = cfg.daily_cap_usd,
        call_cap = cfg.call_cap_usd, enabled = cfg.enabled, voices = %cfg.voices_dir.display(),
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
fn voice_chat_title(state: &AppState, chat: &ChatRef) -> String {
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
            let handles: Vec<String> = state
                .db
                .members_of(&chat.chat_id)
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.user_id())
                .map(|u| format!("@{}", Sender::user(state, u).handle))
                .filter(|h| h != "@")
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
        if let Err(stop) = stops_for(state, &payer, &call.id, &project.id) {
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

/// Why the agent must not (keep) talk(ing): `kill_switch`, `credits`, `daily_cap`,
/// `voice_daily_cap`, `call_cap`. `Ok` while every rail has room.
pub fn stops_for(
    state: &AppState,
    payer: &str,
    call_id: &str,
    project_id: &str,
) -> Result<(), &'static str> {
    if crate::credits::check_read_only(state).is_err() {
        return Err("kill_switch");
    }
    match crate::credits::check(state, payer) {
        Ok(()) => {}
        Err(crate::db_accounts::DebitRefused::DailyCap { .. }) => return Err("daily_cap"),
        Err(crate::db_accounts::DebitRefused::ReadOnly { .. }) => return Err("kill_switch"),
        Err(crate::db_accounts::DebitRefused::Insufficient { .. }) => return Err("credits"),
    }
    if crate::credits::metered(state) {
        let cap = state
            .db
            .voice_user_cap(payer)
            .ok()
            .flatten()
            .unwrap_or(state.voice.daily_cap_usd);
        let spent = state
            .db
            .voice_spent_since(payer, start_of_utc_day_ms())
            .unwrap_or(0.0);
        if spent >= cap {
            return Err("voice_daily_cap");
        }
    }
    if let Ok(Some(row)) = state.db.voice_call(call_id, project_id) {
        if row.usage.usd >= state.voice.call_cap_usd {
            return Err("call_cap");
        }
    }
    Ok(())
}

fn caps_json(state: &AppState, payer: &str, call_id: &str, project_id: &str) -> Value {
    let cap = state
        .db
        .voice_user_cap(payer)
        .ok()
        .flatten()
        .unwrap_or(state.voice.daily_cap_usd);
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
        "call_cap_usd": state.voice.call_cap_usd,
        "call_spent_usd": call_usd,
        "daily_cap_usd": cap,
        "daily_spent_usd": spent,
        "call_max_minutes": CALL_MAX_MINUTES,
        "metered": crate::credits::metered(state),
        "stop": stops_for(state, payer, call_id, project_id).err(),
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
    if let Err(stop) = stops_for(&state, &project.user_id, &call.id, &project.id) {
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

/// `POST /voice/calls/{id}/usage` — cumulative usage for this agent in this call. The relay
/// debits the difference since the last report from the owner's ledger (`voice`,
/// `ref = call:<id>`; the camera-vision part as its own line, `ref = call:<id>#vision`),
/// mirrors it into the project's `usage` table (so the project's spend cap counts it), and
/// answers the caps: `{ caps: { stop: null | reason, … }, usd_charged }`.
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
    let row =
        match state
            .db
            .voice_usage_upsert(&call.id, &project.id, &payer, call.started_at, &b.usage)
        {
            Ok(r) => r,
            Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        };
    let delta = row.usage.usd - row.usd_charged;
    if delta > 0.000_001 {
        let vision_delta = (row.usage.vision_usd - row.vision_charged).clamp(0.0, delta);
        let voice_delta = delta - vision_delta;
        let reference = format!("call:{}", call.id);
        if voice_delta > 0.000_001 {
            crate::credits::charge(&state, &payer, voice_delta, "voice", Some(&reference));
        }
        if vision_delta > 0.000_001 {
            let vision_ref = format!("{reference}#vision");
            crate::credits::charge(&state, &payer, vision_delta, "voice", Some(&vision_ref));
        }
        let _ = state
            .db
            .voice_charged(&call.id, &project.id, delta, vision_delta);
        let _ = state.db.record_usage(
            &project.id,
            "voice",
            &state.voice.model,
            b.usage.audio_in_tokens + b.usage.text_in_tokens,
            b.usage.audio_out_tokens + b.usage.text_out_tokens,
            b.usage.cached_tokens,
            0,
            delta,
            (b.usage.seconds * 1000.0) as i64,
            b.usage.ttfa_ms_p50,
        );
    }
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
        "usd_charged": row.usd_charged + delta.max(0.0),
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
    let Some(p) = state.db.project(&id).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such project");
    };
    let owner = match &principal {
        Principal::Admin => true,
        Principal::Device { user_id, .. } => *user_id == p.user_id,
        Principal::Box { .. } => false,
    };
    if !owner {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only the agent's owner changes its voice settings",
        );
    }
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

#[derive(Deserialize)]
pub struct UserCapBody {
    /// `null` returns the user to the relay default.
    pub daily_cap_usd: Option<f64>,
}

/// `PUT /admin/users/{id}/voice {daily_cap_usd}` → `{ user_id, daily_cap_usd, default }`.
pub async fn put_user_cap(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<UserCapBody>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    if let Some(c) = b.daily_cap_usd {
        if !c.is_finite() || c < 0.0 {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "daily_cap_usd must be a non-negative number",
            );
        }
    }
    if let Err(e) = state.db.set_voice_user_cap(&id, b.daily_cap_usd) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    Json(json!({
        "user_id": id, "daily_cap_usd": b.daily_cap_usd.unwrap_or(state.voice.daily_cap_usd),
        "default": b.daily_cap_usd.is_none(),
        "spent_today_usd": state.db.voice_spent_since(&id, start_of_utc_day_ms()).unwrap_or(0.0),
    }))
    .into_response()
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
        "daily_cap_usd": state.voice.daily_cap_usd,
        "call_cap_usd": state.voice.call_cap_usd,
        "call_max_minutes": CALL_MAX_MINUTES,
        "boxes_linked": state.voice_conns.try_lock().map(|c| c.len()).unwrap_or(0),
        "vision": {
            "today": { "minutes": (vision_minutes_today * 100.0).round() / 100.0, "usd": vision_usd_today, "frames": frames_today },
            "all": { "minutes": (vision_minutes_all * 100.0).round() / 100.0, "usd": vision_usd_all, "frames": frames_all },
        },
        "recent": recent,
    })
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
                avatar_leaf: None,
                avatar_leaf_color: None,
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
