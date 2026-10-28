//! Message notifications (iOS APNs today; the Mac reads the same `people_message` pushes).
//!
//! The relay never parses Loro rows, so whoever writes a whole message tells it:
//! agentd when an agent reply completes, a device when its person sends. Each announcement is
//! one `chat_events` row (deduplicated by `(chat, message_id)`), which drives unread counts and
//! one APNs alert per recipient device — never the sender's own devices, never a device whose
//! presence says that chat is on screen. Reading a chat anywhere pushes the new badge to the
//! user's other devices and a `notifications_cleared` row on their `/me/pushes` queue, so they
//! drop the chat's banners ([`clear_on_read`]).
//!
//! Endpoints: `POST/DELETE /devices/{id}/push`, `GET /me/unread`, `POST /chats/{id}/notify`
//! (people chats), `POST /projects/{project}/chats/{chat}/notify`; `PUT /chats/{id}/read` (in
//! `people.rs`) accepts project chat ids too.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use arbos_core::api::{PushRegisterResponseDto, SenderDto, UnreadChatDto, UnreadDto};
use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::db::Project;
use crate::db_push::PushToken;
use crate::people;
use crate::push::eligibility::{self, Recipient, Skip, Verdict};
use crate::push::{payload, NewPush, PushKind, BADGE_TTL_MS, MESSAGE_TTL_MS};
use crate::signin::api_error;
use crate::state::AppState;

/// APNs category the apps register; the service extension turns it into a communication notification.
pub const CATEGORY: &str = "ARBOS_MESSAGE";
/// Alert body cap (the lock screen shows about this much).
pub const BODY_MAX: usize = 140;
/// A presence frame arrives every 15 s; a focus older than this is stale (the app is gone).
pub const FOCUS_TTL_MS: i64 = 45_000;
/// How long the row-level fallback waits for a proper announcement before it notifies without content.
const FALLBACK_DELAY: Duration = Duration::from_millis(2500);
/// At most one content-less fallback per (chat, recipient) per minute.
const FALLBACK_EVERY_MS: i64 = 60_000;

/// device_id → (chat id on screen, when it said so).
pub type Focus = Arc<Mutex<HashMap<String, (String, i64)>>>;

#[derive(Debug, Clone)]
pub struct ChatRef {
    pub chat_id: String,
    /// `None` for people chats.
    pub project_id: Option<String>,
}

impl ChatRef {
    pub fn people(chat_id: &str) -> Self {
        Self {
            chat_id: chat_id.to_owned(),
            project_id: None,
        }
    }

    pub fn project(project_id: &str, chat_id: &str) -> Self {
        Self {
            chat_id: chat_id.to_owned(),
            project_id: Some(project_id.to_owned()),
        }
    }

    /// What the apps see as `project_id`: `people` for DMs.
    pub fn project_label(&self) -> &str {
        self.project_id.as_deref().unwrap_or(people::PEOPLE_PROJECT)
    }
}

/// Who wrote the message, as the apps render it.
#[derive(Debug, Clone)]
pub struct Sender {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub handle: String,
    pub avatar: Option<String>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
    /// The person behind it (the user for a human; the owner for an agent is *not* excluded).
    pub user_id: Option<String>,
    /// Agent's initials-colour override (D79); people never have one.
    pub avatar_color_index: Option<i64>,
    /// Agent's leaf avatar in effect (D92: shape 0–8, disc colour 0–9); people never have one.
    pub leaf: Option<arbos_core::avatar::leaf::LeafPick>,
}

impl Sender {
    pub fn user(state: &AppState, user_id: &str) -> Self {
        let card = state
            .db
            .user_card(user_id)
            .ok()
            .flatten()
            .unwrap_or_default();
        let handle = if card.handle.is_empty() {
            user_id.to_owned()
        } else {
            card.handle
        };
        let name = card
            .name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("@{handle}"));
        Self {
            kind: "user",
            id: user_id.to_owned(),
            name,
            handle,
            avatar: None,
            avatar_color: None,
            avatar_url: card.avatar_url,
            user_id: Some(user_id.to_owned()),
            avatar_color_index: None,
            leaf: None,
        }
    }

    pub fn agent(state: &AppState, p: &Project) -> Self {
        Self {
            kind: "agent",
            id: p.id.clone(),
            name: p.name.clone(),
            handle: String::new(),
            avatar: Some(
                p.avatar
                    .clone()
                    .unwrap_or_else(|| crate::avatar::default_for(&p.id, &state.cfg.avatar_slugs)),
            ),
            avatar_color: Some(p.avatar_color.clone().unwrap_or_else(|| {
                crate::avatar::default_color_for(&p.id, &state.cfg.avatar_colors)
            })),
            avatar_url: None,
            user_id: None,
            avatar_color_index: p.avatar_color_index,
            leaf: Some(crate::avatar::effective_leaf(p)),
        }
    }

    pub fn dto(&self) -> SenderDto {
        SenderDto {
            kind: self.kind.to_owned(),
            id: self.id.clone(),
            name: self.name.clone(),
            handle: self.handle.clone(),
            avatar: self.avatar.clone(),
            avatar_color: self.avatar_color.clone(),
            avatar_url: self.avatar_url.clone(),
            avatar_color_index: self.avatar_color_index,
            avatar_color_index_effective: i64::from(crate::avatar::effective_color_index(
                &self.id,
                self.avatar_color_index,
            )),
            avatar_leaf_effective: self.leaf.map(|l| i64::from(l.leaf)),
            avatar_leaf_color_effective: self.leaf.map(|l| i64::from(l.color)),
        }
    }

    pub fn json(&self) -> Value {
        serde_json::to_value(self.dto()).expect("SenderDto")
    }

    /// Alert title: the agent's name, or `@handle` for a person.
    pub fn title(&self) -> String {
        if self.kind == "agent" || self.handle.is_empty() {
            self.name.clone()
        } else {
            format!("@{}", self.handle)
        }
    }
}

/// First `BODY_MAX` characters of the message, one line, no trailing fragments of whitespace.
pub fn brief(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= BODY_MAX {
        return one_line;
    }
    let cut: String = one_line.chars().take(BODY_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// People who should hear about a message in this chat (the sender included; callers skip them).
pub fn recipients(state: &AppState, chat: &ChatRef) -> Vec<String> {
    let mut out = match &chat.project_id {
        None => state
            .db
            .members_of(&chat.chat_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|m| m.user_id().map(str::to_owned))
            .collect(),
        Some(pid) => {
            let mut v: Vec<String> = state.user_of(pid).into_iter().collect();
            v.extend(
                state
                    .db
                    .project_members(pid)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(u, _, _)| u),
            );
            v
        }
    };
    out.sort();
    out.dedup();
    out
}

/// Is this device looking at this chat right now?
pub async fn device_focused(state: &AppState, device_id: &str, chat_id: &str) -> bool {
    let focus = state.focus.lock().await;
    match focus.get(device_id) {
        Some((c, at)) => c == chat_id && now_ms() - at < FOCUS_TTL_MS,
        None => false,
    }
}

/// Is any of this user's devices looking at `chat_id` right now?
async fn user_focused_on_chat(state: &AppState, user_id: &str, chat_id: &str) -> bool {
    let ids = state.db.devices_of_user(user_id).unwrap_or_default();
    for d in ids {
        let Some(id) = d.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if device_focused(state, id, chat_id).await {
            return true;
        }
    }
    false
}

/// Presence from a room socket: remember (or forget) what the device looks at.
pub async fn set_focus(state: &AppState, device_id: &str, focused_chat: Option<&str>) {
    let mut focus = state.focus.lock().await;
    match focused_chat {
        Some(c) => {
            focus.insert(device_id.to_owned(), (c.to_owned(), now_ms()));
        }
        None => {
            focus.remove(device_id);
        }
    }
}

/// The socket to `chat_id` closed: that device is no longer looking at it.
pub async fn clear_focus(state: &AppState, device_id: &str, chat_id: &str) {
    let mut focus = state.focus.lock().await;
    if focus.get(device_id).is_some_and(|(c, _)| c == chat_id) {
        focus.remove(device_id);
    }
}

/// What a notification needs to know about the chat it is for, beyond its id: the kind and —
/// for anything but a DM — the name the recipient sees in their list, plus the group picture.
/// A DM is titled with the sender; a group, shared project chat or channel is titled with the
/// chat (Messages / Slack shape) and the sender moves into the body ("Sender: text"), so the
/// banner's first line says *where* the message is from. Decided 2026-10-27 (Jacob: "cannot
/// tell which group a message came from"); the alternative "Sender in Group" title was
/// rejected because the iOS communication intent and Notification Center group by title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMeta {
    /// `dm` | `group` | `project` | `channel`.
    pub kind: &'static str,
    /// The list name for this recipient; `None` for a DM.
    pub title: Option<String>,
    /// Blob key of the group picture under `people:{chat_id}` (people groups only).
    pub image_key: Option<String>,
}

impl ChatMeta {
    pub fn dm() -> Self {
        Self {
            kind: "dm",
            title: None,
            image_key: None,
        }
    }

    /// Is the chat titled by itself (group/project/channel) rather than by the sender?
    pub fn titled(&self) -> bool {
        self.title.as_deref().is_some_and(|t| !t.is_empty())
    }
}

/// The chat meta as `recipient` sees it. An untitled people group is named after the *other*
/// members' handles (the same rule as `GET /chats`), so it is per recipient. A project chat
/// counts as a group only when it is shared (owner plus guests); the owner's own 1:1 agent
/// chat is a DM with the agent.
pub fn chat_meta(state: &AppState, chat: &ChatRef, recipient: &str) -> ChatMeta {
    match &chat.project_id {
        Some(pid) => {
            let shared = state
                .db
                .project_members(pid)
                .map(|m| !m.is_empty())
                .unwrap_or(false);
            if !shared {
                return ChatMeta::dm();
            }
            ChatMeta {
                kind: "project",
                title: state.db.project(pid).ok().flatten().map(|p| p.name),
                image_key: None,
            }
        }
        None => {
            let Some(c) = state.db.people_chat(&chat.chat_id).ok().flatten() else {
                return ChatMeta::dm();
            };
            if c.kind == "dm" {
                return ChatMeta::dm();
            }
            let title = c
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    let members = state.db.members_of(&chat.chat_id).unwrap_or_default();
                    let others: Vec<String> = members
                        .iter()
                        .filter_map(|m| m.user_id())
                        .filter(|u| *u != recipient)
                        .map(|u| people::handle_of(state, u))
                        .filter(|h| !h.is_empty())
                        .collect();
                    let named = if others.is_empty() {
                        members
                            .iter()
                            .filter_map(|m| m.project_id())
                            .filter_map(|p| state.db.project(p).ok().flatten())
                            .map(|p| p.name)
                            .collect()
                    } else {
                        others
                    };
                    named.join(", ")
                });
            ChatMeta {
                kind: "group",
                title: Some(title),
                image_key: c.image_key.filter(|k| !k.is_empty()),
            }
        }
    }
}

/// One outbox entry for a message alert to one device.
fn message_push(token: &PushToken, chat: &ChatRef, message_id: &str, payload: Value) -> NewPush {
    let kind = PushKind::Message {
        chat_id: chat.chat_id.clone(),
        message_id: message_id.to_owned(),
    };
    NewPush {
        user_id: token.user_id.clone(),
        device_id: token.device_id.clone(),
        collapse_id: Some(kind.collapse_id(&token.user_id)),
        kind,
        payload,
        ttl_ms: MESSAGE_TTL_MS,
    }
}

/// Announce one whole message: record it, then alert every recipient device except the
/// sender's own and those focused on the chat. Returns the event id, `None` when already seen.
///
/// A recipient with the chat on screen is marked read through this event. Notify can land
/// after Loro already showed the row and the client marked read — without this, the list
/// badge counts messages the user already saw.
pub async fn announce(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    text: &str,
) -> Option<i64> {
    announce_with(state, chat, sender, message_id, text, None, &[]).await
}

/// [`announce`] with an optional cross-chat reply reference (Reply in DM): the
/// alert body says what it answers and each recipient's payload carries the
/// address only when they can open the source chat.
pub async fn announce_with(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    text: &str,
    reply_ref: Option<&ReplyRefBody>,
    mentions: &[String],
) -> Option<i64> {
    let event_id = state
        .db
        .insert_chat_event(
            &chat.chat_id,
            chat.project_id.as_deref(),
            message_id,
            sender.user_id.as_deref(),
        )
        .ok()
        .flatten()?;
    // Writing in a chat is reading it: the sender's own cursor moves past everything so far,
    // on every one of their devices (the cursor is per user).
    if let Some(u) = sender.user_id.as_deref() {
        let _ = state.db.mark_chat_read(u, &chat.chat_id);
    }
    // Outside agents in this chat get the message as an ASP event (#2140).
    crate::outside::on_announce(state, chat, sender, message_id, text, None, mentions);
    let member = match sender.kind {
        "agent" => format!("agent:{}", sender.id),
        _ => format!("user:{}", sender.id),
    };
    state.matrix.message(
        state,
        &chat.chat_id,
        chat.project_id.as_deref(),
        message_id,
        &member,
        text,
    );
    let body = match reply_ref {
        // "↩ Jacob in Arbonites: Works" — the banner says what the DM answers.
        Some(r) if !r.sender_name.trim().is_empty() && !r.chat_title.trim().is_empty() => {
            brief(&format!(
                "↩ {} in {}: {text}",
                r.sender_name.trim(),
                r.chat_title.trim()
            ))
        }
        _ => brief(text),
    };
    // An agent's reply reaches the apps' in-app banner path (`/me/pushes` → local
    // notification; what a phone gets while push is off or while it is open on another chat)
    // only from here: a device's own row queues it in `people::on_room_push`, a box's row does
    // not, and until 2026-10-26 nothing queued it for agents at all — an agent's answer in a
    // group never showed on the iPhone while a like on a bubble did. Per recipient, because an
    // untitled group is named after the *other* members.
    let inapp_seq = (sender.kind == "agent").then(|| {
        let room = arbos_core::wire::room_name(chat.project_label(), &chat.chat_id);
        let (head, _) = state.db.room_activity(&room).unwrap_or((0, 0));
        head + 1
    });
    let shape = eligibility::chat_shape(state, chat);
    let large_group_min = eligibility::large_group_min();
    let mut pushes = Vec::new();
    for user in recipients(state, chat) {
        let recipient = Recipient::load(state, chat, sender, &user, text, mentions);
        let verdict = eligibility::decide(&recipient, shape, large_group_min);
        // The sender's own account, and a recipient who blocked the sender (the row is hidden
        // for them alone, [feedback blocked-state-relay-2026-10-26]): not even a read mark.
        if matches!(verdict, Verdict::Skip(Skip::Sender | Skip::BlockedSender)) {
            continue;
        }
        if user_focused_on_chat(state, &user, &chat.chat_id).await {
            let _ = state.db.mark_chat_read(&user, &chat.chat_id);
            // Queued after the message job for this chat, so the receipt lands on it.
            state.matrix.read(state, &chat.chat_id, &user);
        }
        // Archived, level `none`, or mentions-only without a mention: no banner (in-app or
        // APNs) and no badge; the unread count excludes archived chats on its own.
        if let Verdict::Skip(why) = verdict {
            tracing::debug!(
                user = %user, chat = %chat.chat_id, message = %message_id, why = why.label(),
                "push: recipient skipped"
            );
            continue;
        }
        let meta = chat_meta(state, chat, &user);
        if let Some(seq) = inapp_seq {
            let event = arbos_core::api::PushEventDto::PeopleMessage(
                arbos_core::api::PeopleMessagePayloadDto {
                    chat_id: chat.chat_id.clone(),
                    from: format!("agent:{}", sender.id),
                    from_handle: sender.handle.clone(),
                    seq,
                    sender: sender.dto(),
                    project_id: chat.project_id.clone(),
                    text: Some(body.clone()),
                    chat_kind: Some(meta.kind.to_owned()),
                    chat_title: meta.title.clone(),
                    chat_image_key: meta.image_key.clone(),
                },
            );
            let _ = state.db.push_user_event(&user, &event, Some(&chat.chat_id));
        }
        if !state.push.is_on() {
            continue;
        }
        // Stage B: a flipped user's alerts come from the Matrix push gateway instead.
        if crate::matrix::reads::reads_on(state, &user) {
            continue;
        }
        let tokens = state.db.push_tokens_of_user(&user).unwrap_or_default();
        if tokens.is_empty() {
            continue;
        }
        let badge = state.db.unread_total(&user).unwrap_or(0);
        let reply_ref_json = reply_ref.map(|r| r.for_recipient(state, &user));
        for t in tokens {
            if device_focused(state, &t.device_id, &chat.chat_id).await {
                continue;
            }
            let mut p = payload::message_alert(chat, sender, message_id, &body, badge, &meta);
            if let Some(r) = &reply_ref_json {
                p["reply_ref"] = r.clone();
            }
            pushes.push(message_push(&t, chat, message_id, p));
        }
    }
    state.push.enqueue(pushes);
    Some(event_id)
}

/// One alert to one device (the Matrix push-gateway path, stage B): same payload, focus and
/// archive rules as [`announce`], for a message the homeserver told us about.
pub async fn alert_device(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    text: &str,
    badge: i64,
    device_id: &str,
) {
    if !state.push.is_on() || device_focused(state, device_id, &chat.chat_id).await {
        return;
    }
    let Some(user_id) = state.db.device_user(device_id).ok().flatten() else {
        return;
    };
    let Some(t) = state
        .db
        .push_tokens_of_user(&user_id)
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.device_id == device_id)
    else {
        return;
    };
    if state
        .db
        .chat_archived_at(&user_id, &chat.chat_id)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let p = payload::message_alert(
        chat,
        sender,
        message_id,
        &brief(text),
        badge,
        &chat_meta(state, chat, &user_id),
    );
    state
        .push
        .enqueue(vec![message_push(&t, chat, message_id, p)]);
}

/// Kind of the clear-on-read row on `/me/pushes`.
pub const CLEAR_KIND: &str = "notifications_cleared";

/// The user read `chat` on `except_device`: their other devices drop the banners they still
/// show for it and move their badge to the new unread total. The clear rides on `/me/pushes`
/// (one pending row per chat per device, newest badge wins; undelivered `people_message` rows
/// for the chat go with it so a device that has not drained yet never posts a banner for what
/// was already read). Returns the devices told. Same design as Fluxer's clear job, written
/// for this relay's queue; the silent APNs form of the same event belongs on the push
/// provider once it exists.
pub async fn clear_on_read(
    state: &AppState,
    user_id: &str,
    except_device: &str,
    chat: &ChatRef,
    read_upto: &str,
) -> Vec<String> {
    let badge = crate::matrix::reads::unread_total(state, user_id).await;
    silent_clear(state, user_id, except_device, chat, read_upto, badge);
    let event = arbos_core::api::PushEventDto::NotificationsCleared(
        arbos_core::api::NotificationsClearedPayloadDto {
            chat_id: chat.chat_id.clone(),
            project_id: chat.project_label().to_owned(),
            badge,
        },
    );
    let (kind, payload) = event.kind_and_payload();
    debug_assert_eq!(kind, CLEAR_KIND);
    match state
        .db
        .queue_chat_clear(user_id, except_device, &chat.chat_id, &kind, &payload)
    {
        Ok(devices) => {
            if !devices.is_empty() {
                tracing::debug!(
                    user = %user_id,
                    chat = %chat.chat_id,
                    devices = devices.len(),
                    badge,
                    "clear-on-read queued"
                );
            }
            devices
        }
        Err(e) => {
            tracing::warn!(user = %user_id, chat = %chat.chat_id, error = %e, "clear-on-read not queued");
            Vec::new()
        }
    }
}

/// A clear-on-read push stays worth sending about as long as a badge count.
pub const CLEAR_TTL_MS: i64 = BADGE_TTL_MS;

/// The APNs form of [`clear_on_read`]: one background push (`PushKind::Clear`, `type: clear`,
/// `chat_id`, `badge`) per other device with a token, so a phone that is not running the app
/// drops the chat's banners too. `read_upto` (the row the reader got to) is the kind's
/// `upto_message_id`: it makes a later read of the same chat a new push instead of a dedupe
/// hit, while the same cursor re-sent (the apps re-PUT on foreground) pushes once. Nothing
/// is queued while no provider can reach a device.
fn silent_clear(
    state: &AppState,
    user_id: &str,
    except_device: &str,
    chat: &ChatRef,
    read_upto: &str,
    badge: i64,
) {
    if !state.push.is_on() {
        return;
    }
    let kind = PushKind::Clear {
        chat_id: chat.chat_id.clone(),
        upto_message_id: Some(read_upto.to_owned()),
    };
    let pushes: Vec<NewPush> = state
        .db
        .push_tokens_of_user(user_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.device_id != except_device)
        .map(|t| NewPush {
            user_id: t.user_id.clone(),
            device_id: t.device_id.clone(),
            collapse_id: Some(kind.collapse_id(user_id)),
            kind: kind.clone(),
            payload: payload::clear(&chat.chat_id, Some(read_upto), badge),
            ttl_ms: CLEAR_TTL_MS,
        })
        .collect();
    state.push.enqueue(pushes);
}

/// The user read something somewhere: every other device gets the new badge count.
pub async fn badge_refresh(state: &AppState, user_id: &str, except_device: &str) {
    if !state.push.is_on() {
        return;
    }
    let total = crate::matrix::reads::unread_total(state, user_id).await;
    let kind = PushKind::Badge { total };
    let pushes: Vec<NewPush> = state
        .db
        .push_tokens_of_user(user_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.device_id != except_device)
        .map(|t| NewPush {
            user_id: t.user_id.clone(),
            device_id: t.device_id.clone(),
            collapse_id: Some(kind.collapse_id(user_id)),
            kind: kind.clone(),
            payload: payload::badge_only(total),
            ttl_ms: BADGE_TTL_MS,
        })
        .collect();
    state.push.enqueue(pushes);
}

/// A device pushed a row into a chat without announcing a message (older apps). Wait a moment
/// for the announcement; if none comes, notify the others with the sender's name only, at most
/// once a minute per recipient and chat, and count it as one unread message.
pub fn on_row_from_device(state: &AppState, chat: ChatRef, user_id: &str, seq: i64) {
    if !state.push.is_on() {
        return;
    }
    let state = state.clone();
    let user_id = user_id.to_owned();
    let at = now_ms();
    tokio::spawn(async move {
        tokio::time::sleep(FALLBACK_DELAY).await;
        if state
            .db
            .chat_event_since(&chat.chat_id, &user_id, at - 1_000)
            .unwrap_or(true)
        {
            return;
        }
        let key = (chat.chat_id.clone(), user_id.clone());
        {
            let mut last = state.fallback_sent.lock().await;
            if last.get(&key).is_some_and(|t| at - t < FALLBACK_EVERY_MS) {
                return;
            }
            last.insert(key, at);
        }
        let sender = Sender::user(&state, &user_id);
        announce(&state, &chat, &sender, &format!("row:{seq}"), "New message").await;
    });
}

// ---- handlers -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterBody {
    pub platform: String,
    pub token: String,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
}

fn own_device(principal: &Principal, id: &str) -> Result<String, Box<Response>> {
    match principal {
        Principal::Device {
            id: mine, user_id, ..
        } if mine == id => Ok(user_id.clone()),
        Principal::Device { .. } => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a device registers only its own token",
        ))),
        Principal::Box { .. } | Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "devices only",
        ))),
    }
}

/// `POST /devices/{id}/push {platform: "ios", token, environment, bundle_id, app_version}`
pub async fn register_push(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<RegisterBody>,
) -> Response {
    let user_id = match own_device(&principal, &id) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if b.platform != "ios" {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "platform must be \"ios\"",
        );
    }
    let token = b.token.trim().to_ascii_lowercase();
    if token.is_empty() || token.len() > 512 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "token must be the hex APNs device token",
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
    let t = PushToken {
        device_id: id,
        user_id,
        platform: b.platform,
        token,
        environment: environment.to_owned(),
        bundle_id: b
            .bundle_id
            .unwrap_or_else(|| crate::apns::DEFAULT_TOPIC.to_owned()),
        app_version: b.app_version.unwrap_or_default(),
        updated_at: now_ms(),
    };
    match state.db.upsert_push_token(&t) {
        Ok(()) => Json(PushRegisterResponseDto {
            device_id: t.device_id,
            environment: t.environment,
            push: state.push.is_on(),
        })
        .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `DELETE /devices/{id}/push` — sign-out; the token is never pushed to again.
pub async fn unregister_push(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = own_device(&principal, &id) {
        return *r;
    }
    match state.db.delete_push_token(&id) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `GET /me/unread` → `{chats: [{chat_id, project_id, unread}], total}`
pub async fn me_unread(State(state): State<AppState>, principal: Principal) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    // Stage B: flipped users are answered from the homeserver (relay counts on a miss).
    let flipped = if crate::matrix::reads::reads_on(&state, user_id) {
        crate::matrix::reads::unread_for_user(&state, user_id).await
    } else {
        None
    };
    match flipped
        .map(Ok)
        .unwrap_or_else(|| state.db.unread_for_user(user_id))
    {
        Ok(chats) => {
            let total: i64 = chats.iter().map(|c| c.unread).sum();
            Json(UnreadDto {
                chats: chats
                    .iter()
                    .map(|c| UnreadChatDto {
                        chat_id: c.chat_id.clone(),
                        project_id: c
                            .project_id
                            .clone()
                            .unwrap_or_else(|| people::PEOPLE_PROJECT.to_owned()),
                        unread: c.unread,
                    })
                    .collect(),
                total,
            })
            .into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct NotificationsBody {
    /// `all` | `mentions` | `none` — the level every chat without its own setting uses.
    pub level: String,
}

fn notifications_json(state: &AppState, user_id: &str) -> Response {
    let level = state
        .db
        .user_notify_default(user_id)
        .unwrap_or(eligibility::Level::All);
    Json(json!({
        "level": level.as_str(),
        "large_group_min": eligibility::large_group_min(),
    }))
    .into_response()
}

/// `GET /me/notifications` → `{level, large_group_min}`: the account-wide default.
pub async fn me_notifications(State(state): State<AppState>, principal: Principal) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    notifications_json(&state, user_id)
}

/// `PUT /me/notifications {level}` — set the account-wide default (`inherit` = back to `all`).
pub async fn put_me_notifications(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<NotificationsBody>,
) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    let Some(level) = eligibility::Level::parse(&b.level) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "level must be all, mentions or none",
        );
    };
    if let Err(e) = state.db.set_user_notify_default(user_id, level) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    notifications_json(&state, user_id)
}

#[derive(Deserialize)]
pub struct NotifyBody {
    pub message_id: String,
    #[serde(default)]
    pub text: String,
    /// People chats, box callers: which of the box's agents spoke.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Reply in DM: the message this one answers in another chat, with its snapshot
    /// (crdt-sync §6.2 `replyRef`). Rides on the push so the banner reads
    /// "↩ Jacob in Arbonites: Testing reply in dm" and the tap can land there.
    #[serde(default)]
    pub reply_ref: Option<ReplyRefBody>,
    /// Box callers (#1832): what kind of bubble this is for a live call in the chat —
    /// `result` (a turn's answer), `job` (a background job's outcome), `quick` (a status line
    /// while a turn runs). Devices send none (a person's message).
    #[serde(default)]
    pub kind: Option<String>,
    /// Box callers: the whole prose of the turn for the call's context (longer than the push
    /// body), when it differs from `text`.
    #[serde(default)]
    pub voice_text: Option<String>,
    /// Who the message addresses (`user:<id>` or `@handle`), for mentions-only recipients.
    /// `@handle` in `text` counts too, so older writers need not send it.
    #[serde(default)]
    pub mentions: Vec<String>,
}

/// The cross-chat reference a client announces with its message.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ReplyRefBody {
    pub chat_id: String,
    /// `"people"` for DMs and people groups, else the project id.
    #[serde(default)]
    pub project_id: String,
    pub message_id: String,
    #[serde(default)]
    pub chat_title: String,
    #[serde(default)]
    pub sender_name: String,
    #[serde(default)]
    pub text: String,
}

impl ReplyRefBody {
    /// Where the quoted message lives, as a [`ChatRef`].
    fn source(&self) -> ChatRef {
        if self.project_id.is_empty() || self.project_id == "people" {
            ChatRef::people(&self.chat_id)
        } else {
            ChatRef::project(&self.project_id, &self.chat_id)
        }
    }

    /// The payload a recipient gets: the snapshot always; the address (chat,
    /// project, message id — what a tap opens) only when they are a member of
    /// the source chat. A non-member sees the quote and nothing to follow.
    pub fn for_recipient(&self, state: &AppState, user_id: &str) -> Value {
        let mut v = json!({
            "chat_title": self.chat_title,
            "sender_name": self.sender_name,
            "text": brief(&self.text),
        });
        if recipients(state, &self.source())
            .iter()
            .any(|u| u == user_id)
        {
            v["chat_id"] = json!(self.chat_id);
            v["project_id"] = json!(if self.project_id.is_empty() {
                "people"
            } else {
                self.project_id.as_str()
            });
            v["message_id"] = json!(self.message_id);
        }
        v
    }
}

/// The sender behind a notify call: a device is its person; a box is one of its owner's agents.
fn sender_for(
    state: &AppState,
    principal: &Principal,
    project_id: Option<&str>,
) -> Result<Sender, Box<Response>> {
    match principal {
        Principal::Device { user_id, .. } => Ok(Sender::user(state, user_id)),
        Principal::Box { user_id } => {
            let Some(pid) = project_id else {
                return Err(Box::new(api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "project_id (the agent) is required",
                )));
            };
            match state.db.project(pid) {
                Ok(Some(p)) if &p.user_id == user_id => Ok(Sender::agent(state, &p)),
                Ok(_) => Err(Box::new(api_error(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "not this box's agent",
                ))),
                Err(e) => Err(Box::new(api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "db",
                    e.to_string(),
                ))),
            }
        }
        Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "devices and boxes only",
        ))),
    }
}

fn notify_response(event_id: Option<i64>) -> Response {
    Json(json!({ "recorded": event_id.is_some(), "event_id": event_id })).into_response()
}

/// `POST /chats/{id}/notify {message_id, text, project_id?}` — a whole message landed in a
/// people chat (member devices; boxes whose agent is in the chat).
pub async fn notify_people_chat(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<NotifyBody>,
) -> Response {
    if !state.allows(&principal, &format!("{}{id}", people::PEOPLE_ACL_PREFIX)) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not in this chat");
    }
    // The same gate the room push runs (#2012): a person a DM partner blocks, or an admin
    // silenced, announces nothing — no fan-out, no Matrix mirror. Boxes (agents) pass.
    if let Err(r) = people::sender_gate(&state, &principal, &id) {
        return api_error(StatusCode::FORBIDDEN, r.code(), r.message());
    }
    let sender = match sender_for(&state, &principal, b.project_id.as_deref()) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let chat = ChatRef::people(&id);
    let out = announce_with(
        &state,
        &chat,
        &sender,
        &b.message_id,
        &b.text,
        b.reply_ref.as_ref(),
        &b.mentions,
    )
    .await;
    if out.is_some() {
        crate::voice::call_update(&state, &chat, &sender, &b).await;
    }
    notify_response(out)
}

/// `POST /projects/{project_id}/chats/{chat_id}/notify {message_id, text}` — a whole message
/// landed in a project chat (owner and guest devices; the owner's box for its agent).
pub async fn notify_project_chat(
    State(state): State<AppState>,
    principal: Principal,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(b): Json<NotifyBody>,
) -> Response {
    if !state.allows(&principal, &project_id) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "not your project");
    }
    let Ok(Some(project)) = state.db.project(&project_id) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such project");
    };
    if !state.db.chat_exists(&project.id, &chat_id).unwrap_or(false) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
    }
    let sender = match sender_for(&state, &principal, Some(&project.id)) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let chat = ChatRef::project(&project.id, &chat_id);
    let out = announce_with(
        &state,
        &chat,
        &sender,
        &b.message_id,
        &b.text,
        b.reply_ref.as_ref(),
        &b.mentions,
    )
    .await;
    if out.is_some() {
        crate::voice::call_update(&state, &chat, &sender, &b).await;
    }
    notify_response(out)
}

#[cfg(test)]
mod tests {
    //! Invariants: no push to the sender's own devices; none to a device focused on the chat;
    //! the badge is the recipient's total unread and drops when they read elsewhere.

    use arbos_core::ids::ulid;

    use super::*;
    use crate::db::Project;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn device_id(p: &Principal) -> String {
        match p {
            Principal::Device { id, .. } => id.clone(),
            Principal::Box { .. } | Principal::Admin => unreachable!("device principal"),
        }
    }

    /// Work the outbox, then everything the provider was handed so far.
    async fn pushed(state: &AppState) -> Vec<crate::push::Outgoing> {
        state.push.drain().await;
        state.push.captured().await
    }

    fn register(state: &AppState, p: &Principal, user: &str) -> String {
        let tok: String = ulid()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        let tok = format!("{tok:0<64}");
        state
            .db
            .upsert_push_token(&PushToken {
                device_id: device_id(p),
                user_id: user.to_owned(),
                platform: "ios".into(),
                token: tok.clone(),
                environment: "development".into(),
                bundle_id: crate::apns::DEFAULT_TOPIC.into(),
                app_version: "1".into(),
                updated_at: now_ms(),
            })
            .expect("token");
        tok
    }

    /// A second signed-in device for the same person.
    fn second_device(state: &AppState, user: &str) -> Principal {
        let id = ulid();
        state
            .db
            .insert_device(&id, "d2", &ulid(), user, "github")
            .expect("device");
        Principal::Device {
            id,
            token: String::new(),
            user_id: user.to_owned(),
        }
    }

    #[tokio::test]
    async fn sender_devices_are_never_pushed_and_focus_suppresses() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let bob_dev2 = second_device(&state, &bob);
        let chat = dm(&state, &alice, &bob);
        let alice_tok = register(&state, &alice_dev, &alice);
        let bob_tok = register(&state, &bob_dev, &bob);
        let bob_tok2 = register(&state, &bob_dev2, &bob);

        let sender = Sender::user(&state, &alice);
        let ev = announce(&state, &ChatRef::people(&chat), &sender, "m1", "hello bob").await;
        assert!(ev.is_some());
        let sent = pushed(&state).await;
        let tokens: Vec<&str> = sent.iter().map(|o| o.device_token.as_str()).collect();
        assert!(
            !tokens.contains(&alice_tok.as_str()),
            "sender's own device is silent"
        );
        assert!(tokens.contains(&bob_tok.as_str()));
        assert!(tokens.contains(&bob_tok2.as_str()));
        assert_eq!(sent.len(), 2);
        let p = &sent[0].payload;
        assert_eq!(p["aps"]["alert"]["title"], "@user1");
        assert_eq!(p["aps"]["alert"]["body"], "hello bob");
        assert_eq!(p["aps"]["thread-id"], chat);
        assert_eq!(p["aps"]["category"], CATEGORY);
        assert_eq!(p["aps"]["mutable-content"], 1);
        assert_eq!(p["aps"]["badge"], 1);
        assert_eq!(p["project_id"], "people");
        assert_eq!(p["sender"]["kind"], "user");
        assert_eq!(p["sender"]["handle"], "user1");

        // The same message announced again is not a second notification.
        assert!(
            announce(&state, &ChatRef::people(&chat), &sender, "m1", "hello bob")
                .await
                .is_none()
        );
        assert_eq!(pushed(&state).await.len(), 2);

        // Bob's phone is looking at the chat: only his other device hears the next one,
        // and the event is marked read (he is seeing it) so the badge does not stick.
        set_focus(&state, &device_id(&bob_dev), Some(&chat)).await;
        announce(
            &state,
            &ChatRef::people(&chat),
            &sender,
            "m2",
            "still there?",
        )
        .await;
        let sent = pushed(&state).await;
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[2].device_token, bob_tok2);
        assert_eq!(sent[2].payload["aps"]["badge"], 0);
        assert_eq!(state.db.unread_total(&bob).expect("bob unread"), 0);

        // Focus on a different chat does not suppress.
        set_focus(&state, &device_id(&bob_dev), Some("other-chat")).await;
        announce(&state, &ChatRef::people(&chat), &sender, "m3", "?").await;
        assert_eq!(pushed(&state).await.len(), 5);
    }

    /// Reply in DM (reply-in-dm-context-2026-10-25): the DM's push carries the quoted
    /// message's snapshot for everyone, the body says what it answers, and the address
    /// (chat / message id — what a tap opens) only for a recipient who is a member of
    /// the source chat. A non-member gets the quote and nothing to follow.
    #[tokio::test]
    async fn dm_reply_push_carries_the_quote_and_links_only_members() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (carol, carol_dev) = person(&state, 3);
        // The source: a group of alice + bob (carol is not in it).
        let group = dm(&state, &alice, &bob);
        // Two DMs alice replies in: with bob (member of the source) and carol (not).
        let dm_bob = dm(&state, &alice, &bob);
        let dm_carol = dm(&state, &alice, &carol);
        let bob_tok = register(&state, &bob_dev, &bob);
        let carol_tok = register(&state, &carol_dev, &carol);
        let sender = Sender::user(&state, &alice);
        let reply_ref = ReplyRefBody {
            chat_id: group.clone(),
            project_id: "people".into(),
            message_id: "orig-1".into(),
            chat_title: "Arbonites".into(),
            sender_name: "Jacob".into(),
            text: "Testing reply in dm".into(),
        };

        announce_with(
            &state,
            &ChatRef::people(&dm_bob),
            &sender,
            "r1",
            "Works",
            Some(&reply_ref),
            &[],
        )
        .await
        .expect("recorded");
        announce_with(
            &state,
            &ChatRef::people(&dm_carol),
            &sender,
            "r2",
            "Works",
            Some(&reply_ref),
            &[],
        )
        .await
        .expect("recorded");

        let sent = pushed(&state).await;
        let to_bob = sent
            .iter()
            .find(|o| o.device_token == bob_tok)
            .expect("bob's push");
        let to_carol = sent
            .iter()
            .find(|o| o.device_token == carol_tok)
            .expect("carol's push");
        for p in [&to_bob.payload, &to_carol.payload] {
            assert_eq!(p["aps"]["alert"]["body"], "↩ Jacob in Arbonites: Works");
            assert_eq!(p["reply_ref"]["text"], "Testing reply in dm");
            assert_eq!(p["reply_ref"]["chat_title"], "Arbonites");
            assert_eq!(p["reply_ref"]["sender_name"], "Jacob");
        }
        // Bob is in the source group: his payload can open the original.
        assert_eq!(to_bob.payload["reply_ref"]["chat_id"], group);
        assert_eq!(to_bob.payload["reply_ref"]["project_id"], "people");
        assert_eq!(to_bob.payload["reply_ref"]["message_id"], "orig-1");
        // Carol is not: quote only, no address to follow.
        assert!(to_carol.payload["reply_ref"].get("chat_id").is_none());
        assert!(to_carol.payload["reply_ref"].get("message_id").is_none());
        // A plain announcement carries no reply_ref at all.
        announce(&state, &ChatRef::people(&dm_bob), &sender, "r3", "plain").await;
        let last = pushed(&state).await.into_iter().last().unwrap();
        assert!(last.payload.get("reply_ref").is_none());
        assert_eq!(last.payload["aps"]["alert"]["body"], "plain");
    }

    #[tokio::test]
    async fn badge_is_total_unread_and_read_elsewhere_pushes_the_new_count() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let bob_dev2 = second_device(&state, &bob);
        let dm_chat = dm(&state, &alice, &bob);
        let project_id = project_of(&state, &alice, "Research");
        state
            .db
            .add_project_member(&project_id, &bob, &alice)
            .expect("guest");
        let chat_id = ulid();
        state
            .db
            .insert_chat(&crate::db::Chat {
                id: chat_id.clone(),
                project_id: project_id.clone(),
                title: "Plan".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .expect("chat");
        let bob_tok = register(&state, &bob_dev, &bob);
        let bob_tok2 = register(&state, &bob_dev2, &bob);

        let alice_s = Sender::user(&state, &alice);
        announce(&state, &ChatRef::people(&dm_chat), &alice_s, "d1", "one").await;
        announce(&state, &ChatRef::people(&dm_chat), &alice_s, "d2", "two").await;
        let project: Project = state.db.project(&project_id).expect("p").expect("p");
        let agent = Sender::agent(&state, &project);
        announce(
            &state,
            &ChatRef::project(&project_id, &chat_id),
            &agent,
            "a1",
            "Here is the plan.",
        )
        .await;

        let unread = state.db.unread_for_user(&bob).expect("unread");
        let total: i64 = unread.iter().map(|u| u.unread).sum();
        assert_eq!(total, 3);
        assert_eq!(
            unread
                .iter()
                .find(|u| u.chat_id == dm_chat)
                .map(|u| u.unread),
            Some(2)
        );
        assert_eq!(
            unread
                .iter()
                .find(|u| u.chat_id == chat_id)
                .map(|u| u.unread),
            Some(1)
        );
        // Alice wrote two of them and is the agent's owner: nothing unread for her from the DM,
        // but the agent's reply counts (it is not her message).
        assert_eq!(state.db.unread_total(&alice).expect("alice"), 1);

        let sent = pushed(&state).await;
        let last = sent.last().expect("agent push");
        assert_eq!(last.payload["aps"]["badge"], 3);
        assert_eq!(last.payload["aps"]["alert"]["title"], "Research");
        assert_eq!(last.payload["sender"]["kind"], "agent");
        assert_eq!(last.payload["chat_title"], "Research");
        assert_eq!(last.payload["project_id"], project_id);
        assert!(last.payload["sender"]["avatar"].is_string());
        assert!(last.payload["sender"]["avatar_color"].is_string());

        // Bob reads the DM on his phone: the other device gets badge 1, the phone nothing.
        let before = sent.len();
        state.db.mark_chat_read(&bob, &dm_chat).expect("read");
        badge_refresh(&state, &bob, &device_id(&bob_dev)).await;
        let sent = pushed(&state).await;
        assert_eq!(sent.len(), before + 1);
        let badge = sent.last().expect("badge push");
        assert_eq!(badge.device_token, bob_tok2);
        assert_ne!(badge.device_token, bob_tok);
        assert_eq!(badge.payload, json!({ "aps": { "badge": 1 } }));
        assert_eq!(state.db.unread_total(&bob).expect("total"), 1);
    }

    /// Clear on read: reading a chat on one device queues one `notifications_cleared` row for
    /// each other device of the same user — never the reader's own, never another person's —
    /// with the chat and the unread total after the read; an undelivered `people_message` for
    /// that chat on those devices is dropped with it; reads before the device drains keep one
    /// pending clear per chat, carrying the newest badge.
    #[tokio::test]
    async fn reading_on_one_device_clears_the_chat_on_the_others() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_phone) = person(&state, 2);
        let bob_mac = second_device(&state, &bob);
        let dm_chat = dm(&state, &alice, &bob);
        let other = dm(&state, &alice, &bob);
        let alice_s = Sender::user(&state, &alice);

        // Alice writes in both chats; her row in the DM queues the in-app banner for Bob.
        assert!(crate::people::on_room_push(&state, &alice_dev, &dm_chat).is_ok());
        announce(&state, &ChatRef::people(&dm_chat), &alice_s, "d1", "one").await;
        announce(&state, &ChatRef::people(&other), &alice_s, "o1", "two").await;
        assert_eq!(state.db.unread_total(&bob).expect("unread"), 2);

        let read = |dev: Principal, chat: String, seq: i64| {
            let state = state.clone();
            async move {
                let r = crate::people::chat_read(
                    State(state),
                    dev,
                    Path(chat),
                    Json(crate::people::ReadBody { seq }),
                )
                .await;
                assert_eq!(r.status(), StatusCode::NO_CONTENT);
            }
        };
        // Bob reads the DM on his phone, then the other chat, then the DM again — all before
        // his Mac polls.
        read(bob_phone.clone(), dm_chat.clone(), 1).await;
        read(bob_phone.clone(), other.clone(), 1).await;
        read(bob_phone.clone(), dm_chat.clone(), 2).await;
        assert_eq!(state.db.unread_total(&bob).expect("unread"), 0);

        let mac = state
            .db
            .take_user_pushes(&device_id(&bob_mac))
            .expect("mac");
        assert!(
            mac.iter()
                .all(|(_, k, p, _)| !(k == "people_message" && p["chat_id"] == dm_chat)),
            "the Mac never posts a banner for a message Bob already read: {mac:?}"
        );
        let clears: Vec<&Value> = mac
            .iter()
            .filter(|(_, k, _, _)| k == CLEAR_KIND)
            .map(|(_, _, p, _)| p)
            .collect();
        assert_eq!(clears.len(), 2, "one pending clear per chat: {clears:?}");
        let for_dm = clears.iter().find(|p| p["chat_id"] == dm_chat).expect("dm");
        let for_other = clears
            .iter()
            .find(|p| p["chat_id"] == other)
            .expect("other");
        assert_eq!(for_dm["project_id"], "people");
        assert_eq!(
            for_dm["badge"], 0,
            "the newest badge wins over the first read's 1"
        );
        assert_eq!(for_other["badge"], 0);

        // The reading device already knows; Alice is not told about Bob's reads.
        let phone = state
            .db
            .take_user_pushes(&device_id(&bob_phone))
            .expect("phone");
        assert!(phone.iter().any(|(_, k, _, _)| k == "people_message"));
        assert!(phone.iter().all(|(_, k, _, _)| k != CLEAR_KIND));
        let alice_rows = state
            .db
            .take_user_pushes(&device_id(&alice_dev))
            .expect("alice");
        assert!(alice_rows.iter().all(|(_, k, _, _)| k != CLEAR_KIND));

        // The Mac has drained: the next read queues a fresh clear with the badge of that moment.
        announce(&state, &ChatRef::people(&other), &alice_s, "o2", "three").await;
        announce(&state, &ChatRef::people(&dm_chat), &alice_s, "d2", "four").await;
        read(bob_phone.clone(), dm_chat.clone(), 3).await;
        let mac = state
            .db
            .take_user_pushes(&device_id(&bob_mac))
            .expect("mac");
        let clears: Vec<&Value> = mac
            .iter()
            .filter(|(_, k, _, _)| k == CLEAR_KIND)
            .map(|(_, _, p, _)| p)
            .collect();
        assert_eq!(clears.len(), 1);
        assert_eq!(clears[0]["chat_id"], dm_chat);
        assert_eq!(clears[0]["badge"], 1, "the other chat is still unread");

        // A project chat (agent reply, Bob as guest) clears the same way, labelled with the
        // project id.
        let project_id = project_of(&state, &alice, "Research");
        state
            .db
            .add_project_member(&project_id, &bob, &alice)
            .expect("guest");
        let chat_id = ulid();
        state
            .db
            .insert_chat(&crate::db::Chat {
                id: chat_id.clone(),
                project_id: project_id.clone(),
                title: "Plan".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .expect("chat");
        let project: Project = state.db.project(&project_id).expect("p").expect("p");
        let agent = Sender::agent(&state, &project);
        announce(
            &state,
            &ChatRef::project(&project_id, &chat_id),
            &agent,
            "a1",
            "Here is the plan.",
        )
        .await;
        read(bob_mac.clone(), chat_id.clone(), 1).await;
        let phone = state
            .db
            .take_user_pushes(&device_id(&bob_phone))
            .expect("phone");
        let clear = phone
            .iter()
            .find(|(_, k, _, _)| k == CLEAR_KIND)
            .map(|(_, _, p, _)| p)
            .expect("project clear");
        assert_eq!(clear["chat_id"], chat_id);
        assert_eq!(clear["project_id"], project_id);
        assert_eq!(clear["badge"], 1, "the DM's d2 is still unread");
        assert!(
            phone
                .iter()
                .all(|(_, k, p, _)| !(k == "people_message" && p["chat_id"] == chat_id)),
            "the agent's in-app banner for the read chat is dropped too"
        );
    }

    /// The APNs form of the clear: a background `type: clear` push per other device with a
    /// token (never the reader's), collapse id `clear:<chat>`, carrying the chat and the badge;
    /// the same read cursor re-sent is one push, a later read of the same chat is a new one.
    #[tokio::test]
    async fn a_read_queues_one_silent_clear_push_per_other_device() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 1);
        let (bob, bob_phone) = person(&state, 2);
        let bob_pad = second_device(&state, &bob);
        let dm_chat = dm(&state, &alice, &bob);
        let other = dm(&state, &alice, &bob);
        let phone_tok = register(&state, &bob_phone, &bob);
        let pad_tok = register(&state, &bob_pad, &bob);
        let alice_s = Sender::user(&state, &alice);
        announce(&state, &ChatRef::people(&dm_chat), &alice_s, "d1", "one").await;
        announce(&state, &ChatRef::people(&other), &alice_s, "o1", "two").await;
        let before = pushed(&state).await.len();

        let read = |dev: Principal, seq: i64| {
            let state = state.clone();
            let chat = dm_chat.clone();
            async move {
                let r = crate::people::chat_read(
                    State(state),
                    dev,
                    Path(chat),
                    Json(crate::people::ReadBody { seq }),
                )
                .await;
                assert_eq!(r.status(), StatusCode::NO_CONTENT);
            }
        };
        read(bob_phone.clone(), 1).await;
        read(bob_phone.clone(), 1).await; // the app re-PUTs the same cursor on foreground
        let sent = pushed(&state).await;
        let clears: Vec<_> = sent[before..]
            .iter()
            .filter(|o| o.payload["type"] == "clear")
            .collect();
        assert_eq!(clears.len(), 1, "one silent clear, to the pad: {clears:?}");
        let c = clears[0];
        assert_eq!(c.device_token, pad_tok);
        assert_ne!(c.device_token, phone_tok);
        assert_eq!(c.push_type, "background");
        assert_eq!(
            c.collapse_id.as_deref(),
            Some(format!("clear:{dm_chat}").as_str())
        );
        assert_eq!(c.payload["aps"]["content-available"], 1);
        assert_eq!(c.payload["chat_id"], dm_chat);
        assert_eq!(c.payload["badge"], 1, "the other chat is still unread");
        assert_eq!(c.payload["upto_message_id"], "seq:1");

        // Further along in the same chat: a new push, not a dedupe hit.
        announce(&state, &ChatRef::people(&dm_chat), &alice_s, "d2", "three").await;
        read(bob_phone.clone(), 2).await;
        let sent = pushed(&state).await;
        let clears: Vec<_> = sent[before..]
            .iter()
            .filter(|o| o.payload["type"] == "clear")
            .collect();
        assert_eq!(clears.len(), 2, "{clears:?}");
        assert_eq!(clears[1].payload["upto_message_id"], "seq:2");
        assert_eq!(clears[1].payload["badge"], 1);
        assert_eq!(clears[1].device_token, pad_tok);
    }

    #[test]
    fn brief_caps_at_140_chars_on_one_line() {
        assert_eq!(brief("  two\nlines  here "), "two lines here");
        let long = "x".repeat(300);
        let b = brief(&long);
        assert_eq!(b.chars().count(), BODY_MAX);
        assert!(b.ends_with('…'));
    }
}

#[cfg(test)]
mod archive_tests {
    //! D78: archive hides only — per user across devices, other devices are told, archived chats
    //! neither badge nor alert, unarchive brings everything back.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use arbos_core::ids::ulid;

    use super::*;
    use crate::db_push::PushToken;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn dev_id(p: &Principal) -> String {
        match p {
            Principal::Device { id, .. } => id.clone(),
            Principal::Box { .. } | Principal::Admin => unreachable!(),
        }
    }

    async fn pushed(state: &AppState) -> Vec<crate::push::Outgoing> {
        state.push.drain().await;
        state.push.captured().await
    }

    fn second_device(state: &AppState, user: &str) -> Principal {
        let id = ulid();
        state
            .db
            .insert_device(&id, "d2", &ulid(), user, "github")
            .unwrap();
        Principal::Device {
            id,
            token: String::new(),
            user_id: user.to_owned(),
        }
    }

    async fn body(r: Response) -> Value {
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&b).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn archive_is_per_user_hides_only_and_silences() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let bob_dev2 = second_device(&state, &bob);
        let chat = dm(&state, &alice, &bob);
        let project = project_of(&state, &alice, "Research");
        let pchat = ulid();
        state
            .db
            .insert_chat(&crate::db::Chat {
                id: pchat.clone(),
                project_id: project.clone(),
                title: "Plan".into(),
                is_pinned: true,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        for (p, u) in [(&bob_dev, &bob), (&bob_dev2, &bob)] {
            state
                .db
                .upsert_push_token(&PushToken {
                    device_id: dev_id(p),
                    user_id: u.clone(),
                    platform: "ios".into(),
                    token: format!(
                        "{:0<64}",
                        dev_id(p)
                            .to_ascii_lowercase()
                            .chars()
                            .filter(|c| c.is_ascii_hexdigit())
                            .collect::<String>()
                    ),
                    environment: "development".into(),
                    bundle_id: crate::apns::DEFAULT_TOPIC.into(),
                    app_version: "1".into(),
                    updated_at: now_ms(),
                })
                .unwrap();
        }

        // Bob archives the DM from his phone: the response says so, his Mac is told, Alice is not.
        let r = crate::people::chat_settings(
            State(state.clone()),
            bob_dev.clone(),
            Path(chat.clone()),
            Json(crate::people::ChatSettingsBody {
                read_receipts: None,
                muted: None,
                archived: Some(true),
                notifications: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let v = body(r).await;
        assert!(v["archived_at"].is_i64());
        assert_eq!(v["me"]["archived_at"], v["archived_at"]);
        let mac: Vec<_> = state.db.take_user_pushes(&dev_id(&bob_dev2)).unwrap();
        assert_eq!(mac.len(), 1);
        assert_eq!(mac[0].1, "chat_archived");
        assert_eq!(mac[0].2["chat_id"], chat);
        assert_eq!(mac[0].2["project_id"], "people");
        assert!(mac[0].2["archived_at"].is_i64());
        assert!(
            state
                .db
                .take_user_pushes(&dev_id(&bob_dev))
                .unwrap()
                .is_empty(),
            "the archiving device already knows"
        );
        assert!(
            state
                .db
                .take_user_pushes(&dev_id(&alice_dev))
                .unwrap()
                .is_empty(),
            "archive is private"
        );
        // Alice's own view of the chat is not archived.
        let cm = crate::db_people::ChatWithMembers {
            chat: state.db.people_chat(&chat).unwrap().unwrap(),
            members: state.db.members_of(&chat).unwrap(),
        };
        assert!(
            crate::people::chat_json(&state, &cm, &alice, &dev_id(&alice_dev))["archived_at"]
                .is_null()
        );

        // A message from Alice: recorded, but Bob gets no alert and no unread/badge from it.
        announce(
            &state,
            &ChatRef::people(&chat),
            &Sender::user(&state, &alice),
            "m1",
            "hi",
        )
        .await;
        assert!(
            pushed(&state).await.is_empty(),
            "archived chats do not alert"
        );
        assert_eq!(
            state.db.unread_total(&bob).unwrap(),
            0,
            "archived chats do not badge"
        );
        // people_message is not queued for an archived chat either.
        let _ = crate::people::on_room_push(&state, &alice_dev, &chat);
        assert!(state
            .db
            .take_user_pushes(&dev_id(&bob_dev))
            .unwrap()
            .iter()
            .all(|(_, k, _, _)| k != "people_message"));

        // The project chat form (plain chat, Alice's own project) works through the same endpoint.
        let r = crate::people::chat_settings(
            State(state.clone()),
            alice_dev.clone(),
            Path(pchat.clone()),
            Json(crate::people::ChatSettingsBody {
                read_receipts: None,
                muted: None,
                archived: Some(true),
                notifications: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let v = body(r).await;
        assert_eq!(v["kind"], "chat");
        assert!(v["archived_at"].is_i64());
        let listed = crate::api::chat_json_for(
            &state,
            &state.db.chat(&project, &pchat).unwrap().unwrap(),
            Some(&alice),
        );
        assert!(listed["archived_at"].is_i64());
        assert!(crate::api::chat_json_for(
            &state,
            &state.db.chat(&project, &pchat).unwrap().unwrap(),
            None
        )["archived_at"]
            .is_null());
        // A stranger cannot archive someone else's project chat.
        let r = crate::people::chat_settings(
            State(state.clone()),
            bob_dev.clone(),
            Path(pchat.clone()),
            Json(crate::people::ChatSettingsBody {
                read_receipts: None,
                muted: None,
                archived: Some(true),
                notifications: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Unarchive: alerts and unread return (the earlier message now counts).
        let r = crate::people::chat_settings(
            State(state.clone()),
            bob_dev.clone(),
            Path(chat.clone()),
            Json(crate::people::ChatSettingsBody {
                read_receipts: None,
                muted: None,
                archived: Some(false),
                notifications: None,
            }),
        )
        .await;
        assert!(body(r).await["archived_at"].is_null());
        assert_eq!(state.db.unread_total(&bob).unwrap(), 1);
        announce(
            &state,
            &ChatRef::people(&chat),
            &Sender::user(&state, &alice),
            "m2",
            "there?",
        )
        .await;
        assert_eq!(
            pushed(&state).await.len(),
            2,
            "both of Bob's devices alert again"
        );
    }
}

#[cfg(test)]
mod eligibility_tests {
    //! The eligibility matrix end to end: agent and group messages reach every device (the
    //! 2026-10-26 root cause), blocks hide for the blocker alone in groups and refuse the DM
    //! announcement (#2012), the per-chat level and the account default decide, and a large
    //! group is mentions-only until someone picks `all`.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use arbos_core::ids::ulid;

    use super::*;
    use crate::db::Project;
    use crate::people::tests_support::{dm, group, person, project_of};
    use crate::people::{chat_settings, ChatSettingsBody};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn dev_id(p: &Principal) -> String {
        match p {
            Principal::Device { id, .. } => id.clone(),
            Principal::Box { .. } | Principal::Admin => unreachable!(),
        }
    }

    fn register(state: &AppState, p: &Principal, user: &str) -> String {
        let tok = format!(
            "{:0<64}",
            ulid()
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .collect::<String>()
        );
        state
            .db
            .upsert_push_token(&PushToken {
                device_id: dev_id(p),
                user_id: user.to_owned(),
                platform: "ios".into(),
                token: tok.clone(),
                environment: "development".into(),
                bundle_id: crate::apns::DEFAULT_TOPIC.into(),
                app_version: "1".into(),
                updated_at: now_ms(),
            })
            .expect("token");
        tok
    }

    /// Drain the outbox and return the tokens pushed since the last call.
    async fn pushed_since(state: &AppState, seen: &mut usize) -> Vec<String> {
        state.push.drain().await;
        let all = state.push.captured().await;
        let fresh = all[*seen..]
            .iter()
            .map(|o| o.device_token.clone())
            .collect();
        *seen = all.len();
        fresh
    }

    fn inapp_kinds(state: &AppState, p: &Principal) -> Vec<String> {
        state
            .db
            .take_user_pushes(&dev_id(p))
            .unwrap()
            .into_iter()
            .map(|(_, k, _, _)| k)
            .collect()
    }

    async fn notify(state: &AppState, p: &Principal, chat: &str, text: &str) -> StatusCode {
        notify_people_chat(
            State(state.clone()),
            p.clone(),
            Path(chat.to_owned()),
            Json(NotifyBody {
                message_id: ulid(),
                text: text.into(),
                project_id: None,
                reply_ref: None,
                kind: None,
                voice_text: None,
                mentions: Vec::new(),
            }),
        )
        .await
        .status()
    }

    async fn set_level(state: &AppState, p: &Principal, chat: &str, level: &str) -> Value {
        let r = chat_settings(
            State(state.clone()),
            p.clone(),
            Path(chat.to_owned()),
            Json(ChatSettingsBody {
                read_receipts: None,
                muted: None,
                archived: None,
                notifications: Some(level.into()),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK, "level {level}");
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&b).unwrap()
    }

    #[tokio::test]
    async fn agent_and_group_messages_reach_every_member_device() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (carol, carol_dev) = person(&state, 3);
        let g = group(&state, &[&alice, &bob, &carol]);
        let alice_tok = register(&state, &alice_dev, &alice);
        let bob_tok = register(&state, &bob_dev, &bob);
        let carol_tok = register(&state, &carol_dev, &carol);
        let mut seen = 0;

        // A person's group message: both other members, never the sender.
        assert_eq!(
            notify(&state, &alice_dev, &g, "morning all").await,
            StatusCode::OK
        );
        let mut got = pushed_since(&state, &mut seen).await;
        got.sort();
        let mut want = vec![bob_tok.clone(), carol_tok.clone()];
        want.sort();
        assert_eq!(got, want);

        // An agent's reply in a group (the Arbonites case): every human member, APNs and the
        // in-app people_message both.
        let project_id = project_of(&state, &alice, "Doug");
        let project: Project = state.db.project(&project_id).unwrap().unwrap();
        state
            .db
            .add_member(
                &g,
                &format!("agent:{project_id}"),
                "agent",
                &alice,
                &json!({}),
            )
            .unwrap();
        let agent = Sender::agent(&state, &project);
        announce(
            &state,
            &ChatRef::people(&g),
            &agent,
            "a1",
            "Here you go @user2",
        )
        .await;
        let mut got = pushed_since(&state, &mut seen).await;
        got.sort();
        let mut want = vec![alice_tok.clone(), bob_tok.clone(), carol_tok.clone()];
        want.sort();
        assert_eq!(
            got, want,
            "the agent's owner hears it too: she did not write it"
        );
        for d in [&alice_dev, &bob_dev, &carol_dev] {
            assert!(inapp_kinds(&state, d).contains(&"people_message".to_owned()));
        }
        let last = state.push.captured().await.last().unwrap().payload.clone();
        assert_eq!(last["sender"]["kind"], "agent");
        assert_eq!(last["aps"]["thread-id"], g);
        assert_eq!(last["aps"]["mutable-content"], 1);

        // An agent in its own project chat with a guest: owner and guest.
        state
            .db
            .add_project_member(&project_id, &bob, &alice)
            .unwrap();
        let pchat = ulid();
        state
            .db
            .insert_chat(&crate::db::Chat {
                id: pchat.clone(),
                project_id: project_id.clone(),
                title: "Plan".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        announce(
            &state,
            &ChatRef::project(&project_id, &pchat),
            &agent,
            "a2",
            "Plan is ready",
        )
        .await;
        let mut got = pushed_since(&state, &mut seen).await;
        got.sort();
        let mut want = vec![alice_tok.clone(), bob_tok.clone()];
        want.sort();
        assert_eq!(got, want);
    }

    #[tokio::test]
    async fn blocks_hide_for_the_blocker_in_groups_and_refuse_the_dm_announcement() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (carol, carol_dev) = person(&state, 3);
        let g = group(&state, &[&alice, &bob, &carol]);
        let alice_tok = register(&state, &alice_dev, &alice);
        let bob_tok = register(&state, &bob_dev, &bob);
        let carol_tok = register(&state, &carol_dev, &carol);
        let mut seen = 0;
        // Bob blocks Alice.
        state.db.block(&bob, &alice).unwrap();

        // Group: Alice's announcement is accepted; Carol hears it, Bob does not (no APNs, no
        // in-app event, no unread), and Bob's own posts still reach Alice.
        assert_eq!(notify(&state, &alice_dev, &g, "hi").await, StatusCode::OK);
        assert_eq!(
            pushed_since(&state, &mut seen).await,
            vec![carol_tok.clone()]
        );
        assert!(inapp_kinds(&state, &bob_dev).is_empty());
        assert_eq!(state.db.unread_in_chat(&bob, &g).unwrap(), 0);
        assert_eq!(state.db.unread_in_chat(&carol, &g).unwrap(), 1);
        assert_eq!(
            notify(&state, &bob_dev, &g, "ignoring you").await,
            StatusCode::OK
        );
        let mut got = pushed_since(&state, &mut seen).await;
        got.sort();
        let mut want = vec![alice_tok.clone(), carol_tok.clone()];
        want.sort();
        assert_eq!(got, want);

        // DM (#2012): the announcement is refused like the row — not_available, no event, no
        // push, no Matrix mirror — in both directions; Carol's DM with Alice is untouched.
        let d = dm(&state, &alice, &bob);
        let r = notify_people_chat(
            State(state.clone()),
            alice_dev.clone(),
            Path(d.clone()),
            Json(NotifyBody {
                message_id: "blocked-1".into(),
                text: "hello?".into(),
                project_id: None,
                reply_ref: None,
                kind: None,
                voice_text: None,
                mentions: Vec::new(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["error"]["type"], "not_available");
        assert!(pushed_since(&state, &mut seen).await.is_empty());
        assert_eq!(state.db.unread_in_chat(&bob, &d).unwrap(), 0);
        assert!(
            !state.db.chat_event_since(&d, &alice, 0).unwrap(),
            "no chat_events row: the announcement never landed"
        );
        assert_eq!(
            notify(&state, &bob_dev, &d, "nor you").await,
            StatusCode::FORBIDDEN,
            "the blocker cannot announce into the DM either"
        );
        let d2 = dm(&state, &alice, &carol);
        assert_eq!(
            notify(&state, &alice_dev, &d2, "hi carol").await,
            StatusCode::OK
        );
        assert_eq!(
            pushed_since(&state, &mut seen).await,
            vec![carol_tok.clone()]
        );

        // Silenced in a group (#2012): the announcement is refused too; a box is never gated.
        state
            .db
            .set_member_silenced(&g, &format!("user:{carol}"), true, None)
            .unwrap();
        assert_eq!(
            notify(&state, &carol_dev, &g, "psst").await,
            StatusCode::FORBIDDEN
        );
        assert!(pushed_since(&state, &mut seen).await.is_empty());
        let _ = bob_tok;
    }

    #[tokio::test]
    async fn chat_level_and_account_default_decide() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (carol, carol_dev) = person(&state, 3);
        let g = group(&state, &[&alice, &bob, &carol]);
        register(&state, &alice_dev, &alice);
        let bob_tok = register(&state, &bob_dev, &bob);
        let carol_tok = register(&state, &carol_dev, &carol);
        let mut seen = 0;

        // Bob: none. Carol: mentions.
        let v = set_level(&state, &bob_dev, &g, "none").await;
        assert_eq!(v["notifications"], "none");
        let v = set_level(&state, &carol_dev, &g, "mentions").await;
        assert_eq!(v["notifications"], "mentions");
        assert_eq!(
            notify(&state, &alice_dev, &g, "plain words").await,
            StatusCode::OK
        );
        assert!(pushed_since(&state, &mut seen).await.is_empty());
        assert!(
            inapp_kinds(&state, &bob_dev).is_empty(),
            "none = no in-app banner either"
        );
        assert_eq!(
            notify(&state, &alice_dev, &g, "@user3 look at this").await,
            StatusCode::OK
        );
        assert_eq!(
            pushed_since(&state, &mut seen).await,
            vec![carol_tok.clone()],
            "a mention reaches Carol; Bob's none stays none"
        );
        // Declared mentions count without the text.
        notify_people_chat(
            State(state.clone()),
            alice_dev.clone(),
            Path(g.clone()),
            Json(NotifyBody {
                message_id: ulid(),
                text: "see attachment".into(),
                project_id: None,
                reply_ref: None,
                kind: None,
                voice_text: None,
                mentions: vec![format!("user:{carol}")],
            }),
        )
        .await;
        assert_eq!(
            pushed_since(&state, &mut seen).await,
            vec![carol_tok.clone()]
        );

        // Back to inherit: the account default (all) applies; then the default is set to none.
        let v = set_level(&state, &bob_dev, &g, "inherit").await;
        assert_eq!(v["notifications"], "inherit");
        assert_eq!(
            notify(&state, &alice_dev, &g, "again").await,
            StatusCode::OK
        );
        assert_eq!(pushed_since(&state, &mut seen).await, vec![bob_tok.clone()]);
        let r = put_me_notifications(
            State(state.clone()),
            bob_dev.clone(),
            Json(NotificationsBody {
                level: "none".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            notify(&state, &alice_dev, &g, "and again").await,
            StatusCode::OK
        );
        assert!(pushed_since(&state, &mut seen).await.is_empty());
        // An explicit `all` on the chat beats the account default.
        set_level(&state, &bob_dev, &g, "all").await;
        assert_eq!(
            notify(&state, &alice_dev, &g, "once more").await,
            StatusCode::OK
        );
        assert_eq!(pushed_since(&state, &mut seen).await, vec![bob_tok.clone()]);

        // The old mute flag is `none` while no level is chosen.
        set_level(&state, &bob_dev, &g, "inherit").await;
        let r = put_me_notifications(
            State(state.clone()),
            bob_dev.clone(),
            Json(NotificationsBody {
                level: "all".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        state
            .db
            .set_member_muted(&g, &format!("user:{bob}"), true)
            .unwrap();
        assert_eq!(
            notify(&state, &alice_dev, &g, "muted?").await,
            StatusCode::OK
        );
        assert!(pushed_since(&state, &mut seen).await.is_empty());
        // A bad word is refused.
        let r = chat_settings(
            State(state.clone()),
            bob_dev.clone(),
            Path(g.clone()),
            Json(ChatSettingsBody {
                read_receipts: None,
                muted: None,
                archived: None,
                notifications: Some("loud".into()),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_large_group_is_mentions_only_unless_all_is_chosen() {
        let state = test_state(AuthMode::Github);
        let n = eligibility::large_group_min();
        let people: Vec<(String, Principal)> = (1..=n as i64).map(|i| person(&state, i)).collect();
        let ids: Vec<&str> = people.iter().map(|(u, _)| u.as_str()).collect();
        let g = group(&state, &ids);
        let (alice, alice_dev) = &people[0];
        let (bob, bob_dev) = &people[1];
        let (_carol, carol_dev) = &people[2];
        let bob_tok = register(&state, bob_dev, bob);
        let carol_tok = register(&state, carol_dev, &people[2].0);
        let mut seen = 0;

        assert_eq!(
            notify(&state, alice_dev, &g, "hello crowd").await,
            StatusCode::OK
        );
        assert!(
            pushed_since(&state, &mut seen).await.is_empty(),
            "{n} humans: nobody is pushed without a mention"
        );
        assert_eq!(
            notify(&state, alice_dev, &g, "@user2 ping").await,
            StatusCode::OK
        );
        assert_eq!(pushed_since(&state, &mut seen).await, vec![bob_tok.clone()]);
        // Carol picks `all` for this chat: she hears everything again.
        set_level(&state, carol_dev, &g, "all").await;
        assert_eq!(
            notify(&state, alice_dev, &g, "hello again").await,
            StatusCode::OK
        );
        assert_eq!(
            pushed_since(&state, &mut seen).await,
            vec![carol_tok.clone()]
        );
        // One member leaves: below the threshold the group is an ordinary one.
        state
            .db
            .remove_member(&g, &format!("user:{}", people[n - 1].0), false)
            .unwrap();
        assert_eq!(
            notify(&state, alice_dev, &g, "smaller now").await,
            StatusCode::OK
        );
        let mut got = pushed_since(&state, &mut seen).await;
        got.sort();
        let mut want = vec![bob_tok, carol_tok];
        want.sort();
        assert_eq!(got, want);
        let _ = alice;
    }
}

#[cfg(test)]
mod chat_meta_tests {
    //! Group banners are titled with the chat as each recipient sees it; DMs with the sender.

    use arbos_core::ids::{now_ms, ulid};
    use serde_json::json;

    use super::*;
    use crate::db_people::PeopleChat;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn dev_id(p: &Principal) -> String {
        match p {
            Principal::Device { id, .. } => id.clone(),
            Principal::Box { .. } | Principal::Admin => unreachable!("device principal"),
        }
    }

    fn group(state: &AppState, members: &[&str], title: Option<&str>) -> String {
        let chat = PeopleChat {
            id: ulid(),
            kind: "group".into(),
            created_by: members[0].to_owned(),
            created_at: now_ms(),
            title: title.map(str::to_owned),
            image_key: Some("pic.png".into()),
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: None,
        };
        state.db.insert_people_chat(&chat).expect("chat");
        state
            .db
            .update_people_chat_profile(&chat.id, None, Some(Some("pic.png")), None, None)
            .expect("picture");
        for u in members {
            state
                .db
                .add_member(
                    &chat.id,
                    &format!("user:{u}"),
                    "member",
                    members[0],
                    &json!({}),
                )
                .expect("member");
        }
        chat.id
    }

    #[test]
    fn meta_per_kind_and_per_recipient() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 41);
        let (bob, _) = person(&state, 42);
        let (carol, _) = person(&state, 43);
        // DM: no title.
        let d = dm(&state, &alice, &bob);
        assert_eq!(
            chat_meta(&state, &ChatRef::people(&d), &bob),
            ChatMeta::dm()
        );
        // Named group: the name for everyone, with its picture.
        let g = group(&state, &[&alice, &bob, &carol], Some("Arbonites"));
        let m = chat_meta(&state, &ChatRef::people(&g), &bob);
        assert_eq!(m.kind, "group");
        assert_eq!(m.title.as_deref(), Some("Arbonites"));
        assert_eq!(m.image_key.as_deref(), Some("pic.png"));
        // Untitled group: the *other* members' handles, so it differs per recipient.
        let u = group(&state, &[&alice, &bob, &carol], None);
        let for_bob = chat_meta(&state, &ChatRef::people(&u), &bob);
        let for_carol = chat_meta(&state, &ChatRef::people(&u), &carol);
        assert_eq!(for_bob.title.as_deref(), Some("user41, user43"));
        assert_eq!(for_carol.title.as_deref(), Some("user41, user42"));
        // Own 1:1 agent chat: a DM with the agent; shared project chat: a group named after it.
        let pid = project_of(&state, &alice, "Research");
        assert_eq!(
            chat_meta(&state, &ChatRef::project(&pid, "c"), &alice),
            ChatMeta::dm()
        );
    }

    #[tokio::test]
    async fn agent_reply_in_a_group_queues_a_titled_in_app_push() {
        let state = test_state(AuthMode::Github);
        let (alice, _alice_dev) = person(&state, 51);
        let (bob, bob_dev) = person(&state, 52);
        let g = group(&state, &[&alice, &bob], Some("Arbonites"));
        let pid = project_of(&state, &alice, "Doug");
        let project = state.db.project(&pid).unwrap().unwrap();
        let agent = Sender::agent(&state, &project);
        announce(&state, &ChatRef::people(&g), &agent, "m1", "Five.").await;
        let pushes = state.db.take_user_pushes(&dev_id(&bob_dev)).unwrap();
        let pm: Vec<_> = pushes
            .iter()
            .filter(|(_, k, _, _)| k == "people_message")
            .collect();
        assert_eq!(pm.len(), 1, "{pushes:?}");
        let p = &pm[0].2;
        assert_eq!(p["chat_kind"], "group");
        assert_eq!(p["chat_title"], "Arbonites");
        assert_eq!(p["chat_image_key"], "pic.png");
        assert_eq!(p["text"], "Five.");
        assert_eq!(p["sender"]["kind"], "agent");
    }
}
