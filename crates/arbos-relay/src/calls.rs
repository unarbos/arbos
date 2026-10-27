//! Calls v1: voice/video between people, 1:1 and group, on a self-hosted LiveKit next to the
//! relay. The relay owns the call's life (who may join, who is in, when it ends) and mints
//! LiveKit join tokens; media never touches it.
//!
//! Endpoints (bearer = device access token): `POST /chats/{id}/calls` and
//! `POST /projects/{p}/chats/{c}/calls` (start, or join the live one), `GET /chats/{id}/call`
//! (+ projects form), `GET /calls/{id}`, `POST /calls/{id}/join`, `POST /calls/{id}/leave`,
//! `POST /calls/{id}/speakers`, `DELETE /calls/{id}/speakers/{user_id}`,
//! `POST/DELETE /devices/{id}/voip`, `POST /livekit/webhook`, `GET /admin/calls`.
//!
//! Bigger group calls use hosted speak: the starter is the host speaker; they invite others
//! with `POST …/speakers`. Everyone else joins listen-only (`canPublish: false`).
//!
//! Signalling: `call_started` / `call_ended` on `/me/pushes`, an APNs alert with category
//! `ARBOS_CALL`, and a PushKit VoIP push on start. System lines in the chat are written by the
//! clients (the starter on 201; the one client whose leave answered `ended: true`, or the device
//! named in `call_ended.writer_device`).

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{
    CallAlertDto, CallDto, CallEndedPayloadDto, CallEnvelopeDto, CallMissedPayloadDto,
    CallReachabilityDto, CallRingPayloadDto, CallStartedPayloadDto, LeaveCallResponseDto,
    PushEventDto, ReachableDeviceDto, SenderDto, StartCallResponseDto, VoipRegisterResponseDto,
};
use arbos_core::ids::{now_ms, ulid};

use crate::auth::Principal;
use crate::db_calls::{CallRow, VoipToken};
use crate::notify::{self, ChatRef, Sender};
use crate::people;
use crate::push::{Outgoing, SendOutcome};
use crate::signin::api_error;
use crate::state::AppState;

pub const CATEGORY: &str = "ARBOS_CALL";
/// Nobody joined (not even the starter's LiveKit connection) within this: the call is abandoned.
const EMPTY_START_MS: i64 = 90_000;
/// Everyone gone for this long without a `room_finished`: ended as abandoned.
const EMPTY_AFTER_MS: i64 = 60_000;
/// Hard cap on a call row that stays live with no activity at all (lost webhooks and leaves).
const STALE_MS: i64 = 8 * 3_600_000;

/// A callee device counts as "Arbos is open" when it talked to the relay this recently.
const LIVE_WINDOW_MS: i64 = 30_000;

/// What the starting client said it is: `X-Arbos-Client: ios/1.2 (202610161326)` →
/// `("ios", "1.2 (202610161326)")`; falls back to the device's push registration.
fn client_identity(
    state: &AppState,
    headers: &HeaderMap,
    device_id: &str,
) -> (Option<String>, Option<String>) {
    if let Some(h) = headers
        .get("x-arbos-client")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let (platform, version) = h.split_once('/').unwrap_or((h, ""));
        return (
            Some(platform.trim().to_ascii_lowercase()),
            Some(version.trim().to_owned()).filter(|v| !v.is_empty()),
        );
    }
    state
        .db
        .push_token(device_id)
        .ok()
        .flatten()
        .map(|t| {
            (
                Some(t.platform),
                Some(t.app_version).filter(|v| !v.is_empty()),
            )
        })
        .unwrap_or((None, None))
}

/// `reachable` in the start response: can each callee hear this call right now?
async fn reachability(state: &AppState, callees: &[String]) -> Vec<CallReachabilityDto> {
    let now = now_ms();
    let presence = state.presence_seen.lock().await.clone();
    callees
        .iter()
        .map(|u| {
            let devices: Vec<ReachableDeviceDto> = state
                .db
                .device_reach_rows(u)
                .unwrap_or_default()
                .into_iter()
                .map(|(id, name, kind, seen, platform, app_version, voip)| {
                    let seen = seen.into_iter().chain(presence.get(&id).copied()).max();
                    let has_push = platform.as_ref().is_some_and(|p| !p.is_empty());
                    ReachableDeviceDto {
                        device_id: id,
                        name,
                        kind,
                        platform,
                        app_version,
                        polled_within_30s: seen.is_some_and(|t| now - t < LIVE_WINDOW_MS),
                        last_seen_at: seen,
                        voip_token: voip,
                        push_token: has_push,
                    }
                })
                .collect();
            let card = Sender::user(state, u);
            CallReachabilityDto {
                user_id: u.clone(),
                handle: card.handle,
                name: card.name,
                any_live: devices.iter().any(|d| d.polled_within_30s),
                any_voip: devices.iter().any(|d| d.voip_token),
                // Regular APNs can still pop an Incoming-call banner (and wake
                // a backgrounded iPhone via content-available) when PushKit is
                // missing — do not tell the caller "they'll see a missed call".
                any_push: devices.iter().any(|d| d.push_token),
                devices,
            }
        })
        .collect()
}

fn identity_of(user_id: &str) -> String {
    format!("u_{user_id}")
}

fn user_of_identity(identity: &str) -> Option<&str> {
    identity.strip_prefix("u_")
}

/// Live participants as `sender{}` objects: humans first, then agents in the call (Phase 2).
fn participants_dto(state: &AppState, call: &CallRow) -> Vec<SenderDto> {
    let mut out: Vec<SenderDto> = state
        .db
        .call_participants(&call.id)
        .unwrap_or_default()
        .iter()
        .filter(|p| p.left_at.is_none())
        .map(|p| Sender::user(state, &p.user_id).dto())
        .collect();
    out.extend(
        crate::voice::agent_participants_json(state, &call.id)
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok()),
    );
    out
}

/// The call object without a token (worker and admin views).
pub fn call_json_public(state: &AppState, call: &CallRow) -> Value {
    serde_json::to_value(call_dto(state, call, None, None)).expect("CallDto")
}

/// Bigger group calls use hosted speak: only the host and invited speakers may publish.
/// 1:1 and tiny chats stay open-mic (`speak_mode: "open"`).
fn speak_hosted(state: &AppState, call: &CallRow) -> bool {
    let chat = chat_ref_of(call);
    let humans = members(state, &chat);
    if humans.len() >= 3 {
        return true;
    }
    if call.project_id.is_none() {
        if let Ok(Some(c)) = state.db.people_chat(&call.chat_id) {
            return c.kind == "group";
        }
    }
    false
}

fn invited_speaker_ids(state: &AppState, call_id: &str) -> Vec<String> {
    state.db.call_speakers(call_id).unwrap_or_default()
}

fn user_can_speak(state: &AppState, call: &CallRow, user_id: &str) -> bool {
    if !speak_hosted(state, call) {
        return true;
    }
    if user_id == call.started_by {
        return true;
    }
    invited_speaker_ids(state, &call.id)
        .iter()
        .any(|u| u == user_id)
}

fn invited_speakers_dto(state: &AppState, call: &CallRow) -> Vec<SenderDto> {
    invited_speaker_ids(state, &call.id)
        .iter()
        .map(|u| Sender::user(state, u).dto())
        .collect()
}

/// The call object; with a fresh token for `for_user` when the call is live.
/// `platform` (from `X-Arbos-Client`) is written into LiveKit participant metadata so the
/// voice agent can say where each person is calling from.
pub(crate) fn call_dto(
    state: &AppState,
    call: &CallRow,
    for_user: Option<&str>,
    platform: Option<&str>,
) -> CallDto {
    let hosted = speak_hosted(state, call);
    let mut dto = CallDto {
        call_id: call.id.clone(),
        chat_id: call.chat_id.clone(),
        project_id: call
            .project_id
            .clone()
            .unwrap_or_else(|| people::PEOPLE_PROJECT.to_owned()),
        kind: call.kind.clone(),
        room: call.room.clone(),
        livekit_url: state.livekit.as_ref().map(|l| l.url.clone()),
        max_participants: i64::from(crate::livekit::MAX_PARTICIPANTS),
        started_by: Sender::user(state, &call.started_by).dto(),
        participants: participants_dto(state, call),
        agents: crate::voice::agents_in_chat(state, &chat_ref_of(call))
            .iter()
            .map(|p| Sender::agent(state, p).dto())
            .collect(),
        started_at: call.started_at,
        ended_at: call.ended_at,
        reason: call.end_reason.clone(),
        started_device: call.started_device.clone(),
        platform: call.platform.clone(),
        app_version: call.app_version.clone(),
        token: None,
        token_expires_at: None,
        speak_mode: Some(if hosted { "hosted" } else { "open" }.into()),
        invited_speakers: if hosted {
            invited_speakers_dto(state, call)
        } else {
            Vec::new()
        },
        can_speak: None,
    };
    if let (Some(lk), Some(user), true) = (&state.livekit, for_user, call.active()) {
        let me = Sender::user(state, user);
        let mut meta = serde_json::to_value(me.dto()).expect("SenderDto");
        let plat = platform
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .or_else(|| {
                call.platform
                    .as_deref()
                    .filter(|_| user == call.started_by.as_str())
            });
        if let Some(p) = plat {
            meta["platform"] = json!(p);
        }
        let can_speak = user_can_speak(state, call, user);
        dto.can_speak = Some(can_speak);
        let (token, exp) =
            lk.join_token(&call.room, &identity_of(user), &me.name, &meta, can_speak);
        dto.token = Some(token);
        dto.token_expires_at = Some(exp);
    }
    dto
}

fn call_json(
    state: &AppState,
    call: &CallRow,
    for_user: Option<&str>,
    platform: Option<&str>,
) -> Value {
    serde_json::to_value(call_dto(state, call, for_user, platform)).expect("CallDto")
}

fn to_json<T: serde::Serialize>(dto: T) -> Value {
    serde_json::to_value(dto).expect("call dto")
}

fn chat_ref_of(call: &CallRow) -> ChatRef {
    match &call.project_id {
        Some(p) => ChatRef::project(p, &call.chat_id),
        None => ChatRef::people(&call.chat_id),
    }
}

/// Members of the chat the call lives in (humans only).
fn members(state: &AppState, chat: &ChatRef) -> Vec<String> {
    notify::recipients(state, chat)
}

fn is_member(state: &AppState, chat: &ChatRef, user_id: &str) -> bool {
    members(state, chat).iter().any(|u| u == user_id)
}

fn calls_off() -> Response {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "calls_off",
        "this relay has no LiveKit (LIVEKIT_URL / LIVEKIT_API_KEY / LIVEKIT_API_SECRET)",
    )
}

fn device_user(principal: &Principal) -> Result<(String, String), Box<Response>> {
    match principal {
        Principal::Device { id, user_id, .. } => Ok((id.clone(), user_id.clone())),
        Principal::Box { .. } | Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        ))),
    }
}

// ---- signalling -----------------------------------------------------------------------------------

async fn deliver_voip(state: &AppState, t: &VoipToken, payload: Value) {
    let out = Outgoing {
        device_token: t.token.clone(),
        environment: t.environment.clone(),
        push_type: "voip",
        topic: Some(format!("{}.voip", t.bundle_id)),
        collapse_id: None,
        payload,
    };
    match state.push.send_now(out).await {
        SendOutcome::Accepted => {}
        SendOutcome::TokenInvalid { .. } => {
            let _ = state.db.delete_voip_token_value(&t.token);
        }
        outcome @ (SendOutcome::Permanent { .. } | SendOutcome::Transient { .. }) => {
            tracing::warn!(device = %t.device_id, error = %outcome, "VoIP push failed")
        }
    }
}

async fn deliver_alert(
    state: &AppState,
    t: &crate::db_push::PushToken,
    payload: Value,
    collapse: String,
) {
    let out = Outgoing {
        device_token: t.token.clone(),
        environment: t.environment.clone(),
        push_type: "alert",
        topic: None,
        collapse_id: Some(collapse),
        payload,
    };
    match state.push.send_now(out).await {
        SendOutcome::Accepted => {}
        SendOutcome::TokenInvalid { .. } => {
            let _ = state.db.delete_push_token_value(&t.token);
        }
        outcome @ (SendOutcome::Permanent { .. } | SendOutcome::Transient { .. }) => {
            tracing::warn!(device = %t.device_id, error = %outcome, "call alert failed")
        }
    }
}

/// A call began: `call_started` on `/me/pushes`, a VoIP push (CallKit) and an APNs alert to
/// each callee. `callees` is who to ring (already without the starter); empty means nobody
/// is notified (a selective start that named only agents).
async fn announce_started(state: &AppState, call: &CallRow, starter: &Sender, callees: &[String]) {
    let call_d = call_dto(state, call, None, None);
    let starter_d = starter.dto();
    let started = CallStartedPayloadDto {
        call: call_d.clone(),
        sender: starter_d.clone(),
    };
    let ring = CallRingPayloadDto {
        call_id: call.id.clone(),
        chat_id: call.chat_id.clone(),
        project_id: call_d.project_id.clone(),
        kind: call.kind.clone(),
        room: call.room.clone(),
        livekit_url: call_d.livekit_url.clone(),
        caller: starter_d.clone(),
        started_at: call.started_at,
    };
    let alert = CallAlertDto {
        event: "started".into(),
        chat_id: call.chat_id.clone(),
        project_id: call_d.project_id.clone(),
        call: call_d,
        sender: Some(starter_d),
    };
    let title = starter.title();
    let what = if call.kind == "video" {
        "video call"
    } else {
        "voice call"
    };
    for user in callees {
        let _ = state
            .db
            .push_user_event(user, &PushEventDto::CallStarted(started.clone()), None);
        if !state.push.is_on() {
            continue;
        }
        for t in state.db.voip_tokens_of_user(user).unwrap_or_default() {
            deliver_voip(state, &t, to_json(&ring)).await;
        }
        for t in state.db.push_tokens_of_user(user).unwrap_or_default() {
            // content-available wakes a backgrounded (not force-quit) iPhone so
            // it can report CallKit — WhatsApp-style ring without the app open.
            let mut payload = to_json(&alert);
            payload["aps"] = json!({
                "alert": { "title": title, "body": format!("Incoming {what}") },
                "sound": "default",
                "thread-id": call.chat_id,
                "category": CATEGORY,
                "mutable-content": 1,
                "content-available": 1,
                "interruption-level": "time-sensitive",
            });
            deliver_alert(state, &t, payload, format!("call:{}", call.id)).await;
        }
    }
}

/// A call is over: `call_ended` to every member (one device is asked to write the system line)
/// and an APNs alert that replaces the ringing banner.
async fn announce_ended(
    state: &AppState,
    call: &CallRow,
    reason: &str,
    writer_device: Option<&str>,
) {
    state.voice_inbox.close_call(&call.id).await;
    let chat = chat_ref_of(call);
    let ended_at = call.ended_at.unwrap_or_else(now_ms);
    let duration_s = ((ended_at - call.started_at) / 1000).max(0);
    let project_label = call
        .project_id
        .clone()
        .unwrap_or_else(|| people::PEOPLE_PROJECT.to_owned());
    let ended = CallEndedPayloadDto {
        call_id: call.id.clone(),
        chat_id: call.chat_id.clone(),
        project_id: project_label.clone(),
        duration_s,
        reason: reason.to_owned(),
        writer_device: writer_device.map(str::to_owned),
    };
    let alert = CallAlertDto {
        event: "ended".into(),
        chat_id: call.chat_id.clone(),
        project_id: project_label,
        call: call_dto(state, call, None, None),
        sender: None,
    };
    for user in members(state, &chat) {
        let _ = state
            .db
            .push_user_event(&user, &PushEventDto::CallEnded(ended.clone()), None);
        if !state.push.is_on() {
            continue;
        }
        for t in state.db.push_tokens_of_user(&user).unwrap_or_default() {
            let mut payload = to_json(&alert);
            payload["aps"] = json!({
                "alert": { "title": "Call ended", "body": format!("{} min", (duration_s + 30) / 60) },
                "thread-id": call.chat_id,
                "category": CATEGORY,
                "content-available": 1,
            });
            deliver_alert(state, &t, payload, format!("call:{}", call.id)).await;
        }
    }
}

/// Members who never joined a call that is now over: `call_missed` on `/me/pushes` (their next
/// app open shows "Missed call from <name>" and writes the system line) and an APNs alert.
async fn announce_missed(state: &AppState, call: &CallRow) {
    let chat = chat_ref_of(call);
    let joined: Vec<String> = state
        .db
        .call_participants(&call.id)
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.user_id)
        .collect();
    let from = Sender::user(state, &call.started_by);
    let from_d = from.dto();
    let ended_at = call.ended_at.unwrap_or_else(now_ms);
    let project_label = call
        .project_id
        .clone()
        .unwrap_or_else(|| people::PEOPLE_PROJECT.to_owned());
    let call_d = call_dto(state, call, None, None);
    let missed = CallMissedPayloadDto {
        call_id: call.id.clone(),
        chat_id: call.chat_id.clone(),
        project_id: project_label.clone(),
        kind: call.kind.clone(),
        from: from_d.clone(),
        started_at: call.started_at,
        ended_at,
        duration_s: ((ended_at - call.started_at) / 1000).max(0),
        reason: call.end_reason.clone(),
    };
    let alert = CallAlertDto {
        event: "missed".into(),
        chat_id: call.chat_id.clone(),
        project_id: project_label,
        call: call_d,
        sender: Some(from_d),
    };
    let what = if call.kind == "video" {
        "video"
    } else {
        "voice"
    };
    for user in members(state, &chat) {
        if user == call.started_by || joined.contains(&user) {
            continue;
        }
        let _ = state
            .db
            .push_user_event(&user, &PushEventDto::CallMissed(missed.clone()), None);
        if !state.push.is_on() {
            continue;
        }
        for t in state.db.push_tokens_of_user(&user).unwrap_or_default() {
            let mut payload = to_json(&alert);
            payload["aps"] = json!({
                "alert": { "title": from.title(), "body": format!("Missed {what} call") },
                "sound": "default",
                "thread-id": call.chat_id,
                "category": CATEGORY,
                "mutable-content": 1,
            });
            deliver_alert(state, &t, payload, format!("call:{}", call.id)).await;
        }
    }
}

/// End a call for `reason`; announces once, closes the LiveKit room. `writer_device` is the
/// device that should write the "Call ended" line when no client already knows it ended.
pub async fn end_call(
    state: &AppState,
    call_id: &str,
    reason: &str,
    writer_device: Option<String>,
) {
    if !state.db.end_call(call_id, reason).unwrap_or(false) {
        return;
    }
    let Ok(Some(call)) = state.db.call(call_id) else {
        return;
    };
    tracing::info!(call = %call.id, room = %call.room, reason, "call ended");
    let writer = match writer_device {
        Some(d) => Some(d),
        None => state.db.freshest_device_of(&call.started_by).ok().flatten(),
    };
    announce_ended(state, &call, reason, writer.as_deref()).await;
    announce_missed(state, &call).await;
    // Drop any voice turn still running on the box (typed chat after hang-up).
    // Prefer agents still marked live on the call; also notify every agent that belongs
    // to the chat — the voice worker may already have left (`call_agent_leave`) before
    // `end_call`, which used to leave `call_agents_live` empty and skip cancel
    // (feedback app-01M5CT92B0NPK1X09YW5RHNRN8).
    let mut cancel_pids = state.db.call_agents_live(&call.id).unwrap_or_default();
    for p in crate::voice::agents_in_chat(state, &chat_ref_of(&call)) {
        if !cancel_pids.iter().any(|id| id == &p.id) {
            cancel_pids.push(p.id.clone());
        }
    }
    for pid in cancel_pids {
        if let Ok(Some(p)) = state.db.project(&pid) {
            let _ = crate::voice::box_notify(
                state,
                &p.user_id,
                serde_json::json!({
                    "type": "voice_cancel",
                    "call_id": call.id,
                    "chat_id": call.chat_id,
                    "project_id": p.id,
                }),
            )
            .await;
        }
    }
    if let Some(lk) = &state.livekit {
        lk.delete_room(&state.http, &call.room).await;
    }
}

// ---- handlers -------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct StartBody {
    #[serde(default)]
    pub kind: Option<String>,
    /// When set and non-empty: only these chat members are rung / dispatched.
    /// Human entries are bare user ids; agents are project ids (same as `ChatMember.id`).
    /// Omitted or empty = the whole group (current behaviour).
    #[serde(default)]
    pub ring: Option<Vec<String>>,
}

async fn start_or_join(
    state: &AppState,
    principal: &Principal,
    headers: &HeaderMap,
    chat: ChatRef,
    body: StartBody,
) -> Response {
    if state.livekit.is_none() {
        return calls_off();
    }
    let (device, me) = match device_user(principal) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !is_member(state, &chat, &me) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not in this chat");
    }
    let kind = match body.kind.as_deref().unwrap_or("voice") {
        "voice" => "voice",
        "video" => "video",
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "kind must be voice or video",
            )
        }
    };
    let (platform, app_version) = client_identity(state, headers, &device);
    if let Ok(Some(existing)) = state.db.active_call_in_chat(&chat.chat_id) {
        let _ = state.db.call_join(&existing.id, &me);
        crate::voice::ensure_agents(state, &existing).await;
        return Json(to_json(StartCallResponseDto {
            call: call_dto(state, &existing, Some(&me), platform.as_deref()),
            joined: true,
            reachable: None,
            agents_dispatched: None,
        }))
        .into_response();
    }
    let now = now_ms();
    let call = CallRow {
        id: ulid(),
        chat_id: chat.chat_id.clone(),
        project_id: chat.project_id.clone(),
        kind: kind.to_owned(),
        room: format!("call_{}", ulid()),
        started_by: me.clone(),
        started_at: now,
        ended_at: None,
        end_reason: None,
        last_activity_at: now,
        started_device: Some(device),
        platform: platform.clone(),
        app_version,
    };
    if let Err(e) = state.db.insert_call(&call) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    let _ = state.db.call_join(&call.id, &me);
    tracing::info!(call = %call.id, chat = %call.chat_id, kind, by = %me, platform = ?call.platform, app_version = ?call.app_version, "call started");
    let starter = Sender::user(state, &me);
    // Reachability before announce: a failed VoIP push may delete the token
    // (TokenGone), and the caller must still see "Ringing …" for this attempt.
    let all_humans: Vec<String> = members(state, &chat)
        .into_iter()
        .filter(|u| *u != me)
        .collect();
    let ring_set: Option<std::collections::HashSet<&str>> = body
        .ring
        .as_ref()
        .filter(|ids| !ids.is_empty())
        .map(|ids| ids.iter().map(String::as_str).collect());
    let callees: Vec<String> = match &ring_set {
        Some(want) => all_humans
            .into_iter()
            .filter(|u| want.contains(u.as_str()))
            .collect(),
        None => all_humans,
    };
    let reachable = reachability(state, &callees).await;
    announce_started(state, &call, &starter, &callees).await;
    // Phase 2: the chat's project agents join through the voice worker.
    // A selective start only dispatches agents named in `ring`.
    let dispatched = crate::voice::dispatch_agents(state, &call, ring_set.as_ref()).await;
    (
        StatusCode::CREATED,
        Json(to_json(StartCallResponseDto {
            call: call_dto(state, &call, Some(&me), platform.as_deref()),
            joined: false,
            reachable: Some(reachable),
            agents_dispatched: Some(Value::Array(dispatched)),
        })),
    )
        .into_response()
}

/// `POST /chats/{id}/calls {"kind": "voice"|"video"}` — 201 new call, 200 joined the live one.
pub async fn start_people_call(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<StartBody>>,
) -> Response {
    if !state.allows(&principal, &format!("{}{id}", people::PEOPLE_ACL_PREFIX)) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not in this chat");
    }
    start_or_join(
        &state,
        &principal,
        &headers,
        ChatRef::people(&id),
        body.map(|b| b.0).unwrap_or_default(),
    )
    .await
}

/// `POST /projects/{project_id}/chats/{chat_id}/calls` — same for a shared project chat.
pub async fn start_project_call(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<StartBody>>,
) -> Response {
    let Some(project) = resolve_project_chat(&state, &principal, &project_id, &chat_id) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
    };
    start_or_join(
        &state,
        &principal,
        &headers,
        ChatRef::project(&project, &chat_id),
        body.map(|b| b.0).unwrap_or_default(),
    )
    .await
}

/// The project's canonical id when the principal may use it and the chat exists.
fn resolve_project_chat(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
    chat_id: &str,
) -> Option<String> {
    if !state.allows(principal, project_id) {
        return None;
    }
    let project = state.db.project(project_id).ok().flatten()?;
    state
        .db
        .chat_exists(&project.id, chat_id)
        .unwrap_or(false)
        .then_some(project.id)
}

fn active_call_response(state: &AppState, chat_id: &str, me: &str) -> Response {
    match state.db.active_call_in_chat(chat_id) {
        Ok(Some(call)) => Json(to_json(CallEnvelopeDto {
            call: call_dto(state, &call, Some(me), None),
        }))
        .into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "no_call", "no call in this chat"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `GET /chats/{id}/call` → the live call (with a join token) or 404.
pub async fn people_chat_call(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if !state.allows(&principal, &format!("{}{id}", people::PEOPLE_ACL_PREFIX)) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not in this chat");
    }
    let (_, me) = match device_user(&principal) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    active_call_response(&state, &id, &me)
}

/// `GET /projects/{project_id}/chats/{chat_id}/call`
pub async fn project_chat_call(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    if resolve_project_chat(&state, &principal, &project_id, &chat_id).is_none() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
    }
    let (_, me) = match device_user(&principal) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    active_call_response(&state, &chat_id, &me)
}

/// The call when the caller is a member of its chat.
fn member_call(
    state: &AppState,
    principal: &Principal,
    call_id: &str,
) -> Result<(String, String, CallRow), Box<Response>> {
    let (device, me) = device_user(principal)?;
    let call = state
        .db
        .call(call_id)
        .map_err(|e| {
            Box::new(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db",
                e.to_string(),
            ))
        })?
        .ok_or_else(|| {
            Box::new(api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no such call",
            ))
        })?;
    if !is_member(state, &chat_ref_of(&call), &me) {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "not_member",
            "not in this call's chat",
        )));
    }
    Ok((device, me, call))
}

/// `GET /calls/{id}` → `{call}`; ended calls answer too (no token then).
pub async fn get_call(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (device, me, call) = match member_call(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let (platform, _) = client_identity(&state, &headers, &device);
    Json(to_json(CallEnvelopeDto {
        call: call_dto(&state, &call, Some(&me), platform.as_deref()),
    }))
    .into_response()
}

/// `POST /calls/{id}/join` → `{call}` with a fresh token; 410 when the call ended.
pub async fn join_call(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if state.livekit.is_none() {
        return calls_off();
    }
    let (device, me, call) = match member_call(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !call.active() {
        return api_error(StatusCode::GONE, "call_ended", "this call has ended");
    }
    let live = state
        .db
        .call_participants(&call.id)
        .unwrap_or_default()
        .iter()
        .filter(|p| p.left_at.is_none() && p.user_id != me)
        .count() as u32;
    if live >= crate::livekit::MAX_PARTICIPANTS {
        return api_error(StatusCode::CONFLICT, "call_full", "this call is full");
    }
    let _ = state.db.call_join(&call.id, &me);
    crate::voice::ensure_agents(&state, &call).await;
    let (platform, _) = client_identity(&state, &headers, &device);
    Json(to_json(CallEnvelopeDto {
        call: call_dto(&state, &call, Some(&me), platform.as_deref()),
    }))
    .into_response()
}

/// `POST /calls/{id}/leave` → `{ended, duration_s}`; the last one out ends the call.
pub async fn leave_call(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let (device, me, call) = match member_call(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !call.active() {
        let duration_s = (call.ended_at.unwrap_or(call.started_at) - call.started_at) / 1000;
        return Json(to_json(LeaveCallResponseDto {
            ended: false,
            duration_s,
            already_ended: Some(true),
        }))
        .into_response();
    }
    let remaining = state.db.call_leave(&call.id, &me).unwrap_or(1);
    if remaining > 0 {
        return Json(to_json(LeaveCallResponseDto {
            ended: false,
            duration_s: (now_ms() - call.started_at) / 1000,
            already_ended: None,
        }))
        .into_response();
    }
    // This client knows the call ended (it gets `ended: true`), so nobody else writes the line.
    end_call(&state, &call.id, "last_left", Some(device)).await;
    let duration_s = state
        .db
        .call(&call.id)
        .ok()
        .flatten()
        .and_then(|c| c.ended_at)
        .map(|e| (e - call.started_at) / 1000)
        .unwrap_or(0);
    Json(to_json(LeaveCallResponseDto {
        ended: true,
        duration_s,
        already_ended: None,
    }))
    .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct SpeakerBody {
    pub user_id: Option<String>,
}

/// `POST /calls/{id}/speakers {"user_id"}` — host invites someone to speak on a hosted group call.
pub async fn invite_speaker(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<SpeakerBody>>,
) -> Response {
    let (device, me, call) = match member_call(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !call.active() {
        return api_error(StatusCode::GONE, "call_ended", "this call has ended");
    }
    if me != call.started_by {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_host",
            "only the host can invite speakers",
        );
    }
    if !speak_hosted(&state, &call) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "open_mic",
            "this call lets everyone speak",
        );
    }
    let Some(user_id) = body
        .and_then(|b| b.0.user_id)
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
    else {
        return api_error(StatusCode::BAD_REQUEST, "bad_request", "user_id required");
    };
    if user_id == call.started_by {
        return api_error(
            StatusCode::BAD_REQUEST,
            "already_host",
            "the host already speaks",
        );
    }
    if !is_member(&state, &chat_ref_of(&call), &user_id) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "not_member",
            "that person is not in this chat",
        );
    }
    let _ = state.db.add_call_speaker(&call.id, &user_id);
    if let Some(lk) = &state.livekit {
        lk.update_publish(&state.http, &call.room, &identity_of(&user_id), true)
            .await;
    }
    let (platform, _) = client_identity(&state, &headers, &device);
    Json(to_json(CallEnvelopeDto {
        call: call_dto(&state, &call, Some(&me), platform.as_deref()),
    }))
    .into_response()
}

/// `DELETE /calls/{id}/speakers/{user_id}` — host revokes speak permission.
pub async fn revoke_speaker(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (device, me, call) = match member_call(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !call.active() {
        return api_error(StatusCode::GONE, "call_ended", "this call has ended");
    }
    if me != call.started_by {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_host",
            "only the host can change who speaks",
        );
    }
    if user_id == call.started_by {
        return api_error(StatusCode::BAD_REQUEST, "is_host", "the host always speaks");
    }
    let _ = state.db.remove_call_speaker(&call.id, &user_id);
    if let Some(lk) = &state.livekit {
        lk.update_publish(&state.http, &call.room, &identity_of(&user_id), false)
            .await;
    }
    let (platform, _) = client_identity(&state, &headers, &device);
    Json(to_json(CallEnvelopeDto {
        call: call_dto(&state, &call, Some(&me), platform.as_deref()),
    }))
    .into_response()
}

// ---- VoIP tokens ----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct VoipBody {
    pub token: String,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
}

/// `POST /devices/{id}/voip {token, environment, bundle_id}` → `{push}`
pub async fn register_voip(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<VoipBody>,
) -> Response {
    let (device, user_id) = match device_user(&principal) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if device != id {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a device registers only its own token",
        );
    }
    let token = b.token.trim().to_ascii_lowercase();
    if token.is_empty() || token.len() > 512 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "token must be the hex PushKit token",
        );
    }
    let environment = match b.environment.as_deref().unwrap_or("production") {
        "development" => "development",
        "production" => "production",
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "environment must be development or production",
            )
        }
    };
    let t = VoipToken {
        device_id: id,
        user_id,
        token,
        environment: environment.to_owned(),
        bundle_id: b
            .bundle_id
            .unwrap_or_else(|| crate::apns::DEFAULT_TOPIC.to_owned()),
        updated_at: now_ms(),
    };
    match state.db.upsert_voip_token(&t) {
        Ok(()) => Json(to_json(VoipRegisterResponseDto {
            device_id: t.device_id,
            push: state.push.is_on(),
        }))
        .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `DELETE /devices/{id}/voip` → 204
pub async fn unregister_voip(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let (device, _) = match device_user(&principal) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if device != id {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "not your device");
    }
    match state.db.delete_voip_token(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

// ---- LiveKit webhook ------------------------------------------------------------------------------

/// `POST /livekit/webhook` — LiveKit tells us who is really in the room and when it closed.
pub async fn livekit_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(lk) = &state.livekit else {
        return calls_off();
    };
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(event) = lk.verify_webhook(auth, &body) else {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "bad_signature",
            "webhook signature",
        );
    };
    handle_webhook(&state, &event).await;
    StatusCode::OK.into_response()
}

/// The verified event body. Unknown rooms (not ours) and unknown events are ignored.
pub async fn handle_webhook(state: &AppState, event: &Value) {
    let Some(room) = event["room"]["name"].as_str() else {
        return;
    };
    let Ok(Some(call)) = state.db.call_by_room(room) else {
        return;
    };
    let identity = event["participant"]["identity"]
        .as_str()
        .unwrap_or_default();
    match event["event"].as_str().unwrap_or_default() {
        "participant_joined" => {
            if let Some(u) = user_of_identity(identity) {
                let _ = state.db.call_join(&call.id, u);
            } else if let Some(p) = crate::voice::project_of_identity(identity) {
                let _ = state.db.call_agent_join(&call.id, p);
            }
        }
        "participant_left" => {
            if let Some(u) = user_of_identity(identity) {
                let _ = state.db.call_leave(&call.id, u);
            } else if let Some(p) = crate::voice::project_of_identity(identity) {
                let _ = state.db.call_agent_leave(&call.id, p);
            }
        }
        "room_finished" => end_call(state, &call.id, "room_finished", None).await,
        _ => {
            let _ = state.db.touch_call(&call.id);
        }
    }
}

/// Every 15 s: end calls nobody is in any more (lost leaves or webhooks).
pub async fn spawn_sweeper(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            sweep(&state).await;
        }
    });
}

pub async fn sweep(state: &AppState) {
    let now = now_ms();
    for call in state.db.active_calls().unwrap_or_default() {
        let empty_since = state.db.call_empty_since(&call.id).ok().flatten();
        let abandoned = match empty_since {
            Some(at) => now - at > EMPTY_AFTER_MS,
            None => false,
        } || (now - call.last_activity_at > STALE_MS)
            || (state
                .db
                .call_participants(&call.id)
                .map(|p| p.is_empty())
                .unwrap_or(false)
                && now - call.started_at > EMPTY_START_MS);
        if abandoned {
            end_call(state, &call.id, "abandoned", None).await;
        }
    }
}

// ---- admin ------------------------------------------------------------------------------------------

/// `GET /admin/calls` → LiveKit status, live calls, recent ended calls, VoIP token count.
pub async fn admin_calls(State(state): State<AppState>, principal: Principal) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    let active: Vec<Value> = state
        .db
        .active_calls()
        .unwrap_or_default()
        .iter()
        .map(|c| call_json(&state, c, None, None))
        .collect();
    let recent: Vec<Value> = state
        .db
        .recent_calls(20)
        .unwrap_or_default()
        .iter()
        .map(|c| {
            json!({
                "call_id": c.id, "chat_id": c.chat_id, "kind": c.kind, "started_by": c.started_by,
                "started_at": c.started_at, "ended_at": c.ended_at, "reason": c.end_reason,
                "duration_s": c.ended_at.map(|e| (e - c.started_at) / 1000),
                "started_device": c.started_device, "platform": c.platform, "app_version": c.app_version,
                "joined": state.db.call_participants(&c.id).unwrap_or_default().iter().map(|p| p.user_id.clone()).collect::<Vec<_>>(),
                "voice": state.db.voice_calls_of(&c.id).unwrap_or_default().iter().map(|v| json!({
                    "project_id": v.project_id, "mode": v.usage.mode, "minutes": (v.usage.seconds / 60.0 * 100.0).round() / 100.0,
                    "audio_in_tokens": v.usage.audio_in_tokens, "audio_out_tokens": v.usage.audio_out_tokens,
                    "tool_calls": v.usage.tool_calls, "decisions": v.usage.decisions, "usd": v.usage.usd,
                    "ttfa_ms_p50": v.usage.ttfa_ms_p50, "barge_ms_p50": v.usage.barge_ms_p50, "stop_reason": v.usage.stop_reason,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    // What agentd commit each awake box runs (reported in its hello), and the fleet minimum:
    // the SHA of the awake box that reported longest ago (oldest build still running). A fix is
    // shipped everywhere when every awake box's `sha` descends from it; `all_same` is the quick check.
    // `real_*` counts only non-test users: test boxes are woken by workers on whatever image
    // they last had and would otherwise drag the fleet minimum down.
    let mut fleet: Vec<Value> = Vec::new();
    let mut awake_shas: Vec<(i64, String)> = Vec::new();
    let mut real_shas: Vec<(i64, String)> = Vec::new();
    for a in state.db.all_box_agentd().unwrap_or_default() {
        let (bs, _) = state.box_state(&a.user_id).await;
        // No user row (a deleted test user, or a box minted under a project id by mistake) is
        // never a real user's box: it must not hold the fleet minimum (3 h on 0.1.72, Oct 22).
        let is_test = state
            .db
            .user(&a.user_id)
            .ok()
            .flatten()
            .is_none_or(|u| u.test);
        if matches!(bs, arbos_core::wire::BoxState::Awake) {
            if let Some(sha) = &a.sha {
                awake_shas.push((a.reported_at, sha.clone()));
                if !is_test {
                    real_shas.push((a.reported_at, sha.clone()));
                }
            }
        }
        fleet.push(json!({ "user_id": a.user_id, "test": is_test, "state": bs, "sha": a.sha, "version": a.version, "image": a.image, "reported_at": a.reported_at }));
    }
    awake_shas.sort();
    real_shas.sort();
    let agentd = json!({
        "boxes": fleet,
        "awake": awake_shas.len(),
        "fleet_min_sha": awake_shas.first().map(|x| x.1.clone()),
        "all_same": awake_shas.windows(2).all(|w| w[0].1 == w[1].1),
        "shas": awake_shas.iter().map(|x| x.1.clone()).collect::<std::collections::BTreeSet<_>>(),
        "real_awake": real_shas.len(),
        "real_min_sha": real_shas.first().map(|x| x.1.clone()),
        "real_all_same": real_shas.windows(2).all(|w| w[0].1 == w[1].1),
    });
    Json(json!({
        "agentd": agentd,
        "livekit": state.livekit.as_ref().map(|l| json!({ "url": l.url, "api_key": l.api_key })),
        "push": state.push.is_on(),
        "voip_tokens": state.db.all_voip_tokens().map(|t| t.len()).unwrap_or(0),
        "active": active,
        "recent": recent,
        "voice": crate::voice::admin_json(&state),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    //! Invariants: only members start/join/see a call; a second start joins the existing call;
    //! `room_finished` ends it exactly once with `call_ended` to every member; the last leave
    //! ends it and tells that client to write the line.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;
    use base64::Engine;
    use sha2::Digest;

    use arbos_core::api::PushItemDto;

    use super::*;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn body_json(r: Response) -> Value {
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    fn with_livekit(state: &mut AppState) {
        state.livekit = Some(crate::livekit::LiveKit::new(
            "wss://lk.test",
            "APIkey",
            "secret123",
            None,
        ));
    }

    fn pushes_of(state: &AppState, p: &Principal) -> Vec<(String, Value)> {
        let Principal::Device { id, .. } = p else {
            unreachable!()
        };
        state
            .db
            .take_user_pushes(id)
            .unwrap()
            .into_iter()
            .map(|(_, kind, payload, _)| (kind, payload))
            .collect()
    }

    /// #1430 (Jacob, 2026-10-23, group "Arbonites"): a voice call into a group rings every
    /// other member (one `call_started` each), dispatches the group's agents, and writes
    /// **nothing** into the chat — the "started a voice call" line is the starter's client's,
    /// and the agent's greeting / join / leave notes stay in the call (`voice::chat_rows_of`).
    #[tokio::test]
    async fn group_call_rings_every_member_and_writes_no_chat_row() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (_bob, bob_dev) = person(&state, 2);
        let (_carol, carol_dev) = person(&state, 3);
        let (_dave, dave_dev) = person(&state, 4);
        let home = project_of(&state, &alice, "Home");
        let r = crate::people::chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(crate::people::CreateGroupBody {
                kind: "group".into(),
                members: vec!["user2".into(), "user3".into(), "user4".into()],
                agent_ids: vec![home.clone()],
                title: Some("Arbonites".into()),
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
        // Creating the group pushed `people_chat_updated`; only the call's pushes count below.
        for d in [&bob_dev, &carol_dev, &dave_dev, &alice_dev] {
            pushes_of(&state, d);
        }

        let r = start_people_call(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            HeaderMap::new(),
            Some(Json(StartBody {
                kind: Some("voice".into()),
                ..Default::default()
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::CREATED);
        let v = body_json(r).await;
        let call_id = v["call"]["call_id"].as_str().unwrap().to_owned();
        // The group's agent belongs to the call (dispatch itself needs a LiveKit server).
        let agents = v["call"]["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["id"], home);
        assert_eq!(agents[0]["kind"], "agent");
        // Every other member is rung exactly once; the starter is not.
        for (who, d) in [
            ("bob", &bob_dev),
            ("carol", &carol_dev),
            ("dave", &dave_dev),
        ] {
            let pushes = pushes_of(&state, d);
            let rings: Vec<_> = pushes.iter().filter(|(k, _)| k == "call_started").collect();
            assert_eq!(rings.len(), 1, "{who} rings once: {pushes:?}");
            assert_eq!(rings[0].1["call"]["call_id"], call_id);
        }
        assert!(pushes_of(&state, &alice_dev)
            .iter()
            .all(|(k, _)| k != "call_started"));
        // Starting the call wrote no chat row (the relay neither posts a line nor wakes
        // anyone; the starter's client writes "<me> started a voice call").
        let room = arbos_core::wire::room_name(people::PEOPLE_PROJECT, &chat);
        assert!(state.db.rows_after(&room, 0, None, 10).unwrap().is_empty());
        assert!(crate::matrix::backfill::load_doc(&state, &chat, None)
            .unwrap()
            .is_none());

        // The worker's first events: joined, the join note and the greeting. None is a chat row;
        // the agent is in the call.
        let r = crate::voice::call_events(
            State(state.clone()),
            Principal::Admin,
            Path(call_id.clone()),
            Json(crate::voice::EventsBody {
                project_id: Some(home.clone()),
                events: vec![
                    json!({ "kind": "joined", "mode": "duplex" }),
                    json!({ "kind": "call_note", "text": "Home joined the call" }),
                    json!({ "kind": "transcript", "speaker": { "kind": "agent" }, "text": "Hey — Home here. What do you need?" }),
                ],
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["accepted"], 3);
        assert_eq!(v["forwarded"], false, "no rows for the box");
        assert_eq!(
            state.db.call_agents_live(&call_id).unwrap(),
            vec![home.clone()]
        );
        assert!(state.db.rows_after(&room, 0, None, 10).unwrap().is_empty());
        // The pushes carry the agent too, so the callees' screens can show it.
        let r = get_call(
            State(state.clone()),
            bob_dev.clone(),
            Path(call_id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["call"]["agents"].as_array().unwrap().len(), 1);
        assert_eq!(v["call"]["participants"].as_array().unwrap().len(), 2);
    }

    /// Selective start: `ring: [bob]` notifies only Bob; Carol and Dave stay quiet, and the
    /// group's agent is not dispatched (Bob alone was named — no agent id in `ring`).
    #[tokio::test]
    async fn group_call_ring_list_notifies_only_named_members() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (_carol, carol_dev) = person(&state, 3);
        let (_dave, dave_dev) = person(&state, 4);
        let home = project_of(&state, &alice, "Home");
        let r = crate::people::chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(crate::people::CreateGroupBody {
                kind: "group".into(),
                members: vec!["user2".into(), "user3".into(), "user4".into()],
                agent_ids: vec![home.clone()],
                title: Some("Pick some".into()),
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
        for d in [&bob_dev, &carol_dev, &dave_dev, &alice_dev] {
            pushes_of(&state, d);
        }

        let r = start_people_call(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            HeaderMap::new(),
            Some(Json(StartBody {
                kind: Some("voice".into()),
                ring: Some(vec![bob.clone()]),
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::CREATED);
        let v = body_json(r).await;
        let call_id = v["call"]["call_id"].as_str().unwrap().to_owned();
        let rings_bob: Vec<_> = pushes_of(&state, &bob_dev)
            .into_iter()
            .filter(|(k, _)| k == "call_started")
            .collect();
        assert_eq!(rings_bob.len(), 1, "{rings_bob:?}");
        assert_eq!(rings_bob[0].1["call"]["call_id"], call_id);
        for (who, d) in [("carol", &carol_dev), ("dave", &dave_dev)] {
            let rings: Vec<_> = pushes_of(&state, d)
                .into_iter()
                .filter(|(k, _)| k == "call_started")
                .collect();
            assert!(rings.is_empty(), "{who} must stay quiet: {rings:?}");
        }
        let dispatched = v["agents_dispatched"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            dispatched.is_empty(),
            "no agent in ring → none dispatched: {dispatched:?}"
        );
    }

    #[tokio::test]
    async fn members_only_and_join_existing() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (_carol, carol_dev) = person(&state, 3);
        let chat = dm(&state, &alice, &bob);

        // A stranger cannot start, see, or join.
        let r = start_people_call(
            State(state.clone()),
            carol_dev.clone(),
            Path(chat.clone()),
            HeaderMap::new(),
            Some(Json(StartBody {
                kind: Some("video".into()),
                ..Default::default()
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let r = start_people_call(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            HeaderMap::new(),
            Some(Json(StartBody {
                kind: Some("video".into()),
                ..Default::default()
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::CREATED);
        let v = body_json(r).await;
        let call_id = v["call"]["call_id"].as_str().unwrap().to_owned();
        assert_eq!(v["call"]["kind"], "video");
        assert_eq!(v["call"]["project_id"], "people");
        assert_eq!(v["call"]["livekit_url"], "wss://lk.test");
        assert_eq!(v["call"]["started_by"]["handle"], "user1");
        assert_eq!(v["call"]["participants"].as_array().unwrap().len(), 1);
        let claims = crate::livekit::claims_of(v["call"]["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims["sub"], format!("u_{alice}"));
        assert_eq!(claims["video"]["room"], v["call"]["room"]);

        // Bob was told (not Alice); the push carries the call without a token.
        let bob_pushes = pushes_of(&state, &bob_dev);
        assert_eq!(bob_pushes.len(), 1);
        assert_eq!(bob_pushes[0].0, "call_started");
        assert_eq!(bob_pushes[0].1["call"]["call_id"], call_id);
        assert!(bob_pushes[0].1["call"]["token"].is_null());
        assert!(pushes_of(&state, &alice_dev).is_empty());

        // Bob starting "a call" in the same chat joins the live one.
        let r = start_people_call(
            State(state.clone()),
            bob_dev.clone(),
            Path(chat.clone()),
            HeaderMap::new(),
            None,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let v = body_json(r).await;
        assert_eq!(v["call"]["call_id"], call_id);
        assert_eq!(v["joined"], true);
        assert_eq!(v["call"]["participants"].as_array().unwrap().len(), 2);
        let claims = crate::livekit::claims_of(v["call"]["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims["sub"], format!("u_{bob}"));
        assert_eq!(v["call"]["speak_mode"], "open");
        assert_eq!(v["call"]["can_speak"], true);
        assert_eq!(claims["video"]["canPublish"], true);

        // Carol can neither join nor read it.
        let r = join_call(
            State(state.clone()),
            carol_dev.clone(),
            Path(call_id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = get_call(
            State(state.clone()),
            carol_dev.clone(),
            Path(call_id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = people_chat_call(State(state.clone()), carol_dev, Path(chat.clone())).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = people_chat_call(State(state.clone()), alice_dev.clone(), Path(chat.clone())).await;
        assert_eq!(r.status(), StatusCode::OK);

        // Alice leaves: Bob is still in, nothing ends. Bob leaves: it ends and he writes the line.
        let v = body_json(
            leave_call(
                State(state.clone()),
                alice_dev.clone(),
                Path(call_id.clone()),
            )
            .await,
        )
        .await;
        assert_eq!(v["ended"], false);
        assert!(state.db.call(&call_id).unwrap().unwrap().active());
        let v = body_json(
            leave_call(State(state.clone()), bob_dev.clone(), Path(call_id.clone())).await,
        )
        .await;
        assert_eq!(v["ended"], true);
        let call = state.db.call(&call_id).unwrap().unwrap();
        assert!(!call.active());
        assert_eq!(call.end_reason.as_deref(), Some("last_left"));
        let Principal::Device { id: bob_device, .. } = &bob_dev else {
            unreachable!()
        };
        for p in [&alice_dev, &bob_dev] {
            let ended: Vec<_> = pushes_of(&state, p)
                .into_iter()
                .filter(|(k, _)| k == "call_ended")
                .collect();
            assert_eq!(ended.len(), 1);
            assert_eq!(ended[0].1["writer_device"], *bob_device);
            assert_eq!(ended[0].1["reason"], "last_left");
        }
        let r = join_call(
            State(state.clone()),
            bob_dev.clone(),
            Path(call_id.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::GONE);
        let r = people_chat_call(State(state.clone()), alice_dev, Path(chat)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    /// Hosted speak on a group call: only the host publishes until they invite someone.
    #[tokio::test]
    async fn group_call_host_invites_speakers() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (_carol, carol_dev) = person(&state, 3);
        let r = crate::people::chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(crate::people::CreateGroupBody {
                kind: "group".into(),
                members: vec!["user2".into(), "user3".into()],
                agent_ids: vec![],
                title: Some("Speakers".into()),
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
            .expect("group")
            .chat
            .id;

        let v = body_json(
            start_people_call(
                State(state.clone()),
                alice_dev.clone(),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(StartBody {
                    kind: Some("voice".into()),
                    ..Default::default()
                })),
            )
            .await,
        )
        .await;
        let call_id = v["call"]["call_id"].as_str().unwrap().to_owned();
        assert_eq!(v["call"]["speak_mode"], "hosted");
        assert_eq!(v["call"]["can_speak"], true);
        assert!(v["call"]["invited_speakers"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty());
        let host_claims = crate::livekit::claims_of(v["call"]["token"].as_str().unwrap()).unwrap();
        assert_eq!(host_claims["video"]["canPublish"], true);

        let v = body_json(
            join_call(
                State(state.clone()),
                bob_dev.clone(),
                Path(call_id.clone()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(v["call"]["speak_mode"], "hosted");
        assert_eq!(v["call"]["can_speak"], false);
        let bob_claims = crate::livekit::claims_of(v["call"]["token"].as_str().unwrap()).unwrap();
        assert_eq!(bob_claims["video"]["canPublish"], false);

        // Non-host cannot invite.
        let r = invite_speaker(
            State(state.clone()),
            bob_dev.clone(),
            Path(call_id.clone()),
            HeaderMap::new(),
            Some(Json(SpeakerBody {
                user_id: Some(bob.clone()),
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let v = body_json(
            invite_speaker(
                State(state.clone()),
                alice_dev.clone(),
                Path(call_id.clone()),
                HeaderMap::new(),
                Some(Json(SpeakerBody {
                    user_id: Some(bob.clone()),
                })),
            )
            .await,
        )
        .await;
        assert_eq!(v["call"]["invited_speakers"].as_array().unwrap().len(), 1);
        assert_eq!(v["call"]["invited_speakers"][0]["id"], bob);

        let v = body_json(
            join_call(
                State(state.clone()),
                bob_dev.clone(),
                Path(call_id.clone()),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(v["call"]["can_speak"], true);
        let bob_claims = crate::livekit::claims_of(v["call"]["token"].as_str().unwrap()).unwrap();
        assert_eq!(bob_claims["video"]["canPublish"], true);

        let v = body_json(
            revoke_speaker(
                State(state.clone()),
                alice_dev.clone(),
                Path((call_id.clone(), bob.clone())),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert!(v["call"]["invited_speakers"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty());

        // Carol still listen-only; open-mic DMs stay open (covered above).
        let v = body_json(
            join_call(
                State(state.clone()),
                carol_dev.clone(),
                Path(call_id),
                HeaderMap::new(),
            )
            .await,
        )
        .await;
        assert_eq!(v["call"]["can_speak"], false);
    }

    #[tokio::test]
    async fn room_finished_ends_the_call_once() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let v = body_json(
            start_people_call(
                State(state.clone()),
                alice_dev.clone(),
                Path(chat.clone()),
                HeaderMap::new(),
                None,
            )
            .await,
        )
        .await;
        let call = state
            .db
            .call(v["call"]["call_id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let _ = pushes_of(&state, &bob_dev);

        // Webhooks: Bob joined, both left, the room closed.
        let ev = |event: &str, identity: &str| json!({ "event": event, "room": { "name": call.room }, "participant": { "identity": identity } });
        handle_webhook(&state, &ev("participant_joined", &format!("u_{bob}"))).await;
        assert_eq!(participants_dto(&state, &call).len(), 2);
        handle_webhook(&state, &ev("participant_left", &format!("u_{alice}"))).await;
        handle_webhook(&state, &ev("participant_left", &format!("u_{bob}"))).await;
        assert!(
            state.db.call(&call.id).unwrap().unwrap().active(),
            "waits for room_finished"
        );
        let finished = json!({ "event": "room_finished", "room": { "name": call.room } });
        handle_webhook(&state, &finished).await;
        handle_webhook(&state, &finished).await;
        let after = state.db.call(&call.id).unwrap().unwrap();
        assert_eq!(after.end_reason.as_deref(), Some("room_finished"));
        for p in [&alice_dev, &bob_dev] {
            let ended: Vec<_> = pushes_of(&state, p)
                .into_iter()
                .filter(|(k, _)| k == "call_ended")
                .collect();
            assert_eq!(
                ended.len(),
                1,
                "announced once even when the webhook repeats"
            );
            assert!(
                ended[0].1["writer_device"].is_string(),
                "one device writes the line"
            );
        }

        // The signed HTTP form: a valid signature is accepted, a bad one is not.
        let lk = state.livekit.as_ref().unwrap();
        let body = serde_json::to_vec(
            &json!({ "event": "room_started", "room": { "name": "call_other" } }),
        )
        .unwrap();
        let digest = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(&body));
        let mut headers = HeaderMap::new();
        let token = {
            let claims = json!({ "iss": "APIkey", "sha256": digest, "exp": now_ms() / 1000 + 60 });
            let hdr = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(br#"{"alg":"HS256","typ":"JWT"}"#);
            let pl = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
            let input = format!("{hdr}.{pl}");
            let mut mac =
                <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(b"secret123").unwrap();
            hmac::Mac::update(&mut mac, input.as_bytes());
            let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(hmac::Mac::finalize(mac).into_bytes());
            format!("{input}.{sig}")
        };
        assert!(lk.verify_webhook(&token, &body).is_some());
        headers.insert(axum::http::header::AUTHORIZATION, token.parse().unwrap());
        let r = livekit_webhook(State(state.clone()), headers, body.clone().into()).await;
        assert_eq!(r.status(), StatusCode::OK);
        let mut bad = HeaderMap::new();
        bad.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer x.y.z".parse().unwrap(),
        );
        let r = livekit_webhook(State(state.clone()), bad, body.into()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn online_member_looking_at_the_chat_still_gets_call_started() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let Principal::Device { id: bob_device, .. } = &bob_dev else {
            unreachable!()
        };
        // Bob's app is open: fresh presence, focused on this very chat, device just seen.
        state.db.touch_device(bob_device).unwrap();
        notify::set_focus(&state, bob_device, Some(&chat)).await;
        let card = crate::people::with_presence(
            &state,
            arbos_core::api::UserCardDto {
                id: bob.clone(),
                ..Default::default()
            },
        );
        assert_eq!(card.online, Some(true));

        let r = start_people_call(
            State(state.clone()),
            alice_dev,
            Path(chat.clone()),
            HeaderMap::new(),
            None,
        )
        .await;
        assert_eq!(r.status(), StatusCode::CREATED);
        let kinds: Vec<String> = pushes_of(&state, &bob_dev)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            kinds,
            vec!["call_started".to_owned()],
            "ringing is never suppressed by focus or presence"
        );
    }

    #[tokio::test]
    async fn missed_calls_and_reachability() {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (carol, carol_dev) = person(&state, 3);
        // A three-person chat: Bob's phone is live and has a VoIP token; Carol is dark.
        let chat = dm(&state, &alice, &bob);
        state
            .db
            .add_member(
                &chat,
                &format!("user:{carol}"),
                "member",
                &alice,
                &json!({}),
            )
            .unwrap();
        let Principal::Device { id: bob_device, .. } = &bob_dev else {
            unreachable!()
        };
        state.db.touch_device(bob_device).unwrap();
        state
            .db
            .upsert_voip_token(&VoipToken {
                device_id: bob_device.clone(),
                user_id: bob.clone(),
                token: "ab".repeat(32),
                environment: "production".into(),
                bundle_id: "com.unarbos.arbos.ios".into(),
                updated_at: now_ms(),
            })
            .unwrap();
        state
            .db
            .upsert_push_token(&crate::db_push::PushToken {
                device_id: bob_device.clone(),
                user_id: bob.clone(),
                platform: "ios".into(),
                token: "cd".repeat(32),
                environment: "production".into(),
                bundle_id: "com.unarbos.arbos.ios".into(),
                app_version: "1.0.0 (202610161326)".into(),
                updated_at: now_ms(),
            })
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("x-arbos-client", "mac/0.6.1 (456)".parse().unwrap());
        let r = start_people_call(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            headers,
            Some(Json(StartBody {
                kind: Some("video".into()),
                ..Default::default()
            })),
        )
        .await;
        assert_eq!(r.status(), StatusCode::CREATED);
        let v = body_json(r).await;
        assert_eq!(v["call"]["platform"], "mac");
        assert_eq!(v["call"]["app_version"], "0.6.1 (456)");
        let Principal::Device {
            id: alice_device, ..
        } = &alice_dev
        else {
            unreachable!()
        };
        assert_eq!(v["call"]["started_device"], *alice_device);
        let reach = v["reachable"].as_array().unwrap();
        assert_eq!(reach.len(), 2, "every callee, not the caller");
        let for_bob = reach.iter().find(|r| r["user_id"] == bob).unwrap();
        let for_carol = reach.iter().find(|r| r["user_id"] == carol).unwrap();
        assert_eq!(for_bob["any_live"], true);
        assert_eq!(for_bob["any_voip"], true);
        assert_eq!(for_bob["any_push"], true);
        assert_eq!(for_bob["devices"][0]["polled_within_30s"], true);
        assert_eq!(for_bob["devices"][0]["voip_token"], true);
        assert_eq!(for_bob["devices"][0]["push_token"], true);
        assert_eq!(for_bob["devices"][0]["app_version"], "1.0.0 (202610161326)");
        assert_eq!(for_carol["any_live"], false);
        assert_eq!(for_carol["any_voip"], false);
        assert_eq!(for_carol["any_push"], false);
        assert_eq!(for_carol["devices"][0]["polled_within_30s"], false);

        // Bob joins, Alice and Bob hang up; Carol never answered → only Carol gets call_missed.
        let call_id = v["call"]["call_id"].as_str().unwrap().to_owned();
        join_call(
            State(state.clone()),
            bob_dev.clone(),
            Path(call_id.clone()),
            HeaderMap::new(),
        )
        .await;
        leave_call(
            State(state.clone()),
            alice_dev.clone(),
            Path(call_id.clone()),
        )
        .await;
        let v = body_json(
            leave_call(State(state.clone()), bob_dev.clone(), Path(call_id.clone())).await,
        )
        .await;
        assert_eq!(v["ended"], true);
        let carol_pushes = pushes_of(&state, &carol_dev);
        let missed: Vec<_> = carol_pushes
            .iter()
            .filter(|(k, _)| k == "call_missed")
            .collect();
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].1["from"]["handle"], "user1");
        assert_eq!(missed[0].1["kind"], "video");
        assert_eq!(missed[0].1["call_id"], call_id);
        assert!(carol_pushes.iter().any(|(k, _)| k == "call_ended"));
        for p in [&alice_dev, &bob_dev] {
            assert!(
                !pushes_of(&state, p).iter().any(|(k, _)| k == "call_missed"),
                "the caller and a member who joined never get call_missed"
            );
        }
        // The only APNs token belongs to Bob, who joined: a "started" alert went out, no "missed".
        state.push.drain().await;
        let alerts = state.push.captured().await;
        assert!(alerts.iter().any(|o| o.payload["event"] == "started"));
        assert!(alerts.iter().all(|o| o.payload["event"] != "missed"));
    }

    #[tokio::test]
    async fn calls_are_off_without_livekit() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let r = start_people_call(
            State(state.clone()),
            alice_dev,
            Path(chat),
            HeaderMap::new(),
            None,
        )
        .await;
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ---- wire fixtures (step 6b-2b) ------------------------------------------------------------

    const FIXED_AT: i64 = 1_700_000_000_000;
    const FIXED_TOKEN: &str = "fixture.join.token";
    const FIXED_TOKEN_EXP: i64 = FIXED_AT + 3_600_000;

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api")
    }

    fn load_fixture(name: &str) -> Value {
        let path = fixtures_dir().join(name);
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&raw).expect("fixture json")
    }

    fn assert_builder(name: &str, got: Value) {
        let path = fixtures_dir().join(name);
        if !path.exists() || std::env::var_os("UPDATE_API_FIXTURES").is_some() {
            let pretty = serde_json::to_string_pretty(&got).expect("pretty");
            std::fs::write(&path, format!("{pretty}\n")).expect("write fixture");
        }
        let want = load_fixture(name);
        assert_eq!(got, want, "fixture {name}");
    }

    fn stabilize_call_token(mut v: Value) -> Value {
        if v.get("token").and_then(Value::as_str).is_some() {
            v["token"] = json!(FIXED_TOKEN);
            v["token_expires_at"] = json!(FIXED_TOKEN_EXP);
        }
        if let Some(call) = v.get_mut("call") {
            if call.get("token").and_then(Value::as_str).is_some() {
                call["token"] = json!(FIXED_TOKEN);
                call["token_expires_at"] = json!(FIXED_TOKEN_EXP);
            }
        }
        v
    }

    fn rename_device(state: &AppState, old: &str, new_id: &str, name: &str) {
        state
            .db
            .with(|c| {
                c.execute(
                    "UPDATE devices SET id = ?2, name = ?3, kind = 'mac', created_at = ?4, last_seen_at = ?4 WHERE id = ?1",
                    rusqlite::params![old, new_id, name, FIXED_AT],
                )?;
                Ok(())
            })
            .unwrap();
    }

    fn fixture_call_setup() -> (AppState, String, String, CallRow) {
        let mut state = test_state(AuthMode::Github);
        with_livekit(&mut state);
        let (alice, alice_dev) = person(&state, 201);
        let (bob, bob_dev) = person(&state, 202);
        let Principal::Device { id: alice_old, .. } = &alice_dev else {
            unreachable!()
        };
        let Principal::Device { id: bob_old, .. } = &bob_dev else {
            unreachable!()
        };
        rename_device(&state, alice_old, "dev_alice_call", "Alice Mac");
        rename_device(&state, bob_old, "dev_bob_call", "Bob iPhone");
        state
            .db
            .upsert_voip_token(&VoipToken {
                device_id: "dev_bob_call".into(),
                user_id: bob.clone(),
                token: "ab".repeat(32),
                environment: "production".into(),
                bundle_id: "com.unarbos.arbos.ios".into(),
                updated_at: FIXED_AT,
            })
            .unwrap();
        state
            .db
            .upsert_push_token(&crate::db_push::PushToken {
                device_id: "dev_bob_call".into(),
                user_id: bob.clone(),
                platform: "ios".into(),
                token: "cd".repeat(32),
                environment: "production".into(),
                bundle_id: "com.unarbos.arbos.ios".into(),
                app_version: "1.0.0 (fixture)".into(),
                updated_at: FIXED_AT,
            })
            .unwrap();
        let chat = crate::db_people::PeopleChat {
            id: "people_call_fixture".into(),
            kind: "dm".into(),
            created_by: alice.clone(),
            created_at: FIXED_AT,
            title: None,
            image_key: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: None,
            allowed_agents: "members_own".into(),
        };
        state.db.insert_people_chat(&chat).unwrap();
        for u in [&alice, &bob] {
            state
                .db
                .add_member(&chat.id, &format!("user:{u}"), "owner", &alice, &json!({}))
                .unwrap();
        }
        let call = CallRow {
            id: "call_fixture".into(),
            chat_id: chat.id.clone(),
            project_id: None,
            kind: "voice".into(),
            room: "call_room_fixture".into(),
            started_by: alice.clone(),
            started_at: FIXED_AT,
            ended_at: None,
            end_reason: None,
            last_activity_at: FIXED_AT,
            started_device: Some("dev_alice_call".into()),
            platform: Some("mac".into()),
            app_version: Some("0.6.24".into()),
        };
        state.db.insert_call(&call).unwrap();
        let _ = state.db.call_join(&call.id, &alice);
        (state, alice, bob, call)
    }

    #[tokio::test]
    async fn call_wire_fixtures() {
        let (state, alice, bob, call) = fixture_call_setup();

        let live =
            stabilize_call_token(to_json(call_dto(&state, &call, Some(&alice), Some("mac"))));
        assert_builder("call_live_with_token.json", live);

        let mut ended_row = call.clone();
        ended_row.ended_at = Some(FIXED_AT + 60_000);
        ended_row.end_reason = Some("last_left".into());
        assert_builder(
            "call_ended.json",
            to_json(call_dto(&state, &ended_row, None, None)),
        );

        let reachable = reachability(&state, std::slice::from_ref(&bob)).await;
        let start = stabilize_call_token(to_json(StartCallResponseDto {
            call: call_dto(&state, &call, Some(&alice), Some("mac")),
            joined: false,
            reachable: Some(reachable),
            agents_dispatched: Some(Value::Array(vec![])),
        }));
        assert_builder("start_call_response.json", start);

        let join = stabilize_call_token(to_json(CallEnvelopeDto {
            call: call_dto(&state, &call, Some(&bob), None),
        }));
        assert_builder("join_call_response.json", join);

        assert_builder(
            "leave_call_response.json",
            to_json(LeaveCallResponseDto {
                ended: true,
                duration_s: 60,
                already_ended: None,
            }),
        );

        let call_d = call_dto(&state, &call, None, None);
        let starter = Sender::user(&state, &alice).dto();
        let started = CallStartedPayloadDto {
            call: call_d.clone(),
            sender: starter.clone(),
        };
        let ended = CallEndedPayloadDto {
            call_id: call.id.clone(),
            chat_id: call.chat_id.clone(),
            project_id: people::PEOPLE_PROJECT.to_owned(),
            duration_s: 60,
            reason: "last_left".into(),
            writer_device: Some("dev_bob_call".into()),
        };
        let missed = CallMissedPayloadDto {
            call_id: call.id.clone(),
            chat_id: call.chat_id.clone(),
            project_id: people::PEOPLE_PROJECT.to_owned(),
            kind: call.kind.clone(),
            from: starter.clone(),
            started_at: call.started_at,
            ended_at: FIXED_AT + 60_000,
            duration_s: 60,
            reason: Some("abandoned".into()),
        };
        assert_builder("call_started_payload.json", to_json(&started));
        assert_builder("call_ended_payload.json", to_json(&ended));
        assert_builder("call_missed_payload.json", to_json(&missed));
        assert_builder(
            "push_call_started.json",
            to_json(PushItemDto {
                id: "push_call_started".into(),
                created_at: FIXED_AT,
                event: PushEventDto::CallStarted(started),
            }),
        );
        assert_builder(
            "push_call_ended.json",
            to_json(PushItemDto {
                id: "push_call_ended".into(),
                created_at: FIXED_AT,
                event: PushEventDto::CallEnded(ended),
            }),
        );
        assert_builder(
            "push_call_missed.json",
            to_json(PushItemDto {
                id: "push_call_missed".into(),
                created_at: FIXED_AT,
                event: PushEventDto::CallMissed(missed),
            }),
        );
        assert_builder(
            "call_ring_payload.json",
            to_json(CallRingPayloadDto {
                call_id: call.id.clone(),
                chat_id: call.chat_id.clone(),
                project_id: people::PEOPLE_PROJECT.to_owned(),
                kind: call.kind.clone(),
                room: call.room.clone(),
                livekit_url: call_d.livekit_url.clone(),
                caller: starter.clone(),
                started_at: call.started_at,
            }),
        );
        assert_builder(
            "call_alert.json",
            to_json(CallAlertDto {
                event: "started".into(),
                chat_id: call.chat_id.clone(),
                project_id: people::PEOPLE_PROJECT.to_owned(),
                call: call_d,
                sender: Some(starter),
            }),
        );
    }
}
