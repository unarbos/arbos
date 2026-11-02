//! Channels (F7): reach a Project from Slack, Telegram, WhatsApp (Cloud API) and Beeper (through
//! the Harness Comms API). The relay owns the platform adapters; a box only ever sees a
//! `channel_message` push and answers with `POST /projects/{id}/channels/reply`.
//!
//! Identity: an external (user, chat) pair must be bound to a project chat first. Settings →
//! Channels mints a 6-digit code; the user sends `/pair <code>` from the channel. Unbound
//! senders get one hint reply and never reach a box (invariant test below).
//!
//! `ARBOS_CHANNELS_MOCK=1`: sends land in `channel_outbox` instead of the platform API and
//! `POST /channels/mock/inbound` injects messages, so the whole loop runs without credentials.

pub mod beeper;
pub mod slack;
pub mod telegram;
pub mod whatsapp;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{
    ChannelBindingDto, ChannelPairingDto, ChannelPlatformDto, ChannelsOverviewDto,
};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::boxes;
use crate::db::{ChannelBinding, ChannelPairCode};
use crate::state::AppState;

type ApiResult = Result<Response, (StatusCode, String)>;

const PAIR_CODE_TTL_MS: i64 = 10 * 60 * 1000;
pub const HINT_TEXT: &str =
    "Pair this chat in Arbos → Settings → Channels, then send /pair <code> here.";
/// Platform text limits are all above this; longer replies are cut with a marker.
const MAX_REPLY_CHARS: usize = 3800;

fn internal(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub fn mock_on(state: &AppState) -> bool {
    state.cfg.channels_mock
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Slack,
    Telegram,
    Whatsapp,
    Beeper,
}

impl Platform {
    pub const ALL: [Platform; 4] = [
        Platform::Slack,
        Platform::Telegram,
        Platform::Whatsapp,
        Platform::Beeper,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "slack" => Some(Platform::Slack),
            "telegram" => Some(Platform::Telegram),
            "whatsapp" => Some(Platform::Whatsapp),
            "beeper" => Some(Platform::Beeper),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Slack => "slack",
            Platform::Telegram => "telegram",
            Platform::Whatsapp => "whatsapp",
            Platform::Beeper => "beeper",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::Slack => "Slack",
            Platform::Telegram => "Telegram",
            Platform::Whatsapp => "WhatsApp (Cloud API)",
            Platform::Beeper => "Beeper (Harness Comms)",
        }
    }
}

/// Settings → Channels row state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformState {
    Configured,
    Missing,
    Mock,
}

impl PlatformState {
    fn as_str(self) -> &'static str {
        match self {
            PlatformState::Configured => "configured",
            PlatformState::Missing => "missing",
            PlatformState::Mock => "mock",
        }
    }
}

/// Credential state of one platform plus a one-line detail (what is missing, or how inbound works).
pub fn platform_state(state: &AppState, p: Platform) -> (PlatformState, String) {
    if mock_on(state) {
        return (
            PlatformState::Mock,
            "ARBOS_CHANNELS_MOCK=1 · inbound via POST /channels/mock/inbound".into(),
        );
    }
    let (ok, detail) = match p {
        Platform::Slack => slack::status(state),
        Platform::Telegram => telegram::status(state),
        Platform::Whatsapp => whatsapp::status(state),
        Platform::Beeper => beeper::status(state),
    };
    (
        if ok {
            PlatformState::Configured
        } else {
            PlatformState::Missing
        },
        detail,
    )
}

/// One message arriving from a platform, normalised by its adapter.
#[derive(Clone, Debug)]
pub struct Inbound {
    pub platform: Platform,
    pub external_chat_id: String,
    pub external_user_id: String,
    pub sender_name: String,
    pub text: String,
    /// Platform event id for webhook retries; `None` when the platform has none.
    pub event_key: Option<String>,
    /// `false` for senders who may never pair or receive the hint (Beeper: anyone who is not the
    /// account owner, so Arbos never writes to the user's contacts from their own account).
    pub trusted_sender: bool,
    /// Human label for the binding list ("Telegram · Saved Messages").
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Same platform event seen before (webhook retry).
    Duplicate,
    /// Dropped without a reply (empty text, untrusted sender, second message from an unbound sender).
    Ignored(&'static str),
    /// First message from an unbound sender: the pairing hint went back, nothing reached a box.
    Hinted,
    /// `/pair` with a bad or expired code; the sender was told, nothing reached a box.
    PairFailed,
    Paired {
        binding_id: String,
    },
    Delivered {
        binding_id: String,
        project_id: String,
    },
}

/// The one entry point every adapter calls. Only `Delivered` enqueues a push.
pub async fn handle_inbound(state: &AppState, inb: Inbound) -> Outcome {
    if let Some(key) = &inb.event_key {
        match state.db.channel_event_first_seen(key) {
            Ok(true) => {}
            Ok(false) => return Outcome::Duplicate,
            Err(e) => tracing::warn!(error = %e, "channel dedup"),
        }
    }
    let text = inb.text.trim();
    if text.is_empty() {
        return Outcome::Ignored("empty");
    }
    let platform = inb.platform.as_str();

    // Pairing: `/pair 123456` (also accepts "pair 123456").
    if let Some(code) = pair_code_in(text) {
        if !inb.trusted_sender {
            return Outcome::Ignored("untrusted sender");
        }
        match state.db.redeem_channel_pair_code(&code) {
            Ok(Some(pc)) => {
                let binding = ChannelBinding {
                    id: ulid(),
                    platform: platform.to_owned(),
                    external_chat_id: inb.external_chat_id.clone(),
                    external_user_id: inb.external_user_id.clone(),
                    project_id: pc.project_id.clone(),
                    chat_id: pc.chat_id.clone(),
                    label: if inb.label.is_empty() {
                        inb.sender_name.clone()
                    } else {
                        inb.label.clone()
                    },
                    bound_at: now_ms(),
                };
                if let Err(e) = state.db.upsert_channel_binding(&binding) {
                    tracing::warn!(error = %e, "channel binding");
                    return Outcome::Ignored("binding failed");
                }
                // The upsert may have kept an older row id for the same (platform, chat, user).
                let binding = state
                    .db
                    .channel_binding_for(platform, &inb.external_chat_id, &inb.external_user_id)
                    .ok()
                    .flatten()
                    .unwrap_or(binding);
                let project = state
                    .db
                    .project(&pc.project_id)
                    .ok()
                    .flatten()
                    .map(|p| p.name)
                    .unwrap_or_else(|| pc.project_id.clone());
                let chat = state
                    .db
                    .list_chats(&pc.project_id)
                    .ok()
                    .and_then(|cs| cs.into_iter().find(|c| c.id == pc.chat_id))
                    .map(|c| c.title)
                    .unwrap_or_else(|| "its chat".to_owned());
                let text = format!(
                    "Paired with Arbos → {project} · {chat}. Messages here now reach that chat."
                );
                send_text(state, &binding, &text).await;
                tracing::info!(platform, binding = %binding.id, "channel paired");
                return Outcome::Paired {
                    binding_id: binding.id,
                };
            }
            Ok(None) => {
                reply_unbound(
                    state,
                    &inb,
                    "That pairing code is invalid or expired. Mint a new one in Arbos → Settings → Channels.",
                )
                .await;
                return Outcome::PairFailed;
            }
            Err(e) => {
                tracing::warn!(error = %e, "pair code lookup");
                return Outcome::Ignored("pair lookup failed");
            }
        }
    }

    let binding =
        match state
            .db
            .channel_binding_for(platform, &inb.external_chat_id, &inb.external_user_id)
        {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "binding lookup");
                return Outcome::Ignored("binding lookup failed");
            }
        };
    let Some(binding) = binding else {
        if !inb.trusted_sender {
            return Outcome::Ignored("untrusted sender");
        }
        let first = state
            .db
            .mark_channel_hinted(platform, &inb.external_chat_id, &inb.external_user_id)
            .unwrap_or(false);
        if first {
            reply_unbound(state, &inb, HINT_TEXT).await;
            return Outcome::Hinted;
        }
        return Outcome::Ignored("unbound");
    };

    let payload = json!({
        "bindingId": binding.id,
        "platform": platform,
        "externalChatId": binding.external_chat_id,
        "externalUserId": binding.external_user_id,
        "chatId": binding.chat_id,
        "text": text,
        "senderName": inb.sender_name,
        "label": binding.label,
        "receivedAt": now_ms(),
    });
    if let Err(e) = state
        .db
        .enqueue_push(&binding.project_id, "channel_message", &payload)
    {
        tracing::warn!(error = %e, "enqueue channel_message");
        return Outcome::Ignored("enqueue failed");
    }
    boxes::ensure_awake(state, &binding.project_id, "channel").await;
    tracing::info!(platform, binding = %binding.id, project = %binding.project_id, "channel message queued");
    Outcome::Delivered {
        binding_id: binding.id,
        project_id: binding.project_id,
    }
}

/// `/pair 123456` → `Some("123456")`.
fn pair_code_in(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let head = words.next()?.trim_start_matches('/').to_ascii_lowercase();
    if head != "pair" {
        return None;
    }
    let code: String = words
        .next()?
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    (code.len() == 6).then_some(code)
}

async fn reply_unbound(state: &AppState, inb: &Inbound, text: &str) {
    let r = send(
        state,
        inb.platform,
        &inb.external_chat_id,
        text,
        None,
        &format!("arbos:hint:{}", ulid()),
    )
    .await;
    if let Err(e) = r {
        tracing::info!(platform = inb.platform.as_str(), error = %e, "hint reply not sent");
    }
}

async fn send_text(state: &AppState, b: &ChannelBinding, text: &str) {
    let Some(p) = Platform::parse(&b.platform) else {
        return;
    };
    let r = send(
        state,
        p,
        &b.external_chat_id,
        text,
        Some(&b.id),
        &format!("arbos:channel:{}", ulid()),
    )
    .await;
    if let Err(e) = r {
        tracing::warn!(platform = %b.platform, binding = %b.id, error = %e, "channel send failed");
    }
}

/// Deliver `text` to one external chat. Every attempt is recorded in `channel_outbox`
/// (`mock | sent | failed`); `request_id` is the idempotency key reused on retries.
pub async fn send(
    state: &AppState,
    p: Platform,
    external_chat_id: &str,
    text: &str,
    binding_id: Option<&str>,
    request_id: &str,
) -> anyhow::Result<Value> {
    let text = clip(text);
    if mock_on(state) {
        let id = state.db.record_channel_outbox(
            p.as_str(),
            external_chat_id,
            &text,
            request_id,
            "mock",
            None,
            binding_id,
        )?;
        return Ok(json!({ "mock": true, "outboxId": id }));
    }
    let row = state.db.record_channel_outbox(
        p.as_str(),
        external_chat_id,
        &text,
        request_id,
        "sending",
        None,
        binding_id,
    )?;
    let r = match p {
        Platform::Slack => slack::send(state, external_chat_id, &text).await,
        Platform::Telegram => telegram::send(state, external_chat_id, &text).await,
        Platform::Whatsapp => whatsapp::send(state, external_chat_id, &text).await,
        Platform::Beeper => beeper::send(state, external_chat_id, &text, request_id).await,
    };
    match &r {
        Ok(v) => {
            let detail = v.to_string().chars().take(300).collect::<String>();
            let _ = state
                .db
                .set_channel_outbox_status(row, "sent", Some(&detail));
        }
        Err(e) => {
            let _ = state
                .db
                .set_channel_outbox_status(row, "failed", Some(&e.to_string()));
        }
    }
    r
}

fn clip(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= MAX_REPLY_CHARS {
        return t.to_owned();
    }
    let mut cut: String = t.chars().take(MAX_REPLY_CHARS - 2).collect();
    cut.push_str(" …");
    cut
}

// ---- device-facing endpoints --------------------------------------------------------------

pub(crate) fn binding_dto(b: &ChannelBinding) -> ChannelBindingDto {
    ChannelBindingDto {
        id: b.id.clone(),
        platform: b.platform.clone(),
        external_chat_id: b.external_chat_id.clone(),
        external_user_id: b.external_user_id.clone(),
        project_id: b.project_id.clone(),
        chat_id: b.chat_id.clone(),
        label: b.label.clone(),
        bound_at: b.bound_at,
    }
}

fn binding_json(b: &ChannelBinding) -> Value {
    serde_json::to_value(binding_dto(b)).expect("ChannelBindingDto")
}

pub(crate) fn pair_dto(c: &ChannelPairCode) -> ChannelPairingDto {
    ChannelPairingDto {
        code: c.code.clone(),
        project_id: c.project_id.clone(),
        chat_id: c.chat_id.clone(),
        created_at: c.created_at,
        expires_at: c.expires_at,
    }
}

fn pair_json(c: &ChannelPairCode) -> Value {
    serde_json::to_value(pair_dto(c)).expect("ChannelPairingDto")
}

/// `GET /channels`: platform states, bindings, the active pairing code.
pub async fn overview(State(state): State<AppState>, principal: Principal) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    let platforms: Vec<ChannelPlatformDto> = Platform::ALL
        .iter()
        .map(|p| {
            let (st, detail) = platform_state(&state, *p);
            let inbound = match p {
                Platform::Slack => format!("POST {}/channels/slack/events", state.cfg.public_url),
                Platform::Telegram => format!(
                    "POST {}/channels/telegram/webhook/<secret> or ARBOS_TELEGRAM_POLL=1",
                    state.cfg.public_url
                ),
                Platform::Whatsapp => {
                    format!(
                        "GET/POST {}/channels/whatsapp/webhook",
                        state.cfg.public_url
                    )
                }
                Platform::Beeper => "relay polls POST /v1/messages/poll".to_owned(),
            };
            ChannelPlatformDto {
                platform: p.as_str().to_owned(),
                label: p.label().to_owned(),
                state: st.as_str().to_owned(),
                detail,
                inbound,
            }
        })
        .collect();
    let bindings: Vec<ChannelBindingDto> = state
        .db
        .list_channel_bindings()
        .map_err(internal)?
        .iter()
        .map(binding_dto)
        .collect();
    let pairing = state
        .db
        .active_channel_pair_code()
        .map_err(internal)?
        .map(|c| pair_dto(&c));
    Ok(Json(ChannelsOverviewDto {
        mock: mock_on(&state),
        platforms,
        bindings,
        pairing,
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct PairNewBody {
    #[serde(rename = "projectId")]
    pub project_id: String,
    /// Defaults to the project's pinned chat.
    #[serde(default, rename = "chatId")]
    pub chat_id: Option<String>,
}

/// `POST /channels/pair/new { projectId, chatId? }` → `{ code, expiresAt }` (10 min, one use).
pub async fn pair_new(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<PairNewBody>,
) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    let p = state
        .db
        .project(&b.project_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    let chats = state.db.list_chats(&p.id).map_err(internal)?;
    let chat_id = match b.chat_id {
        Some(c) => {
            if !chats.iter().any(|x| x.id == c) {
                return Err((StatusCode::NOT_FOUND, "no such chat".into()));
            }
            c
        }
        None => chats
            .iter()
            .find(|c| c.is_pinned)
            .or(chats.first())
            .map(|c| c.id.clone())
            .ok_or((StatusCode::CONFLICT, "project has no chats".to_owned()))?,
    };
    let now = now_ms();
    let code = ChannelPairCode {
        code: arbos_core::auth::pairing_code(),
        project_id: p.id,
        chat_id,
        created_at: now,
        expires_at: now + PAIR_CODE_TTL_MS,
    };
    state.db.new_channel_pair_code(&code).map_err(internal)?;
    Ok(Json(pair_json(&code)).into_response())
}

pub async fn list_bindings(State(state): State<AppState>, principal: Principal) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    let items: Vec<Value> = state
        .db
        .list_channel_bindings()
        .map_err(internal)?
        .iter()
        .map(binding_json)
        .collect();
    Ok(Json(json!({ "items": items })).into_response())
}

/// `DELETE /channels/bindings/{id}`: unbind one of the caller's own channel bindings. A binding
/// of another user's project answers 404, as `api::revoke_device` does for a foreign device, so
/// the route is not an enumeration oracle either (#1952).
pub async fn unbind(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    let missing = || (StatusCode::NOT_FOUND, "no such binding".to_owned());
    let binding = state
        .db
        .channel_binding(&id)
        .map_err(internal)?
        .ok_or_else(missing)?;
    if !state.allows(&principal, &binding.project_id) {
        return Err(missing());
    }
    let ok = state.db.delete_channel_binding(&id).map_err(internal)?;
    Ok(if ok {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
    .into_response())
}

#[derive(Deserialize)]
pub struct ReplyBody {
    #[serde(rename = "bindingId")]
    pub binding_id: String,
    pub text: String,
    /// Reused on retry so a platform never gets the same reply twice.
    #[serde(default, rename = "requestId")]
    pub request_id: Option<String>,
}

/// `POST /projects/{id}/channels/reply { bindingId, text }`: the box's turn finished; send the
/// prose back to the channel the message came from.
pub async fn reply(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ReplyBody>,
) -> ApiResult {
    let p = state
        .db
        .project(&id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such project".to_owned()))?;
    if !state.allows(&principal, &p.id) {
        return Err((StatusCode::FORBIDDEN, "not your project".into()));
    }
    let binding = state
        .db
        .channel_binding(&b.binding_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such binding".to_owned()))?;
    if binding.project_id != p.id {
        return Err((
            StatusCode::FORBIDDEN,
            "binding belongs to another project".into(),
        ));
    }
    let platform = Platform::parse(&binding.platform).ok_or((
        StatusCode::CONFLICT,
        format!("unknown platform {}", binding.platform),
    ))?;
    if b.text.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "text required".into()));
    }
    let request_id = b
        .request_id
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| format!("arbos:channel:{}", ulid()));
    let v = send(
        &state,
        platform,
        &binding.external_chat_id,
        &b.text,
        Some(&binding.id),
        &request_id,
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(json!({ "ok": true, "requestId": request_id, "result": v })).into_response())
}

// ---- mock -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MockInboundBody {
    pub platform: String,
    #[serde(rename = "externalChatId")]
    pub external_chat_id: String,
    #[serde(rename = "externalUserId")]
    pub external_user_id: String,
    pub text: String,
    #[serde(default, rename = "senderName")]
    pub sender_name: Option<String>,
}

/// `POST /channels/mock/inbound`: inject a message as if the platform delivered it (mock mode only).
pub async fn mock_inbound(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<MockInboundBody>,
) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    if !mock_on(&state) {
        return Err((
            StatusCode::CONFLICT,
            "set ARBOS_CHANNELS_MOCK=1 on the relay to use the mock inbound".into(),
        ));
    }
    let platform = Platform::parse(&b.platform).ok_or((
        StatusCode::BAD_REQUEST,
        "platform must be slack|telegram|whatsapp|beeper".to_owned(),
    ))?;
    let sender_name = b
        .sender_name
        .unwrap_or_else(|| format!("{} user", platform.label()));
    let outcome = handle_inbound(
        &state,
        Inbound {
            platform,
            external_chat_id: b.external_chat_id.clone(),
            external_user_id: b.external_user_id,
            sender_name: sender_name.clone(),
            text: b.text,
            event_key: None,
            trusted_sender: true,
            label: format!("{} · {sender_name} (mock)", platform.label()),
        },
    )
    .await;
    Ok(Json(json!({ "outcome": outcome_json(&outcome) })).into_response())
}

fn outcome_json(o: &Outcome) -> Value {
    match o {
        Outcome::Duplicate => json!({ "kind": "duplicate" }),
        Outcome::Ignored(why) => json!({ "kind": "ignored", "reason": why }),
        Outcome::Hinted => json!({ "kind": "hinted" }),
        Outcome::PairFailed => json!({ "kind": "pair_failed" }),
        Outcome::Paired { binding_id } => json!({ "kind": "paired", "bindingId": binding_id }),
        Outcome::Delivered {
            binding_id,
            project_id,
        } => json!({ "kind": "delivered", "bindingId": binding_id, "projectId": project_id }),
    }
}

/// `GET /channels/mock/outbox`: what was (or would have been) sent, newest first.
pub async fn mock_outbox(State(state): State<AppState>, principal: Principal) -> ApiResult {
    if principal.is_box() {
        return Err((StatusCode::FORBIDDEN, "device only".into()));
    }
    let items = state.db.channel_outbox(100).map_err(internal)?;
    Ok(Json(json!({ "mock": mock_on(&state), "items": items })).into_response())
}

/// Background pollers: Beeper (always, when Harness Comms is connected) and Telegram long polling.
pub async fn spawn_pollers(state: AppState) {
    beeper::spawn_poller(state.clone()).await;
    telegram::spawn_poller(state).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::crypto::RelayKey;
    use crate::db::{Chat, Db, Project};
    use crate::state::Config;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("arbos-channels-test-{}", ulid()));
        let db = Db::open(&dir.join("relay.db")).expect("db");
        let cfg = Config {
            public_url: "http://127.0.0.1:7707".into(),
            data_dir: dir,
            admin_token: None,
            openrouter_key: None,
            anthropic_key: None,
            openai_key: None,
            openai_base: String::new(),
            anthropic_base: String::new(),
            daytona_snapshot: String::new(),
            daytona_dockerfile: None,
            bootstrap_dir: None,
            agentd_bin: "arbos-agentd".into(),
            local_box_image: String::new(),
            box_idle_grace_ms: 0,
            default_provider: None,
            fleet: crate::fleet::FleetCfg::default(),
            connectors_mock: true,
            channels_mock: true,
            openai_images_base: String::new(),
            image_mock: false,
            avatar_retry_backoff_ms: Vec::new(),
            avatar_attempt_timeout: std::time::Duration::from_secs(40),
            avatar_total_timeout: std::time::Duration::from_secs(75),
            voice_mock: true,
            openai_audio_base: String::new(),
            vault_mock: false,
            auth_mode: crate::state::AuthMode::Paircode,
            github_client_id: None,
            github_client_secret: None,
            auth_redirects: Vec::new(),
            default_grant_usd: 100.0,
            default_daily_cap_usd: 20.0,
            global_daily_cap_usd: 500.0,
            global_machine_daily_cap_usd: 400.0,
            always_on: false,
            machine_usd_per_hour: 0.332,
            machine_stopped_usd_per_day: 0.013,
            releases_repo: "unarbos/arbos".into(),
            site_url: "https://arbos.life".into(),
            avatar_slugs: crate::avatar::DEFAULT_SLUGS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            avatar_colors: crate::avatar::DEFAULT_COLORS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            multi_chat: false,
            openai_direct_base: String::new(),
            openai_route: crate::state::OpenAiRoute::Auto,
        };
        AppState {
            db: db.clone(),
            cfg: Arc::new(cfg),
            http: reqwest::Client::new(),
            daytona: None,
            rooms: Arc::new(RwLock::new(HashMap::new())),
            box_states: Arc::new(RwLock::new(HashMap::new())),
            local_children: Arc::new(Mutex::new(HashMap::new())),
            box_conns: Arc::new(Mutex::new(HashMap::new())),
            wake_started: Arc::new(Mutex::new(HashMap::new())),
            last_client_push: Arc::new(Mutex::new(HashMap::new())),
            decisions_cache: Arc::new(std::sync::Mutex::new(Default::default())),
            decisions_inflight: Arc::new(Mutex::new(HashMap::new())),
            conn_seq: Arc::new(AtomicU64::new(0)),
            relay_key: Arc::new(RelayKey::derive("test-key")),
            jwt_key: Arc::new(RelayKey::derive("test-key").subkey("jwt")),
            download_cache: Arc::new(Mutex::new(None)),
            upgrading: Arc::new(Mutex::new(HashMap::new())),
            upgrade_refused: Arc::new(Mutex::new(HashMap::new())),
            stale_bounces: Arc::new(Mutex::new(HashMap::new())),
            local_conns: Arc::new(Mutex::new(HashMap::new())),
            refresh_grace: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            push: Arc::new(crate::push::Push::capture(db.clone())),
            focus: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            fallback_sent: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            livekit: None,
            presence_seen: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            activity: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            box_http_seen: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            fresh_preview_tokens: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            turn_running: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            evicted: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            fleet_pressure: Arc::new(tokio::sync::Mutex::new(None)),
            voice: Arc::new(crate::voice::VoiceCfg::default()),
            voice_conns: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            default_snapshot: Arc::new(std::sync::RwLock::new(String::new())),
            started_at: std::time::Instant::now(),
            restart: Arc::new(crate::state::RestartSignal::default()),
            revocations: Arc::new(tokio::sync::watch::Sender::new(0)),
            voice_dispatched: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            voice_worker: Arc::new(tokio::sync::Mutex::new(None)),
            voice_inbox: crate::voice::VoiceInbox::default(),
            image_health: Arc::new(tokio::sync::Mutex::new(None)),
            client_seen: Arc::new(std::sync::Mutex::new(HashMap::new())),
            matrix: crate::matrix::Matrix::new(crate::matrix::Config::off()),
        }
    }

    fn inbound(text: &str) -> Inbound {
        Inbound {
            platform: Platform::Telegram,
            external_chat_id: "chat-1".into(),
            external_user_id: "user-1".into(),
            sender_name: "Jacob".into(),
            text: text.into(),
            event_key: None,
            trusted_sender: true,
            label: "Telegram · Jacob".into(),
        }
    }

    /// Invariant (F7): an unbound sender never reaches a box. The first message gets exactly one
    /// hint reply, later ones are dropped silently, a bad `/pair` code binds nothing, and only a
    /// bound (user, chat) pair enqueues a `channel_message` push.
    #[tokio::test]
    async fn unbound_sender_never_reaches_a_box() {
        let state = test_state();
        let project = Project {
            id: "p1".into(),
            name: "Comms".into(),
            icon: "Co".into(),
            instructions: String::new(),
            models: json!({}),
            state: "awake".into(),
            created_at: 0,
            last_active_at: 0,
            spend_cap_daily: 25.0,
            color: "#000".into(),
            user_id: "u_default".into(),
            workspace_state: "stopped".into(),
            avatar: None,
            avatar_color: None,
            description: String::new(),
            primary_chat_id: None,
            avatar_color_index: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_key: None,
            avatar_version: 0,
        };
        state.db.insert_project(&project).unwrap();
        state
            .db
            .insert_chat(&Chat {
                id: "c1".into(),
                project_id: "p1".into(),
                title: "Comms".into(),
                is_pinned: true,
                position: 0,
                created_at: 0,
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();

        // 1. Unbound: one hint, no push.
        assert_eq!(
            handle_inbound(&state, inbound("hello?")).await,
            Outcome::Hinted
        );
        assert_eq!(
            handle_inbound(&state, inbound("anyone there?")).await,
            Outcome::Ignored("unbound")
        );
        assert!(state.db.take_pushes("p1").unwrap().is_empty());
        let outbox = state.db.channel_outbox(10).unwrap();
        assert_eq!(outbox.len(), 1, "exactly one hint reply");
        assert_eq!(outbox[0]["text"], HINT_TEXT);

        // 2. A wrong code binds nothing and enqueues nothing.
        assert_eq!(
            handle_inbound(&state, inbound("/pair 000000")).await,
            Outcome::PairFailed
        );
        assert!(state.db.list_channel_bindings().unwrap().is_empty());
        assert!(state.db.take_pushes("p1").unwrap().is_empty());

        // 3. Untrusted senders (Beeper contacts) get neither a hint nor a binding.
        let mut stranger = inbound("/pair 123456");
        stranger.platform = Platform::Beeper;
        stranger.trusted_sender = false;
        assert_eq!(
            handle_inbound(&state, stranger).await,
            Outcome::Ignored("untrusted sender")
        );
        assert_eq!(state.db.channel_outbox(10).unwrap().len(), 2);

        // 4. The real code pairs; the next message is the first thing a box ever sees.
        let code = ChannelPairCode {
            code: "123456".into(),
            project_id: "p1".into(),
            chat_id: "c1".into(),
            created_at: now_ms(),
            expires_at: now_ms() + 60_000,
        };
        state.db.new_channel_pair_code(&code).unwrap();
        let Outcome::Paired { binding_id } = handle_inbound(&state, inbound("/pair 123456")).await
        else {
            panic!("expected pairing");
        };
        assert!(state.db.take_pushes("p1").unwrap().is_empty());
        let out = handle_inbound(&state, inbound("what's on today?")).await;
        assert_eq!(
            out,
            Outcome::Delivered {
                binding_id: binding_id.clone(),
                project_id: "p1".into()
            }
        );
        let pushes = state.db.take_pushes("p1").unwrap();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].1, "channel_message");
        assert_eq!(pushes[0].2["bindingId"], binding_id);
        assert_eq!(pushes[0].2["text"], "what's on today?");

        // 5. The code is single-use.
        assert_eq!(
            handle_inbound(&state, inbound("/pair 123456")).await,
            Outcome::PairFailed
        );

        // 6. Another user in the same chat is still unbound.
        let mut other = inbound("me too");
        other.external_user_id = "user-2".into();
        assert_eq!(handle_inbound(&state, other).await, Outcome::Hinted);
        assert!(state.db.take_pushes("p1").unwrap().is_empty());
    }

    #[test]
    fn pair_code_parsing() {
        assert_eq!(pair_code_in("/pair 123456"), Some("123456".into()));
        assert_eq!(pair_code_in("PAIR 123-456"), Some("123456".into()));
        assert_eq!(pair_code_in("/pair 12345"), None);
        assert_eq!(pair_code_in("pairing is fun"), None);
    }

    /// `reply` must use `state.allows` (owner + invited members), not `can_access_project` with a
    /// project id. The agent box is `Principal::Box { user_id }`; comparing that user id to the
    /// project id always 403'd and channel replies never reached the platform.
    #[tokio::test]
    async fn reply_allows_owner_and_invited_member() {
        let state = test_state();
        let project = Project {
            id: "proj_a".into(),
            name: "Comms".into(),
            icon: "Co".into(),
            instructions: String::new(),
            models: json!({}),
            state: "awake".into(),
            created_at: 0,
            last_active_at: 0,
            spend_cap_daily: 25.0,
            color: "#000".into(),
            user_id: "user_a".into(),
            workspace_state: "stopped".into(),
            avatar: None,
            avatar_color: None,
            description: String::new(),
            primary_chat_id: None,
            avatar_color_index: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_key: None,
            avatar_version: 0,
        };
        state.db.insert_project(&project).unwrap();
        let binding = ChannelBinding {
            id: "bind_1".into(),
            platform: "telegram".into(),
            external_chat_id: "tg-chat".into(),
            external_user_id: "tg-user".into(),
            project_id: "proj_a".into(),
            chat_id: "c1".into(),
            label: "Telegram · test".into(),
            bound_at: 0,
        };
        state.db.upsert_channel_binding(&binding).unwrap();

        let body = |text: &str| {
            Json(ReplyBody {
                binding_id: "bind_1".into(),
                text: text.into(),
                request_id: None,
            })
        };
        let project_path = || Path("proj_a".to_owned());

        let box_a = Principal::Box {
            user_id: "user_a".into(),
        };
        let r = reply(
            State(state.clone()),
            box_a,
            project_path(),
            body("from box A"),
        )
        .await
        .expect("owner box");
        assert_eq!(r.status(), StatusCode::OK);
        let out = state.db.channel_outbox(10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["text"], "from box A");

        let device_a = Principal::Device {
            id: "dev_a".into(),
            token: "tok_a".into(),
            user_id: "user_a".into(),
        };
        let r = reply(
            State(state.clone()),
            device_a,
            project_path(),
            body("from device A"),
        )
        .await
        .expect("owner device");
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(state.db.channel_outbox(10).unwrap().len(), 2);

        let before = state.db.channel_outbox(10).unwrap().len();
        let box_b = Principal::Box {
            user_id: "user_b".into(),
        };
        let err = reply(
            State(state.clone()),
            box_b,
            project_path(),
            body("from box B"),
        )
        .await
        .expect_err("other box");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(state.db.channel_outbox(10).unwrap().len(), before);

        let device_b = Principal::Device {
            id: "dev_b".into(),
            token: "tok_b".into(),
            user_id: "user_b".into(),
        };
        let err = reply(
            State(state.clone()),
            device_b.clone(),
            project_path(),
            body("from device B"),
        )
        .await
        .expect_err("other device");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(state.db.channel_outbox(10).unwrap().len(), before);

        state
            .db
            .add_project_member("proj_a", "user_b", "user_a")
            .unwrap();
        let r = reply(
            State(state.clone()),
            device_b,
            project_path(),
            body("from invited B"),
        )
        .await
        .expect("invited member");
        assert_eq!(r.status(), StatusCode::OK);
        let out = state.db.channel_outbox(10).unwrap();
        assert_eq!(out.len(), before + 1);
        assert_eq!(out[0]["text"], "from invited B");
    }

    fn status(r: ApiResult) -> StatusCode {
        match r {
            Ok(resp) => resp.status(),
            Err((s, _)) => s,
        }
    }

    /// #1952: `unbind` refused boxes and nothing else, so any signed-in device could unbind
    /// another user's channel. A binding outside the caller's projects answers 404, like
    /// `api::revoke_device` does for a foreign device.
    #[tokio::test]
    async fn unbind_refuses_a_binding_of_another_users_project() {
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let (alice, alice_dev) = crate::people::tests_support::person(&state, 1);
        let (_bob, bob_dev) = crate::people::tests_support::person(&state, 2);
        let project = crate::people::tests_support::project_of(&state, &alice, "Comms");
        let binding = ChannelBinding {
            id: "bind_1".into(),
            platform: "telegram".into(),
            external_chat_id: "tg-chat".into(),
            external_user_id: "tg-user".into(),
            project_id: project,
            chat_id: "c1".into(),
            label: "Telegram · test".into(),
            bound_at: 0,
        };
        state.db.upsert_channel_binding(&binding).unwrap();

        let foreign = unbind(State(state.clone()), bob_dev, Path("bind_1".to_owned())).await;
        assert_eq!(status(foreign), StatusCode::NOT_FOUND);
        assert!(
            state.db.channel_binding("bind_1").unwrap().is_some(),
            "another user's binding survives"
        );

        // An id that exists and one that does not answer alike: no enumeration oracle.
        let unknown = unbind(
            State(state.clone()),
            alice_dev.clone(),
            Path("no_such_binding".to_owned()),
        )
        .await;
        assert_eq!(status(unknown), StatusCode::NOT_FOUND);

        let own = unbind(State(state.clone()), alice_dev, Path("bind_1".to_owned())).await;
        assert_eq!(status(own), StatusCode::NO_CONTENT);
        assert!(state.db.channel_binding("bind_1").unwrap().is_none());
    }
}
