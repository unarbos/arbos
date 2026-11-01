//! Stage B (design §4): the per-user read flip and the push path through the homeserver.
//!
//! A user with `users.matrix_reads = 1` (and `ARBOS_MATRIX=on`) gets unread counts and the badge
//! from Synapse's own `notification_count`s instead of `chat_events`, and — when APNs is on —
//! alerts from the push gateway (`POST /_matrix/push/v1/notify`) instead of `notify::announce`.
//! Rollback is the flag off: nothing to restore, both stores keep writing.

use serde_json::{json, Value};

use arbos_core::api::{ChannelMessagePayloadDto, PushEventDto};

use super::client::{Client, MxResult};
use super::Mode;
use crate::db_push::UnreadChat;
use crate::notify::{ChatRef, Sender};
use crate::state::AppState;

/// `app_id` of the relay's pusher on every flipped device.
pub const PUSHER_APP_ID: &str = "life.arbos.relay";

/// Are this user's reads served from the homeserver?
pub fn reads_on(state: &AppState, user_id: &str) -> bool {
    state.matrix.cfg.mode == Mode::On
        && state
            .db
            .matrix_reads_users()
            .map(|u| u.iter().any(|x| x == user_id))
            .unwrap_or(false)
}

/// Unread per chat as Synapse counts it, mapped back to relay chat ids. `None` when the
/// homeserver did not answer (the caller falls back to the relay's own counts).
pub async fn unread_for_user(state: &AppState, user_id: &str) -> Option<Vec<UnreadChat>> {
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    let mxid = state.matrix.cfg.user_mxid(user_id);
    let counts = match client.unread_counts(&mxid).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(user = %user_id, error = %e, "matrix reads: unread from homeserver failed; relay counts used");
            return None;
        }
    };
    let mut out = Vec::new();
    for (room_id, notif, _) in counts {
        let Some(room) = state.db.matrix_room_by_room_id(&room_id).ok().flatten() else {
            continue;
        };
        // Archived chats never count (same rule as the relay's own query).
        if state
            .db
            .chat_archived_at(user_id, &room.chat_id)
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }
        if notif > 0 {
            out.push(UnreadChat {
                chat_id: room.chat_id,
                project_id: room.project_id,
                unread: notif,
                // Synapse counts messages here, not the unread-mention mark, which is the
                // relay's own state: the caller merges it in with
                // [`crate::db_push::Db::merge_unread_mentions`].
                mentions: 0,
            });
        }
    }
    Some(out)
}

/// Total unread for the badge, from the homeserver when flipped, else the relay.
pub async fn unread_total(state: &AppState, user_id: &str) -> i64 {
    if reads_on(state, user_id) {
        if let Some(chats) = unread_for_user(state, user_id).await {
            return chats.iter().map(|c| c.unread).sum();
        }
    }
    state.db.unread_total(user_id).unwrap_or(0)
}

/// Register (or remove) the relay as push gateway on every live device of a user (the
/// `pushkey` is the Arbos device id). Called from the worker on the `Pushers` job: at the
/// read flip, and — since 2026-10-26 — whenever a device gets its Matrix identity, because
/// server channels live only on the homeserver and its push rules are the one place that
/// knows "a new message landed in a room this user is in" (`on_notify` below).
pub async fn set_pushers(
    state: &AppState,
    client: &Client,
    user_id: &str,
    on: bool,
) -> MxResult<()> {
    let mxid = state.matrix.cfg.user_mxid(user_id);
    let url = format!(
        "{}/_matrix/push/v1/notify",
        state.cfg.public_url.trim_end_matches('/')
    );
    let devices: Vec<String> = state
        .db
        .devices_of_user(user_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d["revoked_at"].is_null())
        .filter_map(|d| d["id"].as_str().map(str::to_owned))
        .collect();
    for device_id in devices {
        let body = if on {
            json!({
                "app_display_name": "Arbos", "app_id": PUSHER_APP_ID, "append": false,
                "device_display_name": device_id, "kind": "http", "lang": "en",
                "pushkey": device_id, "data": { "url": url, "format": "event_id_only" },
            })
        } else {
            json!({ "app_id": PUSHER_APP_ID, "kind": null, "pushkey": device_id })
        };
        client.set_pusher(&mxid, &body).await?;
    }
    Ok(())
}

/// One `notify` call from Synapse, one device at a time. Two kinds of room: a mirrored chat of
/// a flipped user (stage B) gets the APNs alert from here instead of `notify::announce`; a
/// **server channel** (any user — channels exist only on the homeserver, so nothing else sees
/// the message) gets a `channel_message` on `/me/pushes` (the in-app banner) and, when the key
/// exists, an APNs alert to that device. Never the sender's own device; never a server the
/// recipient archived. Returns the pushkeys to reject (no such device) so Synapse drops them.
pub async fn on_notify(state: &AppState, body: &Value) -> Vec<String> {
    let n = &body["notification"];
    let mut rejected = Vec::new();
    let (Some(room_id), Some(event_id)) = (n["room_id"].as_str(), n["event_id"].as_str()) else {
        return rejected;
    };
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    // Channel rooms are in `matrix_rooms` too (kind `channel`, chat_id = channel id), so the
    // channel lookup comes first: a channel is never a mirrored chat.
    let channel = state.db.channel_by_room_id(room_id).ok().flatten();
    let chat = if channel.is_none() {
        state
            .db
            .matrix_room_by_room_id(room_id)
            .ok()
            .flatten()
            .map(|room| ChatRef {
                chat_id: room.chat_id,
                project_id: room.project_id,
            })
    } else {
        None
    };
    if chat.is_none() && channel.is_none() {
        return rejected;
    }
    let server = channel
        .as_ref()
        .and_then(|c| state.db.server(&c.server_id).ok().flatten());
    for d in n["devices"].as_array().into_iter().flatten() {
        if d["app_id"].as_str() != Some(PUSHER_APP_ID) {
            continue;
        }
        let Some(device_id) = d["pushkey"].as_str() else {
            continue;
        };
        let Some(user_id) = state.db.device_user(device_id).ok().flatten() else {
            rejected.push(device_id.to_owned());
            continue;
        };
        let mxid = state.matrix.cfg.user_mxid(&user_id);
        match (&chat, &channel) {
            (Some(chat), _) => {
                if !state.push.is_on() || !reads_on(state, &user_id) {
                    // Not flipped: `notify::announce` covers mirrored chats. The pusher stays
                    // (it carries the channel pushes too).
                    continue;
                }
                // `event_id_only`: fetch the event as the recipient to build the same alert as today.
                let Ok(ev) = client.event(room_id, &mxid, event_id).await else {
                    continue;
                };
                let content = &ev["content"];
                let Some(sender) = sender_of_event(state, &ev) else {
                    continue;
                };
                if sender.user_id.as_deref() == Some(user_id.as_str()) {
                    continue;
                }
                let message_id = content["life.arbos.id"].as_str().unwrap_or(event_id);
                let text = content["body"].as_str().unwrap_or("New message");
                let badge = unread_total(state, &user_id).await;
                crate::notify::alert_device(
                    state, chat, &sender, message_id, text, badge, device_id,
                )
                .await;
            }
            (None, Some(ch)) => {
                let Some(server) = server.as_ref() else {
                    continue;
                };
                // Archived server = hidden on purpose: no banner (same rule as people chats).
                if state
                    .db
                    .space_archived_at(&user_id, &server.id)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    continue;
                }
                let Ok(ev) = client.event(room_id, &mxid, event_id).await else {
                    continue;
                };
                if ev["type"].as_str() != Some("m.room.message") {
                    continue;
                }
                let Some(sender) = sender_of_event(state, &ev) else {
                    continue;
                };
                if sender.user_id.as_deref() == Some(user_id.as_str()) {
                    continue;
                }
                let text = ev["content"]["body"]
                    .as_str()
                    .map(crate::notify::brief)
                    .filter(|t| !t.is_empty());
                let event = PushEventDto::ChannelMessage(ChannelMessagePayloadDto {
                    server_id: server.id.clone(),
                    server_name: server.name.clone(),
                    channel_id: ch.id.clone(),
                    channel_name: ch.name.clone(),
                    room_id: room_id.to_owned(),
                    event_id: event_id.to_owned(),
                    sender: sender.dto(),
                    text: text.clone(),
                });
                let (kind, payload) = event.kind_and_payload();
                if let Err(e) =
                    state
                        .db
                        .push_device(&user_id, device_id, &kind, &payload, Some(room_id))
                {
                    tracing::warn!(device = %device_id, error = %e, "channel push: queue failed");
                }
                // The APNs alert for channels follows as its own item once the outbox
                // pipeline (push-outbox-relay-2026-10-27) is on main; the in-app path is live now.
            }
            (None, None) => {}
        }
    }
    rejected
}

/// The relay `Sender` behind a Matrix event: `life.arbos.sender` when the relay wrote it,
/// else the Matrix sender mapped back.
fn sender_of_event(state: &AppState, ev: &Value) -> Option<Sender> {
    let member = ev["content"]["life.arbos.sender"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            ev["sender"]
                .as_str()
                .and_then(|s| state.matrix.cfg.member_of_mxid(s))
        })?;
    if let Some(u) = member.strip_prefix("user:") {
        return Some(Sender::user(state, u));
    }
    let p = member.strip_prefix("agent:")?;
    let project = state.db.project(p).ok().flatten()?;
    Some(Sender::agent(state, &project))
}
