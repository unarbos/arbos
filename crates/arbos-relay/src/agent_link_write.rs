//! What an agent access link may **write** (PR 5 of 5; spec §9). Only a `reply` link reaches
//! any of this; a `read` link is refused and the refusal is in the owner's log.
//!
//! | Route | Writes |
//! | --- | --- |
//! | `POST /agent-link/chats/{id}/messages` | one message, as the owner |
//! | `POST /agent-link/chats/{id}/reactions` | one reaction, as the owner |
//! | `POST /agent-link/chats/{id}/typing` | "…is typing", never stored |
//!
//! **It reads as the owner, because it is the owner speaking through a delegate.** The row's
//! sender is `user:{user_id}` with the owner's handle, name and avatar. That is the feature, not
//! an oversight: an agent given a reply link is answering *for* somebody.
//!
//! **Still one link.** Writing takes the same `arbl_…` bearer in the same `Authorization`
//! header as reading. No second credential, no second grant to accept.
//!
//! **One chokepoint, still.** Every route here calls [`crate::agent_link_read::may_see`] before
//! it looks at the body. A chat the link may not reach is a 404, so a pinned link cannot write
//! into — or test for — the rest of the account.
//!
//! ## The marker, and why it is not the evidence
//!
//! The row carries `deviceId = "link:{link_id}"`. It is part of the CRDT document, so it travels
//! to every client, into the Matrix mirror and back out through the export, which gives the
//! owner a visible "via agent link" marker in the conversation itself with **no change to
//! `arbos-core`**.
//!
//! It is a label and not a control. On the room socket a client supplies its own Loro bytes, so
//! any of the owner's own devices can write a row with `deviceId: "link:anything"`. The
//! authority is `link_audit`, which only the relay writes and no client can reach: a `message`
//! row whose `message_id` matches. **Clients must join on the audit, not on the label** — a row
//! claiming the marker with no matching audit row is a forgery and is an ordinary message from
//! the owner. That join is what turns a forgeable label into unforgeable evidence (spec §10.5).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::doc::UserMessageOpts;
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::{Frame, FrameType};

use crate::agent_link::{
    client_ip, db_error, not_found, user_agent, AgentLink, LinkPrincipal, Scope,
};
use crate::agent_link_read::{may_see, ChatView};
use crate::db::Db;
use crate::notices::write_doc_as;
use crate::notify::{self, ChatRef};
use crate::people;
use crate::signin::api_error;
use crate::state::{AppState, RoomBroadcast};

/// What a link may write in an hour and in a day. Messages are the number that matters: a
/// reply-scope link exists to answer somebody, and thirty answers an hour is a working
/// conversation, while a hundred is a runaway loop writing in a stranger's name.
const MESSAGES_PER_HOUR: i64 = 30;
const MESSAGES_PER_DAY: i64 = 200;
/// Reactions and typing are cheap and harmless one at a time, and noise in bulk.
const REACTIONS_PER_HOUR: i64 = 60;
const TYPING_PER_HOUR: i64 = 120;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// The same text bound `export::send_message` enforces, so a message a link writes cannot be one
/// the owner's own device would have been refused.
const TEXT_MAX: usize = 20_000;
/// Long enough for a flag or a family sequence, short enough that it is an emoji.
const EMOJI_MAX: usize = 16;
/// How long a typing indicator the clients render stays up; the sender refreshes it.
const TYPING_TTL_MS: i64 = 6_000;

// ---- the gate -----------------------------------------------------------------------------------

/// May this link write at all?
///
/// A `read` link is refused here, and the refusal is recorded: an agent probing the write routes
/// to see whether it can write is something the owner should be able to read about afterwards,
/// and a scope is never raised — to widen a link the owner revokes it and mints another.
fn may_write(
    state: &AppState,
    link: &AgentLink,
    headers: &HeaderMap,
    what: &str,
) -> Option<Response> {
    if link.scope == Scope::Reply {
        return None;
    }
    note(
        state,
        link,
        headers,
        "refused",
        None,
        None,
        &json!({ "wanted": what, "scope": link.scope.as_str() }),
    );
    Some(api_error(
        StatusCode::FORBIDDEN,
        "scope",
        "this link may read but not write; the owner has to mint a new link to change that",
    ))
}

/// Is there room in this link's budget for one more write of `kind`?
///
/// Over the existing `rate_events` counter (`db_people.rs`), keyed on `link:{id}` — strings, so
/// no schema change. Charged **before** the write, because a refusal after the row is in is not
/// a refusal.
fn budget(
    state: &AppState,
    link: &AgentLink,
    kind: &str,
    window_ms: i64,
    limit: i64,
    message: &str,
) -> Option<Response> {
    let key = format!("link:{}", link.id);
    if state
        .db
        .rate_check_and_note(&key, kind, window_ms, limit)
        .unwrap_or(true)
    {
        return None;
    }
    let retry = state.db.rate_retry_s(&key, kind, window_ms).unwrap_or(60);
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, retry.to_string())],
            Json(json!({"error": {"type": "rate_limited", "message": message}})),
        )
            .into_response(),
    )
}

/// One audit row. Every write and every refusal comes through here, because this table is the
/// authority for "via agent link" and a write it does not hold is a write the owner cannot see.
#[allow(clippy::too_many_arguments)]
fn note(
    state: &AppState,
    link: &AgentLink,
    headers: &HeaderMap,
    action: &str,
    chat: Option<&str>,
    message_id: Option<&str>,
    detail: &Value,
) {
    let _ = state.db.note_link_audit(
        &link.id,
        &link.user_id,
        action,
        chat,
        message_id,
        client_ip(headers).as_deref(),
        user_agent(headers).as_deref(),
        detail,
    );
}

/// The device id written onto the row: the marker, not the evidence (see the module note).
fn link_device(link: &AgentLink) -> String {
    format!("link:{}", link.id)
}

impl Db {
    /// Stamp the moment this link first wrote anything. `true` only for the call that set it, so
    /// "your agent link just spoke as you" is a thing that happens once rather than per message.
    pub fn mark_link_first_write(&self, id: &str) -> anyhow::Result<bool> {
        self.with(|c| {
            c.execute(
                "UPDATE agent_links SET first_write_at = ?2
                 WHERE id = ?1 AND first_write_at IS NULL",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
        })
    }
}

/// Everything that happens after a link's row is stored, whatever kind of write it was.
///
/// The relay-side mirror goes through `record_audit` under the chat's own slot and kind
/// `agent_link`, so a link's write shows up in the place the settings audit already uses and the
/// owner has one audit to read rather than two.
fn after_write(
    state: &AppState,
    link: &AgentLink,
    headers: &HeaderMap,
    view: &ChatView,
    action: &str,
    message_id: Option<&str>,
    detail: &Value,
) {
    note(
        state,
        link,
        headers,
        action,
        Some(&view.id),
        message_id,
        detail,
    );
    let _ = state.db.record_audit(
        &view.slot(),
        "agent_link",
        &json!({
            "ts": now_ms(),
            "action": action,
            "link_id": link.id,
            "label": link.label,
            "chat_id": view.id,
            "message_id": message_id,
            "detail": detail,
        }),
    );
    if state.db.mark_link_first_write(&link.id).unwrap_or(false) {
        // The alarm that matters most (spec §9.4). It is a log line and an audit row rather than
        // a push, because a push the apps would render needs a new `PushEventDto` variant in
        // `arbos-core` — a gate-path change, which this stack is deliberately free of. The
        // owner's authoritative view is `GET /me/agent-links/{id}/activity`, which already
        // serves it; the Settings work that renders the alarm adds the variant.
        tracing::warn!(
            user = %link.user_id, link = %link.id, label = %link.label, chat = %view.id,
            "agent link wrote for the first time"
        );
    }
}

// ---- messages -----------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct SendBody {
    pub text: String,
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// `POST /agent-link/chats/{id}/messages {text, reply_to?}` — one message, as the owner.
///
/// This is `export::send_message` with exactly three changes, and everything else deliberately
/// unchanged: the scope check and `may_see` run first, the send budget is charged, and the row's
/// device is `link:{link_id}` instead of a real device id. The block and silence gates, the
/// mention resolution, the single transaction that writes the row **and** its `send` command,
/// the stored row, the room broadcast, the owner's cursor, the Matrix touch, the box wake and the
/// announce all stay as they are — a message from a link has to be the same kind of event as a
/// message from the owner's phone, or something downstream will treat it as a lesser one.
pub async fn send(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<SendBody>>,
) -> Response {
    let link = who.link;
    if let Some(r) = may_write(&state, &link, &headers, "message") {
        return r;
    }
    let Some(view) = may_see(&state, &link, &id) else {
        return not_found();
    };
    let b = body.map(|b| b.0).unwrap_or_default();
    let text = b.text.trim().to_owned();
    if text.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "empty", "text is required");
    }
    if text.chars().count() > TEXT_MAX {
        return api_error(
            StatusCode::BAD_REQUEST,
            "too_long",
            "text is over 20000 characters",
        );
    }
    // Both windows, because thirty an hour alone would allow seven hundred a day.
    if let Some(r) = budget(
        &state,
        &link,
        "linksend",
        HOUR_MS,
        MESSAGES_PER_HOUR,
        "this link has sent its hourly allowance of messages",
    ) {
        return r;
    }
    if let Some(r) = budget(
        &state,
        &link,
        "linksendday",
        DAY_MS,
        MESSAGES_PER_DAY,
        "this link has sent its daily allowance of messages",
    ) {
        return r;
    }
    // A DM partner's block, either way, and an admin silence. The same gate the room push and
    // `/chats/{id}/notify` use, asked of the owner: a link cannot write where its owner cannot.
    // Project chats are the owner's own and have no such gate.
    let cm = match view.project {
        None => match people::load_chat(&state, &view.id) {
            Ok(cm) => {
                if let Err(r) = people::sender_allowed(&state, &link.user_id, &cm) {
                    note(
                        &state,
                        &link,
                        &headers,
                        "refused",
                        Some(&view.id),
                        None,
                        &json!({ "wanted": "message", "why": r.code() }),
                    );
                    return api_error(StatusCode::FORBIDDEN, r.code(), r.message());
                }
                Some(cm)
            }
            Err(_) => return not_found(),
        },
        Some(_) => None,
    };
    let card = state.db.user_card(&link.user_id).ok().flatten();
    let mentions = cm
        .as_ref()
        .map(|cm| people::resolve_mentions(&state, cm, &text, &format!("user:{}", link.user_id)))
        .unwrap_or_default();
    let message_id = ulid();
    let device = link_device(&link);
    let opts = UserMessageOpts {
        reply_to: b.reply_to.filter(|r| !r.trim().is_empty()),
        mentions: mentions.clone(),
        sender: Some(format!("user:{}", link.user_id)),
        // The card's handle is never empty: a legacy device-token account gets its device-name
        // slug, so the row is attributable even without a GitHub login (#1580).
        sender_handle: card
            .as_ref()
            .map(|c| c.handle.clone())
            .filter(|h| !h.is_empty())
            .or_else(|| Some(link.user_id.clone())),
        sender_name: card
            .as_ref()
            .and_then(|c| c.name.clone())
            .filter(|n| !n.is_empty()),
        sender_avatar: card.as_ref().and_then(|c| c.avatar_url.clone()),
        ..Default::default()
    };
    let (mid, dev) = (message_id.clone(), device.clone());
    let body_text = text.clone();
    // The row and its `send` command in one transaction, as the apps write them (crdt-sync
    // §6.2). A row without the command is history to an agent and never work: until #2120 the
    // owner's own HTTP send wrote the row alone and no agent ever answered it.
    let written = write_doc_as(
        &state,
        view.project.as_deref(),
        &view.id,
        &device,
        move |doc| {
            doc.transact(|d| {
                d.append_user_message(&mid, &body_text, &dev, &opts)?;
                d.push_command("send", json!({ "messageId": mid }), &dev);
                Ok::<(), arbos_core::doc::CoreError>(())
            })?;
            Ok(())
        },
    )
    .await;
    let seq = match written {
        Ok(seq) => seq,
        Err(e) => return db_error("write message", e),
    };
    // What the socket path does after a person's row (`rooms.rs`): the owner's own cursor moves
    // past it on every one of their devices, and every agent in the chat is woken — or, with no
    // socket on this room, told to open it.
    let _ = state.db.advance_user_cursor(&link.user_id, &view.id, seq);
    let chat = match &view.project {
        Some(pid) => ChatRef::project(pid, &view.id),
        None => ChatRef::people(&view.id),
    };
    let agents = cm
        .as_ref()
        .map(|cm| people::serving_agents(&state, cm))
        .unwrap_or_default();
    let room = state
        .room(&arbos_core::wire::room_name(chat.project_label(), &view.id))
        .await;
    crate::boxes::after_person_row(&state, &room, &chat, &agents).await;
    let sender = notify::Sender::user(&state, &link.user_id);
    let recorded =
        notify::announce_with(&state, &chat, &sender, &message_id, &text, None, &mentions).await;
    after_write(
        &state,
        &link,
        &headers,
        &view,
        "message",
        Some(&message_id),
        &json!({ "chars": text.chars().count(), "mentions": mentions.len() }),
    );
    (
        StatusCode::CREATED,
        Json(json!({
            "message_id": message_id,
            "seq": seq,
            "recorded": recorded.is_some(),
            "via": device,
        })),
    )
        .into_response()
}

// ---- reactions ----------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct ReactBody {
    pub message: String,
    pub emoji: String,
    /// `true` adds, `false` removes. Defaults to adding.
    #[serde(default = "yes")]
    pub on: bool,
}

fn yes() -> bool {
    true
}

/// `POST /agent-link/chats/{id}/reactions {message, emoji, on}` — one reaction, as the owner.
pub async fn react(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<ReactBody>>,
) -> Response {
    let link = who.link;
    if let Some(r) = may_write(&state, &link, &headers, "reaction") {
        return r;
    }
    let Some(view) = may_see(&state, &link, &id) else {
        return not_found();
    };
    let b = body.map(|b| b.0).unwrap_or_default();
    if b.message.trim().is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "empty", "message is required");
    }
    let emoji = b.emoji.trim().to_owned();
    if emoji.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "empty", "emoji is required");
    }
    if emoji.chars().count() > EMOJI_MAX {
        return api_error(StatusCode::BAD_REQUEST, "too_long", "emoji is too long");
    }
    if let Some(r) = budget(
        &state,
        &link,
        "linkreact",
        HOUR_MS,
        REACTIONS_PER_HOUR,
        "this link has reacted its hourly allowance",
    ) {
        return r;
    }
    let (mid, em, on) = (b.message.trim().to_owned(), emoji.clone(), b.on);
    let sender = format!("user:{}", link.user_id);
    // A reaction on a message the chat does not hold is a 404 and not a silent success: an agent
    // reacting to an id it invented should learn that it did.
    let written = write_doc_as(
        &state,
        view.project.as_deref(),
        &view.id,
        &link_device(&link),
        move |doc| {
            if doc.message(&mid).is_none() {
                anyhow::bail!("no such message");
            }
            doc.set_reaction(&mid, &em, &sender, on)?;
            Ok(())
        },
    )
    .await;
    if let Err(e) = written {
        note(
            &state,
            &link,
            &headers,
            "refused",
            Some(&view.id),
            Some(&b.message),
            &json!({ "wanted": "reaction", "why": e.to_string() }),
        );
        return not_found();
    }
    after_write(
        &state,
        &link,
        &headers,
        &view,
        "reaction",
        Some(&b.message),
        &json!({ "emoji": emoji, "on": b.on }),
    );
    Json(json!({ "ok": true, "message": b.message, "emoji": emoji, "on": b.on })).into_response()
}

// ---- typing -------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct TypingBody {
    #[serde(default = "yes")]
    pub on: bool,
}

/// `POST /agent-link/chats/{id}/typing {on}` — "…is typing", and nothing stored anywhere.
///
/// A `Presence` frame broadcast into the chat's room, which is the same thing a device sends off
/// its own socket: the clients read `handle` and `typing` from the header and show the line over
/// the composer for a few seconds (`AppModel.noteTyping`). It is not written to the document, so
/// it leaves no trace and cannot be replayed — which is correct for a keystroke indicator, and
/// is why the stream in PR 4 does not store one either.
///
/// The header deliberately omits `focused_chat`. That field tells the relay a device has this
/// chat on screen and suppresses the owner's own notifications for it; a link is not a screen
/// the owner is looking at, and claiming otherwise would silence their phone.
pub async fn typing(
    State(state): State<AppState>,
    who: LinkPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<TypingBody>>,
) -> Response {
    let link = who.link;
    if let Some(r) = may_write(&state, &link, &headers, "typing") {
        return r;
    }
    let Some(view) = may_see(&state, &link, &id) else {
        return not_found();
    };
    if let Some(r) = budget(
        &state,
        &link,
        "linktyping",
        HOUR_MS,
        TYPING_PER_HOUR,
        "this link has sent its hourly allowance of typing indicators",
    ) {
        return r;
    }
    let on = body.map(|b| b.0.on).unwrap_or(true);
    let card = state.db.user_card(&link.user_id).ok().flatten();
    let handle = card
        .as_ref()
        .map(|c| c.handle.clone())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| link.user_id.clone());
    let frame = Frame::new(
        FrameType::Presence,
        json!({
            "device": link_device(&link),
            "at": now_ms(),
            "user": link.user_id,
            "handle": handle,
            "typing": on,
        }),
        Bytes::new(),
    )
    .encode();
    let room = state
        .room(&arbos_core::wire::room_name(
            view.project.as_deref().unwrap_or(people::PEOPLE_PROJECT),
            &view.id,
        ))
        .await;
    let _ = room.tx.send(RoomBroadcast {
        from_conn: 0,
        bytes: frame,
        seq: 0,
    });
    // In the log but not in `record_audit`: typing is not a change to the conversation, and
    // mirroring 120 of them an hour into the settings audit would bury the writes that matter.
    note(
        &state,
        &link,
        &headers,
        "typing",
        Some(&view.id),
        None,
        &json!({ "on": on }),
    );
    Json(json!({ "ok": true, "on": on, "ttl_ms": TYPING_TTL_MS })).into_response()
}

#[cfg(test)]
mod tests {
    //! What a write must do, and the five things it must not: happen at all under a read scope,
    //! reach a chat the link was not granted, reach another account, outrun its budget, or reach
    //! a chat the owner themself is blocked out of.

    use axum::body::to_bytes;
    use futures::StreamExt;

    use arbos_core::doc::{ChatDoc, UserMessageOpts as Opts};
    use arbos_core::wire;

    use super::*;
    use crate::agent_link_read::ChatsQuery;
    use crate::db::Chat;
    use crate::matrix::backfill::load_doc;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn json_of(r: Response) -> (StatusCode, Value) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), 1 << 22).await.expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn link_for(state: &AppState, user: &str, scope: Scope, chats: Option<&[String]>) -> AgentLink {
        let id = ulid();
        state
            .db
            .insert_agent_link(
                &id,
                user,
                "Replier",
                scope,
                &arbos_core::auth::hash(&ulid()),
                chats,
                "dev:test",
                7 * 24 * 3_600_000,
            )
            .unwrap();
        state
            .db
            .open_agent_link(&id, &arbos_core::auth::hash(&ulid()))
            .unwrap();
        state.link_hub.note_live(user);
        state.db.agent_link(&id).unwrap().expect("the link")
    }

    fn who(link: &AgentLink) -> LinkPrincipal {
        LinkPrincipal { link: link.clone() }
    }

    fn agent_chat(state: &AppState, project_id: &str, title: &str) -> String {
        let id = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: id.clone(),
                project_id: project_id.to_owned(),
                title: title.to_owned(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 0,
                kind: "chat".to_owned(),
                participants: vec![],
                forked_from: None,
            })
            .unwrap();
        id
    }

    /// An existing message in a chat, so a reaction has something real to land on.
    fn say(state: &AppState, chat_id: &str, project: Option<&str>, message_id: &str, text: &str) {
        let room = wire::room_name(project.unwrap_or(people::PEOPLE_PROJECT), chat_id);
        let doc = ChatDoc::new(Some(5), false);
        let before = doc.oplog_vv();
        doc.append_user_message(message_id, text, "dev:phone", &Opts::default())
            .unwrap();
        state
            .db
            .append_row(
                &room,
                "dev:phone",
                &ulid(),
                &doc.export_updates_since(&before),
            )
            .unwrap();
    }

    fn row_of(state: &AppState, chat: &str, project: Option<&str>, message_id: &str) -> Value {
        load_doc(state, chat, project)
            .unwrap()
            .expect("a doc")
            .projected_rows()
            .into_iter()
            .find(|r| r["id"] == message_id)
            .expect("the row")
    }

    fn audit(state: &AppState, link: &AgentLink) -> Vec<Value> {
        state.db.link_audit_rows(&link.id, None, 100).unwrap()
    }

    /// The feature: a reply link speaks and it reads as the owner, in the owner's own agent
    /// chat. The marker says it came through a link; the audit is what proves it.
    #[tokio::test]
    async fn a_reply_link_speaks_as_the_owner_and_the_audit_is_the_proof() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9101);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Reply, None);

        let (status, body) = json_of(
            send(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(SendBody {
                    text: "  step 4 needs the cache primed  ".into(),
                    reply_to: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let posted = body["message_id"].as_str().expect("an id").to_owned();
        assert_eq!(body["via"], format!("link:{}", link.id));

        let row = row_of(&state, &chat, Some(&project), &posted);
        assert_eq!(
            row["role"], "user",
            "a link writes a user row, not an agent one"
        );
        assert_eq!(row["status"], "done");
        assert_eq!(row["sender"], format!("user:{alice}"));
        assert_eq!(row["senderHandle"], "user9101");
        assert_eq!(
            arbos_core::doc::message_text(&row),
            "step 4 needs the cache primed",
            "the text is trimmed"
        );
        // The marker: a label in the document, so it travels to every client for free.
        assert_eq!(row["deviceId"], format!("link:{}", link.id));

        // The evidence: the join the clients must make before they show the marker.
        let rows = audit(&state, &link);
        let wrote = rows
            .iter()
            .find(|r| r["action"] == "message")
            .expect("a message audit row");
        assert_eq!(wrote["message_id"], posted.as_str());
        assert_eq!(wrote["chat_id"], chat.as_str());
        assert_eq!(wrote["detail"]["chars"], 29);
    }

    /// A read-only link is refused, nothing is written, and the owner can read that it tried.
    #[tokio::test]
    async fn a_read_link_cannot_write_and_the_attempt_is_on_the_record() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9201);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Read, None);

        for (name, r) in [
            (
                "message",
                send(
                    State(state.clone()),
                    who(&link),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(SendBody {
                        text: "let me in".into(),
                        reply_to: None,
                    })),
                )
                .await,
            ),
            (
                "reaction",
                react(
                    State(state.clone()),
                    who(&link),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(ReactBody {
                        message: "m1".into(),
                        emoji: "👍".into(),
                        on: true,
                    })),
                )
                .await,
            ),
            (
                "typing",
                typing(
                    State(state.clone()),
                    who(&link),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(TypingBody { on: true })),
                )
                .await,
            ),
        ] {
            let (status, body) = json_of(r).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{name}: {body}");
            assert_eq!(body["error"]["type"], "scope", "{name}");
        }

        assert!(
            load_doc(&state, &chat, Some(&project)).unwrap().is_none(),
            "a refused write still touched the document"
        );
        let refusals: Vec<Value> = audit(&state, &link)
            .into_iter()
            .filter(|r| r["action"] == "refused")
            .collect();
        assert_eq!(refusals.len(), 3, "every refusal is recorded");
        assert!(
            refusals.iter().all(|r| r["detail"]["scope"] == "read"),
            "{refusals:?}"
        );
    }

    /// The chokepoint. A pinned link cannot write into — or test for — the rest of the account,
    /// and another account is unreachable even when the link names its chat.
    #[tokio::test]
    async fn a_write_cannot_reach_a_chat_the_link_was_not_granted() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9301);
        let (bob, _) = person(&state, 9302);
        let (mallory, _) = person(&state, 9303);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Launch week");
        let private = dm(&state, &alice, &bob);
        let theirs = dm(&state, &mallory, &bob);

        let pinned = link_for(
            &state,
            &alice,
            Scope::Reply,
            Some(std::slice::from_ref(&granted)),
        );
        for chat in [&private, &theirs] {
            let (status, _) = json_of(
                send(
                    State(state.clone()),
                    who(&pinned),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(SendBody {
                        text: "not yours".into(),
                        reply_to: None,
                    })),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "chat {chat}");
            assert!(load_doc(&state, chat, None).unwrap().is_none());
        }

        // A link over the whole account still cannot reach somebody else's chat.
        let whole = link_for(&state, &alice, Scope::Reply, None);
        let (status, _) = json_of(
            send(
                State(state.clone()),
                who(&whole),
                Path(theirs.clone()),
                HeaderMap::new(),
                Some(Json(SendBody {
                    text: "not yours either".into(),
                    reply_to: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// A link cannot write where its owner cannot: the block and silence gates are asked of the
    /// owner, so the announcement path is never looser through a link than through a phone.
    #[tokio::test]
    async fn a_block_stops_the_link_exactly_where_it_stops_the_owner() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9401);
        let (bob, _) = person(&state, 9402);
        let chat = dm(&state, &alice, &bob);
        let link = link_for(&state, &alice, Scope::Reply, None);

        state.db.block(&bob, &alice).unwrap();
        let (status, body) = json_of(
            send(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(SendBody {
                    text: "hello anyway".into(),
                    reply_to: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(load_doc(&state, &chat, None).unwrap().is_none());
        assert!(
            audit(&state, &link)
                .iter()
                .any(|r| r["action"] == "refused" && r["detail"]["wanted"] == "message"),
            "the refusal is in the owner's log"
        );
    }

    /// Nothing is written over the budget, and the refusal says when to come back.
    #[tokio::test]
    async fn the_send_budget_refuses_before_anything_is_written() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9501);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Reply, None);

        for n in 0..MESSAGES_PER_HOUR {
            let (status, body) = json_of(
                send(
                    State(state.clone()),
                    who(&link),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(SendBody {
                        text: format!("line {n}"),
                        reply_to: None,
                    })),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "message {n}: {body}");
        }
        let before = load_doc(&state, &chat, Some(&project))
            .unwrap()
            .unwrap()
            .projected_rows()
            .len();
        let r = send(
            State(state.clone()),
            who(&link),
            Path(chat.clone()),
            HeaderMap::new(),
            Some(Json(SendBody {
                text: "one too many".into(),
                reply_to: None,
            })),
        )
        .await;
        assert!(r.headers().contains_key(axum::http::header::RETRY_AFTER));
        let (status, body) = json_of(r).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"]["type"], "rate_limited");
        let after = load_doc(&state, &chat, Some(&project))
            .unwrap()
            .unwrap()
            .projected_rows()
            .len();
        assert_eq!(before, after, "a refused send still wrote a row");
    }

    /// Empty and over-long text are the same refusals the owner's own device gets.
    #[tokio::test]
    async fn the_text_bounds_are_the_owners_own() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9601);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Reply, None);

        for (text, want) in [("   ", "empty"), (&"x".repeat(TEXT_MAX + 1), "too_long")] {
            let (status, body) = json_of(
                send(
                    State(state.clone()),
                    who(&link),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(SendBody {
                        text: text.to_owned(),
                        reply_to: None,
                    })),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"]["type"], want);
        }
    }

    /// A reaction lands on a real message, and an invented message id is a 404 rather than a
    /// silent success — an agent reacting to an id it made up should learn that it did.
    #[tokio::test]
    async fn a_reaction_lands_and_an_invented_message_does_not() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9701);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        say(&state, &chat, Some(&project), "m-real", "the build is red");
        let link = link_for(&state, &alice, Scope::Reply, None);

        let (status, body) = json_of(
            react(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(ReactBody {
                    message: "m-real".into(),
                    emoji: "👍".into(),
                    on: true,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let row = row_of(&state, &chat, Some(&project), "m-real");
        assert!(
            row["reactions"]
                .to_string()
                .contains(&format!("user:{alice}")),
            "the reaction is the owner's: {}",
            row["reactions"]
        );
        assert!(audit(&state, &link)
            .iter()
            .any(|r| r["action"] == "reaction"
                && r["message_id"] == "m-real"
                && r["detail"]["on"] == true));

        let (status, _) = json_of(
            react(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(ReactBody {
                    message: "m-invented".into(),
                    emoji: "👍".into(),
                    on: true,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Typing reaches the room as the presence frame the apps already read, and leaves nothing
    /// behind: no row, no document change, and nothing the stream could replay.
    #[tokio::test]
    async fn typing_is_broadcast_and_never_stored() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9801);
        let (bob, _) = person(&state, 9802);
        let chat = dm(&state, &alice, &bob);
        let link = link_for(&state, &alice, Scope::Reply, None);

        let room_name = wire::room_name(people::PEOPLE_PROJECT, &chat);
        let room = state.room(&room_name).await;
        let mut rx = room.tx.subscribe();
        let head_before = state.db.room_state(&room_name).unwrap().head;

        let (status, body) = json_of(
            typing(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(TypingBody { on: true })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["on"], true);

        let got = rx.try_recv().expect("a presence frame on the room");
        assert_eq!(got.seq, 0, "presence passes through and is not a row");
        let frame = wire::Frame::decode(got.bytes.clone())
            .expect("decodable")
            .expect("a frame");
        assert_eq!(frame.ty, wire::FrameType::Presence);
        // What `AppModel.noteTyping` reads: a handle that is not the viewer's, and the flag.
        assert_eq!(frame.header["handle"], "user9801");
        assert_eq!(frame.header["typing"], true);
        assert_eq!(frame.header["user"], alice.as_str());
        assert!(
            frame.header.get("focused_chat").is_none(),
            "a link is not a screen the owner is looking at"
        );

        assert_eq!(
            state.db.room_state(&room_name).unwrap().head,
            head_before,
            "typing wrote a row"
        );
        assert_eq!(
            state.db.link_events_head(&alice).unwrap(),
            0,
            "typing reached the durable stream backlog"
        );
    }

    /// End to end across the whole stack: a link writes, and the same link's stream is told
    /// about it — because the write goes through the one announce funnel PR 4 hooked.
    #[tokio::test]
    async fn a_links_own_write_comes_back_on_its_stream() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 9901);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Reply, None);

        let watching = crate::agent_link_stream::stream(
            State(state.clone()),
            who(&link),
            axum::extract::Query(crate::agent_link_stream::StreamQuery::default()),
            HeaderMap::new(),
        )
        .await;
        let mut body = watching.into_body().into_data_stream();
        let hello = tokio::time::timeout(std::time::Duration::from_millis(1500), body.next())
            .await
            .expect("hello in time")
            .expect("a chunk")
            .expect("no error");
        assert!(String::from_utf8_lossy(&hello).contains("event: hello"));

        let (status, posted) = json_of(
            send(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(SendBody {
                    text: "answering for him".into(),
                    reply_to: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let mut seen = String::new();
        while let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(std::time::Duration::from_millis(1500), body.next()).await
        {
            seen.push_str(&String::from_utf8_lossy(&chunk));
            if seen.contains("message.received") {
                break;
            }
        }
        assert!(
            seen.contains("message.received") && seen.contains("answering for him"),
            "the write never reached the stream: {seen}"
        );
        assert!(
            seen.contains(posted["message_id"].as_str().unwrap()),
            "a different message reached the stream: {seen}"
        );
    }

    /// "Your agent link just spoke as you" is a thing that happens once, not once per message.
    #[tokio::test]
    async fn the_first_write_is_stamped_exactly_once() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 10001);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Reply, None);
        assert!(
            link.first_write_at.is_none(),
            "a fresh link has not written"
        );

        for n in 0..3 {
            let (status, _) = json_of(
                send(
                    State(state.clone()),
                    who(&link),
                    Path(chat.clone()),
                    HeaderMap::new(),
                    Some(Json(SendBody {
                        text: format!("line {n}"),
                        reply_to: None,
                    })),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
        }
        let fresh = state.db.agent_link(&link.id).unwrap().expect("the link");
        let first = fresh.first_write_at.expect("stamped");
        assert!(
            !state.db.mark_link_first_write(&link.id).unwrap(),
            "a later write moved the first-write mark"
        );
        assert_eq!(
            state
                .db
                .agent_link(&link.id)
                .unwrap()
                .unwrap()
                .first_write_at,
            Some(first)
        );
    }

    /// The owner's settings audit and the link's own log are the same story: one `agent_link`
    /// entry under the chat's slot per write that changes the conversation, and none for typing.
    #[tokio::test]
    async fn the_write_is_mirrored_into_the_audit_the_owner_already_reads() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 10101);
        let project = project_of(&state, &alice, "codingarbos");
        let chat = agent_chat(&state, &project, "Launch week");
        let link = link_for(&state, &alice, Scope::Reply, None);

        let (status, _) = json_of(
            send(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(SendBody {
                    text: "mirrored".into(),
                    reply_to: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _) = json_of(
            typing(
                State(state.clone()),
                who(&link),
                Path(chat.clone()),
                HeaderMap::new(),
                Some(Json(TypingBody { on: true })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let entries: Vec<Value> = state
            .db
            .audit(&project, 50)
            .unwrap()
            .into_iter()
            .filter(|e| e["action"].is_string())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "typing was mirrored, or the message was not: {entries:?}"
        );
        assert_eq!(entries[0]["action"], "message");
        assert_eq!(entries[0]["link_id"], link.id.as_str());
    }

    /// A link whose grant is a list can still read the list — the write routes did not quietly
    /// widen what `may_see` means for everything else.
    #[tokio::test]
    async fn the_write_routes_use_the_same_chokepoint_the_reads_do() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 10201);
        let project = project_of(&state, &alice, "codingarbos");
        let granted = agent_chat(&state, &project, "Launch week");
        let link = link_for(
            &state,
            &alice,
            Scope::Reply,
            Some(std::slice::from_ref(&granted)),
        );

        let (status, body) = json_of(
            crate::agent_link_read::chats(
                State(state.clone()),
                who(&link),
                axum::extract::Query(ChatsQuery::default()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = body["chats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![granted.as_str()]);

        let (status, _) = json_of(
            send(
                State(state.clone()),
                who(&link),
                Path(granted.clone()),
                HeaderMap::new(),
                Some(Json(SendBody {
                    text: "inside the grant".into(),
                    reply_to: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
}
