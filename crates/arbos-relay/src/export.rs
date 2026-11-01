//! Read-only chat export for a **reader** device (Jacob's Grok Bot, 2026-10-23;
//! `docs/grok-bot-read-access.md`). A reader is a device row with `kind = "reader"` whose token
//! only works for the routes in [`READER_ROUTES`]; everything else answers 403 `reader_only`,
//! so a leaked token can read but never post, call or delete. Nothing joins a chat and no system
//! line is written: the export is a machine view of what the owner's own device already holds.
//!
//! `GET /chats/{id}/export?since=<ms>&after_id=<id>&limit=<n>&format=json|md` rebuilds the
//! chat doc the way the Matrix backfill does (`matrix::backfill::load_doc`: checkpoint + rows,
//! read-only, in memory) and returns the finished messages in time order after the cursor.
//! Attachments come as `/export/blob/{chat}/{key}?exp=&sig=` URLs signed with the relay's JWT key,
//! valid for one hour, needing no bearer.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use arbos_core::doc::{message_text, ChatDoc, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire::{self, Frame, FrameType};

use crate::auth::Principal;
use crate::boxes;
use crate::matrix::backfill::load_doc;
use crate::notify::{self, ChatRef};
use crate::people::{self, PEOPLE_ACL_PREFIX, PEOPLE_PROJECT};
use crate::signin::api_error;
use crate::state::{AppState, RoomBroadcast};

/// Device kind of a read-only token.
pub const READER_KIND: &str = "reader";
/// What a reader may call: list its chats, export one, fetch a signed attachment.
const READER_ROUTES: [&str; 3] = ["/chats", "/chats/", "/export/blob/"];
const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;
/// Signed attachment URLs live this long.
const BLOB_URL_TTL_MS: i64 = 60 * 60 * 1000;

/// Is this request path open to a reader token? `GET /chats` (the list), `GET /chats/{id}/export`
/// and the signed blob route; `/chats/{id}/notify`, members, reads and everything else are not.
pub fn reader_may(method: &str, path: &str) -> bool {
    if method != "GET" {
        return false;
    }
    if path == READER_ROUTES[0] {
        return true;
    }
    if path.starts_with(READER_ROUTES[2]) {
        return true;
    }
    path.starts_with(READER_ROUTES[1]) && path.ends_with("/export")
}

#[derive(Deserialize, Default)]
pub struct MintBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

/// `POST /auth/devices {name, kind:"reader"}` — the owner (a normal signed-in device) mints a
/// read-only token for a bot. The token is shown once; the device appears in Settings → Devices
/// as "<name> · read-only" and is revoked like any other (`DELETE /auth/devices/{id}`).
pub async fn mint_reader(
    State(state): State<AppState>,
    principal: Principal,
    body: Option<Json<MintBody>>,
) -> Response {
    let Principal::Device {
        id: from, user_id, ..
    } = &principal
    else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    if state
        .db
        .device_kind(from)
        .ok()
        .flatten()
        .is_some_and(|k| k == READER_KIND)
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "reader_only",
            "a reader cannot mint devices",
        );
    }
    let b = body.map(|b| b.0).unwrap_or_default();
    if b.kind.as_deref() != Some(READER_KIND) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_kind",
            "only kind \"reader\" can be minted here",
        );
    }
    let name = b
        .name
        .map(|s| s.trim().chars().take(60).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Reader".to_owned());
    let device_id = ulid();
    // A long-lived bearer (the `adv_` shape the relay has always accepted by hash): a bot has no
    // refresh flow. It dies when the device is revoked.
    let token = format!(
        "adv_{}{}",
        ulid().to_ascii_lowercase(),
        ulid().to_ascii_lowercase()
    );
    if let Err(e) = state.db.insert_device(
        &device_id,
        &name,
        &arbos_core::auth::hash(&token),
        user_id,
        READER_KIND,
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    tracing::info!(user = %user_id, device = %device_id, %name, "reader device minted");
    (
        StatusCode::CREATED,
        Json(json!({
            "device_id": device_id,
            "name": name,
            "kind": READER_KIND,
            "token": token,
            "note": "Shown once. Revoke with DELETE /auth/devices/{device_id}.",
        })),
    )
        .into_response()
}

#[derive(Deserialize, Default)]
pub struct ExportQuery {
    /// Messages created after this time (ms since the epoch); `0` = from the start.
    #[serde(default)]
    pub since: i64,
    /// With `since`: skip messages at exactly `since` up to and including this id.
    #[serde(default)]
    pub after_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// `json` (default) or `md`.
    #[serde(default)]
    pub format: Option<String>,
}

/// One exported message, in the shape both formats are built from.
struct Line {
    id: String,
    at: i64,
    sender: Value,
    text: String,
    attachments: Vec<Value>,
    reply_to: Option<String>,
}

type HmacSha256 = Hmac<Sha256>;

fn blob_sig(key: &[u8], chat_id: &str, blob_key: &str, exp: i64) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(format!("{chat_id}\n{blob_key}\n{exp}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// `https://relay/export/blob/{chat}/{key}?exp=…&sig=…`, good for an hour.
pub fn signed_blob_url(state: &AppState, chat_id: &str, blob_key: &str, now: i64) -> String {
    let exp = now + BLOB_URL_TTL_MS;
    let sig = blob_sig(state.jwt_key.as_ref(), chat_id, blob_key, exp);
    format!(
        "{}/export/blob/{chat_id}/{blob_key}?exp={exp}&sig={sig}",
        state.cfg.public_url.trim_end_matches('/')
    )
}

fn sender_json(state: &AppState, row: &Value) -> Value {
    let raw = row["sender"].as_str().unwrap_or("");
    let role = row["role"].as_str().unwrap_or("");
    if let Some(uid) = raw.strip_prefix("user:") {
        let (handle, name) = state
            .db
            .user(uid)
            .ok()
            .flatten()
            .map(|u| (u.login.unwrap_or_default(), u.name.unwrap_or_default()))
            .unwrap_or_default();
        let handle = row["senderHandle"]
            .as_str()
            .filter(|h| !h.is_empty())
            .map(str::to_owned)
            .unwrap_or(handle);
        let name = row["senderName"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .unwrap_or(name);
        return json!({ "kind": "user", "id": uid, "handle": handle, "name": name });
    }
    if let Some(pid) = raw.strip_prefix("agent:") {
        let name = row["senderName"].as_str().unwrap_or("").to_owned();
        return json!({ "kind": "agent", "id": pid, "name": name });
    }
    if role == "assistant" {
        return json!({ "kind": "agent", "name": row["senderName"].as_str().unwrap_or("") });
    }
    json!({ "kind": "system" })
}

fn lines(state: &AppState, chat_id: &str, rows: &[Value], now: i64) -> Vec<Line> {
    rows.iter()
        .filter(|r| !matches!(r["status"].as_str(), Some("queued" | "streaming")))
        .filter_map(|r| {
            let id = r["id"].as_str()?.to_owned();
            let text = message_text(r);
            let attachments: Vec<Value> = r["parts"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|p| {
                            p["kind"] == "attachment" && p["hidden"].as_bool() != Some(true)
                        })
                        .filter_map(|p| {
                            let a = &p["attachment"];
                            // A picked GIF (ARB-544) has no `ref`: the relay stores no copy,
                            // so the export carries the provider's own URL, which needs no
                            // signature and does not expire. Dropping it used to drop the
                            // whole row when the GIF was the message
                            // ([feedback gif-send-and-playback-2026-10-30]).
                            let (url, expires) = match a["ref"].as_str() {
                                Some(key) => (
                                    signed_blob_url(state, chat_id, key, now),
                                    json!(now + BLOB_URL_TTL_MS),
                                ),
                                None => (
                                    a["url"].as_str().filter(|u| !u.is_empty())?.to_owned(),
                                    Value::Null,
                                ),
                            };
                            Some(json!({
                                "name": a["name"], "mime": a["mime"], "kind": a["kind"],
                                "url": url,
                                "url_expires_at": expires,
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if text.trim().is_empty() && attachments.is_empty() {
                return None;
            }
            Some(Line {
                id,
                at: r["createdAt"].as_i64().unwrap_or(0),
                sender: sender_json(state, r),
                text,
                attachments,
                reply_to: r["replyTo"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

fn render_md(chat_title: &str, lines: &[Line]) -> String {
    let mut out = format!("# {chat_title}\n\n");
    for l in lines {
        let who = match l.sender["kind"].as_str() {
            Some("user") => format!("@{}", l.sender["handle"].as_str().unwrap_or("?")),
            Some("agent") => format!("{} (agent)", l.sender["name"].as_str().unwrap_or("agent")),
            _ => "system".to_owned(),
        };
        let when = chrono::DateTime::from_timestamp_millis(l.at)
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default();
        out.push_str(&format!("**{who}** · {when}\n{}\n", l.text.trim()));
        for a in &l.attachments {
            out.push_str(&format!(
                "- attachment: {} ({}) {}\n",
                a["name"].as_str().unwrap_or("file"),
                a["mime"].as_str().unwrap_or(""),
                a["url"].as_str().unwrap_or("")
            ));
        }
        out.push('\n');
    }
    out
}

/// `GET /chats/{id}/export` — members of the chat (readers included), the admin.
pub async fn export_chat(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Response {
    if !state.allows(&principal, &format!("{PEOPLE_ACL_PREFIX}{id}")) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not in this chat");
    }
    let Some(chat) = state.db.people_chat(&id).ok().flatten() else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
    };
    let members = state.db.members_of(&id).unwrap_or_default();
    let doc = match load_doc(&state, &id, None) {
        Ok(d) => d,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "doc", e.to_string()),
    };
    // Loud fail (#1518): rows the relay holds but cannot apply are messages this export cannot
    // show. Say so instead of answering a silently short list.
    let room_name = wire::room_name(PEOPLE_PROJECT, &id);
    let room = state.room(&room_name).await;
    let truncated = crate::room_health::doc_health(&state, &room, &room_name)
        .ok()
        .filter(|h| h.rows_pending > 0 || h.rows_bad > 0)
        .map(|h| {
            json!({
                "pending_rows": h.rows_pending,
                "first_pending_seq": h.first_pending_seq,
                "reason": format!(
                    "{} stored rows do not apply to the relay's doc (their dependencies were lost from the relay's history); the messages they carry are missing here until a device that holds them reconnects (history heal). See GET /chats/{id}/health.",
                    h.rows_pending + h.rows_bad
                ),
            })
        });
    let now = now_ms();
    let mut all = doc
        .as_ref()
        .map(|d| lines(&state, &id, &d.projected_rows(), now))
        .unwrap_or_default();
    // Stable by time: lines written in the same millisecond keep the doc's own order, which is
    // what the Matrix backfill mirrors. The cursor is (time, id): everything after `since`,
    // plus the same-millisecond lines that follow `after_id` in that order.
    all.sort_by_key(|l| l.at);
    let total = all.len();
    let after_pos = q
        .after_id
        .as_deref()
        .and_then(|a| all.iter().position(|l| l.id == a));
    let rest: Vec<&Line> = all
        .iter()
        .enumerate()
        .filter(|(i, l)| l.at > q.since || (l.at == q.since && after_pos.is_some_and(|p| *i > p)))
        .map(|(_, l)| l)
        .collect();
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let has_more = rest.len() > limit;
    let page: Vec<&Line> = rest.into_iter().take(limit).collect();
    let next = page
        .last()
        .map(|l| json!({ "since": l.at, "after_id": l.id }));
    let title = chat
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            // Bare handles, no `@`: a generated group name reads the same on every surface
            // (see `group_name_tests`).
            let handles: Vec<String> = members
                .iter()
                .filter_map(|m| m.member.strip_prefix("user:"))
                .filter_map(|uid| state.db.user(uid).ok().flatten())
                .filter_map(|u| u.login)
                .collect();
            handles.join(", ")
        });
    if q.format.as_deref() == Some("md") {
        let owned: Vec<Line> = page
            .iter()
            .map(|l| Line {
                id: l.id.clone(),
                at: l.at,
                sender: l.sender.clone(),
                text: l.text.clone(),
                attachments: l.attachments.clone(),
                reply_to: l.reply_to.clone(),
            })
            .collect();
        let mut md = render_md(&title, &owned);
        if let Some(t) = &truncated {
            md.push_str(&format!(
                "<!-- truncated: true pending_rows={} reason={} -->\n",
                t["pending_rows"],
                t["reason"].as_str().unwrap_or("")
            ));
        }
        if let Some(n) = &next {
            md.push_str(&format!(
                "<!-- next: since={} after_id={} has_more={has_more} -->\n",
                n["since"],
                n["after_id"].as_str().unwrap_or("")
            ));
        }
        return ([("content-type", "text/markdown; charset=utf-8")], md).into_response();
    }
    Json(json!({
        "chat": { "id": id, "title": title, "kind": chat.kind, "members": members.len() },
        "messages": page.iter().map(|l| json!({
            "id": l.id, "at": l.at, "sender": l.sender, "text": l.text,
            "attachments": l.attachments, "reply_to": l.reply_to,
        })).collect::<Vec<_>>(),
        "next": next,
        "has_more": has_more,
        "total": total,
        "truncated": truncated.is_some(),
        "truncation": truncated,
        "exported_at": now,
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
pub struct SendBody {
    pub text: String,
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// `POST /chats/{id}/messages {text, reply_to?}` — post one message to a people chat as the
/// calling device, without a Loro client: the relay appends the row the apps would write
/// (`sender: user:<id>`, handle, name), stores and broadcasts it, and announces it (push,
/// agents, Matrix mirror) exactly as `POST /chats/{id}/notify` does after an app's own push.
/// Member devices only; readers are refused by the route gate.
pub async fn send_message(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    body: Option<Json<SendBody>>,
) -> Response {
    let Principal::Device {
        id: device_id,
        user_id,
        ..
    } = &principal
    else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    if !state.allows(&principal, &format!("{PEOPLE_ACL_PREFIX}{id}")) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not in this chat");
    }
    // The same gates as a room push (#2012): a blocked or silenced person writes nothing.
    if let Err(r) = people::sender_gate(&state, &principal, &id) {
        return api_error(StatusCode::FORBIDDEN, r.code(), r.message());
    }
    let Ok(cm) = people::load_chat(&state, &id) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
    };
    let b = body.map(|b| b.0).unwrap_or_default();
    let text = b.text.trim().to_owned();
    if text.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "empty", "text is required");
    }
    if text.chars().count() > 20_000 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "too_long",
            "text is over 20000 characters",
        );
    }
    // The card's handle is never empty: a legacy device-token account gets its device-name
    // slug, so the row is attributable even without a GitHub login (#1580).
    let me = state.db.user_card(user_id).ok().flatten();
    let mentions = people::resolve_mentions(&state, &cm, &text, &format!("user:{user_id}"));
    let opts = UserMessageOpts {
        reply_to: b.reply_to.filter(|r| !r.trim().is_empty()),
        mentions: mentions.clone(),
        sender: Some(format!("user:{user_id}")),
        sender_handle: me
            .as_ref()
            .map(|c| c.handle.clone())
            .filter(|h| !h.is_empty())
            .or_else(|| Some(user_id.clone())),
        sender_name: me
            .as_ref()
            .and_then(|c| c.name.clone())
            .filter(|n| !n.is_empty()),
        sender_avatar: me.as_ref().and_then(|c| c.avatar_url.clone()),
        ..Default::default()
    };
    let room_name = wire::room_name(PEOPLE_PROJECT, &id);
    let doc = match load_doc(&state, &id, None) {
        Ok(d) => d.unwrap_or_else(|| ChatDoc::new(None, false)),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "doc", e.to_string()),
    };
    let base = doc.oplog_vv();
    let message_id = ulid();
    // The row and its `send` command in one transaction, as the apps write them (crdt-sync
    // §6.2). A message without the command is history to the agent, never work: until #2120
    // this route wrote the row alone and no agent ever answered a message posted through it.
    let written = doc.transact(|d| {
        d.append_user_message(&message_id, &text, device_id, &opts)?;
        d.push_command("send", json!({ "messageId": message_id }), device_id);
        Ok::<(), arbos_core::doc::CoreError>(())
    });
    if let Err(e) = written {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "doc", e.to_string());
    }
    let bytes = doc.export_updates_since(&base);
    let batch_id = ulid();
    let seq = match state
        .db
        .append_row(&room_name, device_id, &batch_id, &bytes)
    {
        Ok((seq, _)) => seq,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let frame = Frame::new(
        FrameType::Row,
        json!({ "seq": seq, "device": device_id, "batchId": batch_id }),
        Bytes::from(bytes),
    )
    .encode();
    let room = state.room(&room_name).await;
    let _ = room.tx.send(RoomBroadcast {
        from_conn: 0,
        bytes: frame,
        seq,
    });
    let chat = ChatRef::people(&id);
    // What the socket path does after a person's row (rooms.rs): the sender's own cursor moves
    // past it, the Matrix safety net is armed, and every agent's box is woken — or, with no
    // socket on this room, told to open it (#2120: this route never woke a box).
    let _ = state.db.advance_user_cursor(user_id, &id, seq);
    state.matrix.touch_rows(&state, &id, None);
    boxes::after_person_row(&state, &room, &chat, &people::serving_agents(&state, &cm)).await;
    let sender = notify::Sender::user(&state, user_id);
    let event_id =
        notify::announce_with(&state, &chat, &sender, &message_id, &text, None, &mentions).await;
    (
        StatusCode::CREATED,
        Json(json!({ "message_id": message_id, "seq": seq, "recorded": event_id.is_some() })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct BlobQuery {
    pub exp: i64,
    pub sig: String,
}

/// `GET /export/blob/{chat}/{key}?exp=&sig=` — the signed attachment URL from an export. No
/// bearer: the signature is the credential, and it stops working after `exp`.
pub async fn signed_blob(
    State(state): State<AppState>,
    Path((chat_id, key)): Path<(String, String)>,
    Query(q): Query<BlobQuery>,
) -> Response {
    if q.exp < now_ms() {
        return api_error(
            StatusCode::GONE,
            "expired",
            "this link has expired; export again",
        );
    }
    let want = blob_sig(state.jwt_key.as_ref(), &chat_id, &key, q.exp);
    if !constant_eq(want.as_bytes(), q.sig.as_bytes()) {
        return api_error(StatusCode::FORBIDDEN, "bad_signature", "bad link");
    }
    crate::blobs::serve(&state, &format!("{PEOPLE_ACL_PREFIX}{chat_id}"), &key).await
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::SocketAddr;

    use axum::body::to_bytes;
    use axum::Router;

    use arbos_core::doc::UserAttachment;

    use arbos_core::wire::BoxState;

    use super::*;
    use crate::matrix::backfill::{self, Scope};
    use crate::matrix::{Config, Job, Matrix, Mode};
    use crate::people::tests_support::{dm, group, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn dual(state: &mut AppState) {
        let mut cfg = Config::off();
        cfg.mode = Mode::Dual;
        cfg.as_token_users = "t".into();
        cfg.hs_token_users = "h".into();
        state.matrix = Matrix::new(cfg);
    }

    /// A device row of `kind` whose raw bearer we know (the relay stores its hash).
    fn device(state: &AppState, user: &str, kind: &str) -> (Principal, String) {
        let id = ulid();
        let token = format!("adv_{}", ulid().to_ascii_lowercase());
        state
            .db
            .insert_device(&id, kind, &arbos_core::auth::hash(&token), user, kind)
            .unwrap();
        (
            Principal::Device {
                id,
                token: token.clone(),
                user_id: user.to_owned(),
            },
            token,
        )
    }

    /// `n` finished user messages from `user` (the last one carries an attachment and replies to
    /// the first), one system line, one still-streaming assistant row. Returns the message ids
    /// in append order.
    fn seed(state: &AppState, chat_id: &str, user: &str, n: usize) -> Vec<String> {
        let doc = ChatDoc::new(Some(5), false);
        doc.init_meta(chat_id, PEOPLE_PROJECT, 1);
        let mut ids = Vec::new();
        for i in 0..n {
            let id = ulid();
            let mut opts = UserMessageOpts {
                sender: Some(format!("user:{user}")),
                sender_handle: Some("alice".into()),
                ..Default::default()
            };
            if i + 1 == n {
                opts.reply_to = ids.first().cloned();
                opts.attachments.push(UserAttachment {
                    name: "plan.pdf".into(),
                    mime: "application/pdf".into(),
                    kind: Some("file".into()),
                    blob_ref: Some("att-1".into()),
                    url: None,
                    source: None,
                });
            }
            doc.append_user_message(&id, &format!("message {i}"), "dev1", &opts)
                .expect("user message");
            ids.push(id);
        }
        doc.append_system_line(&ulid(), "alice added bob", "dev1", None)
            .expect("system line");
        doc.start_assistant_message(&ulid(), &ids[0], "box1");
        let bytes = doc.export_updates_since(&Default::default());
        state
            .db
            .append_row(
                &wire::room_name(PEOPLE_PROJECT, chat_id),
                "dev1",
                &ulid(),
                &bytes,
            )
            .expect("row");
        ids
    }

    async fn json_of(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    async fn text_of(resp: Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn export(
        state: &AppState,
        p: &Principal,
        chat: &str,
        since: i64,
        after_id: Option<&str>,
        limit: Option<usize>,
        format: Option<&str>,
    ) -> Response {
        export_chat(
            State(state.clone()),
            p.clone(),
            Path(chat.to_owned()),
            Query(ExportQuery {
                since,
                after_id: after_id.map(str::to_owned),
                limit,
                format: format.map(str::to_owned),
            }),
        )
        .await
    }

    /// Every `.route("…", …)` in the relay's router, as `(path, methods)`. Read from `main.rs`
    /// so a route added later is covered without touching this test.
    fn route_table() -> Vec<(String, Vec<String>)> {
        let src = include_str!("main.rs");
        let mut out = Vec::new();
        for chunk in src.split(".route(").skip(1) {
            let chunk = chunk.split("\n        .").next().unwrap_or(chunk);
            let Some(path) = chunk.split('"').nth(1) else {
                continue;
            };
            let methods: Vec<String> = ["get(", "post(", "put(", "patch(", "delete(", "any("]
                .iter()
                .filter(|m| chunk.contains(**m))
                .flat_map(|m| match m.trim_end_matches('(') {
                    "any" => vec!["GET", "POST"],
                    other => vec![other],
                })
                .map(|m| m.to_ascii_uppercase())
                .collect();
            out.push((path.to_owned(), methods));
        }
        out
    }

    fn concrete(path: &str) -> String {
        path.split('/')
            .map(|seg| {
                if seg.starts_with("{*") {
                    "x/y".to_owned()
                } else if seg.starts_with('{') {
                    "01TEST".to_owned()
                } else {
                    seg.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    async fn probe(_: Principal) -> StatusCode {
        StatusCode::OK
    }

    async fn serve_probe(state: AppState) -> SocketAddr {
        let app = Router::new().fallback(probe).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    #[tokio::test]
    async fn reader_token_is_refused_on_every_non_read_route() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 3001);
        let (_, reader_tok) = device(&state, &alice, READER_KIND);
        let (_, normal_tok) = device(&state, &alice, "github");
        let addr = serve_probe(state).await;
        let http = reqwest::Client::new();
        let table = route_table();
        assert!(table.len() > 200, "route table parsed: {}", table.len());

        let mut allowed = HashSet::new();
        let mut refused = 0;
        for (path, methods) in &table {
            for m in methods {
                let url = format!("http://{addr}{}", concrete(path));
                let method = reqwest::Method::from_bytes(m.as_bytes()).unwrap();
                let r = http
                    .request(method.clone(), &url)
                    .bearer_auth(&reader_tok)
                    .send()
                    .await
                    .unwrap();
                if reader_may(m, &concrete(path)) {
                    assert_eq!(r.status(), 200, "{m} {path} is a read route");
                    allowed.insert(format!("{m} {path}"));
                } else {
                    assert_eq!(r.status(), 403, "{m} {path} must refuse a reader");
                    let body: Value = r.json().await.unwrap();
                    assert_eq!(body["error"]["type"], "reader_only", "{m} {path}");
                    refused += 1;
                }
                // The same route with an ordinary device token passes the gate.
                let r = http
                    .request(method, &url)
                    .bearer_auth(&normal_tok)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(r.status(), 200, "{m} {path} with a normal device");
            }
        }
        assert_eq!(
            allowed,
            HashSet::from([
                "GET /chats".to_owned(),
                "GET /chats/{id}/export".to_owned(),
                "GET /export/blob/{chat}/{key}".to_owned(),
            ])
        );
        assert!(refused > 250, "refused {refused}");
    }

    #[tokio::test]
    async fn owner_mints_a_reader_and_a_reader_cannot() {
        let state = test_state(AuthMode::Github);
        let (alice, owner) = person(&state, 3002);
        let (_, mint) = json_of(
            mint_reader(
                State(state.clone()),
                owner.clone(),
                Some(Json(MintBody {
                    name: Some(" Grok Bot ".into()),
                    kind: Some("reader".into()),
                })),
            )
            .await,
        )
        .await;
        assert_eq!(mint["kind"], "reader");
        assert_eq!(mint["name"], "Grok Bot");
        let token = mint["token"].as_str().unwrap().to_owned();
        assert!(token.starts_with("adv_"));
        let device_id = mint["device_id"].as_str().unwrap().to_owned();
        assert_eq!(
            state.db.device_kind(&device_id).unwrap().as_deref(),
            Some("reader")
        );
        let Ok(Principal::Device { id, user_id, .. }) =
            crate::auth::resolve_checked(&state, &token)
        else {
            panic!("reader token resolves");
        };
        assert_eq!((id, user_id), (device_id, alice.clone()));

        // Wrong / missing kind is a 400; a reader minting a reader is refused; a box cannot mint.
        let (status, body) = json_of(
            mint_reader(
                State(state.clone()),
                owner.clone(),
                Some(Json(MintBody {
                    name: None,
                    kind: None,
                })),
            )
            .await,
        )
        .await;
        assert_eq!(
            (status, body["error"]["type"].as_str()),
            (StatusCode::BAD_REQUEST, Some("bad_kind"))
        );
        let (reader, _) = device(&state, &alice, READER_KIND);
        let (status, body) = json_of(
            mint_reader(
                State(state.clone()),
                reader,
                Some(Json(MintBody {
                    name: None,
                    kind: Some("reader".into()),
                })),
            )
            .await,
        )
        .await;
        assert_eq!(
            (status, body["error"]["type"].as_str()),
            (StatusCode::FORBIDDEN, Some("reader_only"))
        );
        let (status, _) = json_of(
            mint_reader(
                State(state.clone()),
                Principal::Box { user_id: alice },
                Some(Json(MintBody {
                    name: None,
                    kind: Some("reader".into()),
                })),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn export_matches_the_backfill_decoder() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (alice, _) = person(&state, 3003);
        let (bob, _) = person(&state, 3004);
        let chat = dm(&state, &alice, &bob);
        let ids = seed(&state, &chat, &alice, 3);
        let (reader, _) = device(&state, &alice, READER_KIND);

        let (status, body) =
            json_of(export(&state, &reader, &chat, 0, None, None, None).await).await;
        assert_eq!(status, StatusCode::OK);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(body["has_more"], false);
        assert_eq!(body["chat"]["id"], chat);
        // An untitled chat's export title is its roster, in roster order. Both members join in
        // the same millisecond, so `members_of`'s tiebreak decides which handle leads; pinning
        // it here is the point, because an export is a document a person keeps. The handles are
        // bare: the generated name has one form on every surface (see `group_name_tests`).
        assert_eq!(body["chat"]["title"], "user3003, user3004");

        // What the Matrix backfill would mirror for the same chat, in stream order.
        backfill::run(
            &state,
            Scope {
                chat_id: Some(chat.clone()),
                ..Default::default()
            },
        );
        let mut mirrored = Vec::new();
        loop {
            let due = state.db.matrix_outbox_due(10).unwrap();
            let Some(row) = due.first() else { break };
            if let Job::Message {
                message_id,
                sender,
                text,
                ts,
                parts,
                ..
            } = &row.job
            {
                mirrored.push((
                    message_id.clone(),
                    sender.clone(),
                    text.clone(),
                    ts.unwrap_or_default(),
                    parts.is_some(),
                ));
            }
            state.db.matrix_outbox_done(row.id).unwrap();
        }
        assert_eq!(mirrored.len(), 4, "3 user messages + the system line");

        let exported: Vec<(String, String, String, i64, bool)> = msgs
            .iter()
            .map(|m| {
                let sender = match m["sender"]["kind"].as_str() {
                    Some("user") => format!("user:{}", m["sender"]["id"].as_str().unwrap()),
                    Some("agent") => format!("agent:{}", m["sender"]["id"].as_str().unwrap()),
                    _ => "system".to_owned(),
                };
                (
                    m["id"].as_str().unwrap().to_owned(),
                    sender,
                    m["text"].as_str().unwrap().to_owned(),
                    m["at"].as_i64().unwrap(),
                    !m["attachments"].as_array().unwrap().is_empty(),
                )
            })
            .collect();
        assert_eq!(exported, mirrored, "same rows, senders, text, timestamps");

        // The attachment comes as a signed, bearer-less URL, and the reply link survives.
        let last = msgs.iter().find(|m| m["id"] == ids[2]).unwrap();
        assert_eq!(last["reply_to"], ids[0]);
        let url = last["attachments"][0]["url"].as_str().unwrap();
        assert!(
            url.starts_with(&format!(
                "http://127.0.0.1:7707/export/blob/{chat}/att-1?exp="
            )),
            "{url}"
        );
        assert!(url.contains("&sig="));
        assert_eq!(last["attachments"][0]["name"], "plan.pdf");
        assert_eq!(last["sender"]["handle"], "alice");

        // Markdown carries the same lines.
        let (_, md) =
            text_of(export(&state, &reader, &chat, 0, None, None, Some("md")).await).await;
        assert!(md.starts_with("# user3003, user3004\n"));
        assert_eq!(md.matches("**@alice**").count(), 3);
        assert!(md.contains("**system**"));
        assert!(md.contains("- attachment: plan.pdf (application/pdf) http://"));
        assert!(md.contains("<!-- next: since="));
    }

    #[tokio::test]
    async fn cursor_walks_every_message_once() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 3005);
        let (bob, _) = person(&state, 3006);
        let chat = dm(&state, &alice, &bob);
        let ids = seed(&state, &chat, &alice, 7);
        let (reader, _) = device(&state, &alice, READER_KIND);

        let mut seen: Vec<String> = Vec::new();
        let mut since = 0;
        let mut after: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, body) = json_of(
                export(
                    &state,
                    &reader,
                    &chat,
                    since,
                    after.as_deref(),
                    Some(3),
                    None,
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            pages += 1;
            let msgs = body["messages"].as_array().unwrap();
            assert!(msgs.len() <= 3);
            seen.extend(msgs.iter().map(|m| m["id"].as_str().unwrap().to_owned()));
            if body["has_more"] != true {
                assert!(body["next"].is_object() || msgs.is_empty());
                break;
            }
            since = body["next"]["since"].as_i64().unwrap();
            after = body["next"]["after_id"].as_str().map(str::to_owned);
        }
        assert_eq!(pages, 3, "8 lines in pages of 3");
        assert_eq!(seen.len(), 8, "7 messages + the system line, each once");
        assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 8);
        for id in &ids {
            assert!(seen.contains(id));
        }
        // Polling again from the final cursor yields nothing new.
        let (_, body) = json_of(
            export(
                &state,
                &reader,
                &chat,
                since,
                after.as_deref(),
                Some(3),
                None,
            )
            .await,
        )
        .await;
        let (_, tail) = json_of(
            export(
                &state,
                &reader,
                &chat,
                body["next"]["since"].as_i64().unwrap_or(since),
                body["next"]["after_id"].as_str().or(after.as_deref()),
                Some(3),
                None,
            )
            .await,
        )
        .await;
        assert_eq!(tail["messages"].as_array().unwrap().len(), 0);
        assert_eq!(tail["has_more"], false);
        assert_eq!(tail["total"], 8);
    }

    #[tokio::test]
    async fn signed_blob_url_expires_and_binds_to_the_chat() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 3007);
        let (bob, _) = person(&state, 3008);
        let chat = dm(&state, &alice, &bob);
        let dir = state
            .cfg
            .data_dir
            .join("blobs")
            .join(format!("people:{chat}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("att-1"), b"%PDF-1.4 hello").unwrap();

        let now = now_ms();
        let url = signed_blob_url(&state, &chat, "att-1", now);
        let query = url.split('?').nth(1).unwrap();
        let exp: i64 = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("exp="))
            .unwrap()
            .parse()
            .unwrap();
        let sig = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("sig="))
            .unwrap()
            .to_owned();
        assert_eq!(exp, now + BLOB_URL_TTL_MS);

        let fetch = |chat: String, key: &str, exp: i64, sig: String| {
            let state = state.clone();
            let key = key.to_owned();
            async move {
                signed_blob(
                    State(state),
                    Path((chat, key)),
                    Query(BlobQuery { exp, sig }),
                )
                .await
            }
        };
        let (status, body) = text_of(fetch(chat.clone(), "att-1", exp, sig.clone()).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "%PDF-1.4 hello");

        // Tampered signature, another chat, another key: refused.
        let (status, _) = json_of(fetch(chat.clone(), "att-1", exp, format!("{sig}0")).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = json_of(fetch(format!("{chat}x"), "att-1", exp, sig.clone()).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = json_of(fetch(chat.clone(), "att-2", exp, sig.clone()).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // A changed expiry breaks the signature; a correctly signed past expiry is gone.
        let (status, _) = json_of(fetch(chat.clone(), "att-1", exp + 1, sig).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let past = now - 1;
        let past_sig = blob_sig(state.jwt_key.as_ref(), &chat, "att-1", past);
        let (status, body) = json_of(fetch(chat.clone(), "att-1", past, past_sig).await).await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(body["error"]["type"], "expired");
    }

    #[tokio::test]
    async fn export_joins_nothing_and_writes_no_line() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (alice, _) = person(&state, 3009);
        let (bob, _) = person(&state, 3010);
        let (carol, _) = person(&state, 3011);
        let chat = dm(&state, &alice, &bob);
        seed(&state, &chat, &alice, 2);
        let room = wire::room_name(PEOPLE_PROJECT, &chat);
        let rows_before = state.db.rows_after(&room, 0, None, 500).unwrap().len();
        let members_before = state.db.members_of(&chat).unwrap().len();

        let (reader, _) = device(&state, &alice, READER_KIND);
        for format in [None, Some("md")] {
            let resp = export(&state, &reader, &chat, 0, None, None, format).await;
            assert_eq!(resp.status(), StatusCode::OK);
        }
        // A reader for someone outside the chat sees nothing.
        let (outsider, _) = device(&state, &carol, READER_KIND);
        let (status, body) =
            json_of(export(&state, &outsider, &chat, 0, None, None, None).await).await;
        assert_eq!(
            (status, body["error"]["type"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_member"))
        );

        assert_eq!(
            state.db.rows_after(&room, 0, None, 500).unwrap().len(),
            rows_before
        );
        assert_eq!(state.db.members_of(&chat).unwrap().len(), members_before);
        assert!(
            state.db.matrix_room(&chat).unwrap().is_none(),
            "no room was created"
        );
        assert!(
            state.db.matrix_outbox_due(10).unwrap().is_empty(),
            "no Matrix job was queued"
        );
        let doc = load_doc(&state, &chat, None).unwrap().unwrap();
        assert_eq!(
            doc.projected_rows().len(),
            4,
            "2 messages + system line + streaming row, unchanged"
        );
    }

    /// #2120: the first message of a brand-new people chat with an agent reaches the agent's
    /// box inside the request — announced, mirror job queued, the box woken and told to open
    /// the room — and the Matrix resync that follows is a safety net that finds nothing to do.
    #[tokio::test]
    async fn first_message_of_a_new_chat_reaches_the_agent_box_in_the_request() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (alice, alice_dev) = person(&state, 3031);
        let (bob, _) = person(&state, 3032);
        let project = project_of(&state, &alice, "Hezbola");
        // What `chats_create_group` leaves behind: the group, its humans, the agent member.
        let chat = group(&state, &[&alice, &bob]);
        state
            .db
            .add_member(
                &chat,
                &format!("agent:{project}"),
                "agent",
                &alice,
                &json!({ "reply_policy": "mention" }),
            )
            .unwrap();
        let _ = state.db.take_pushes(&project).unwrap();
        assert_eq!(state.box_state(&alice).await.0, BoxState::Asleep);

        let send = |text: &str| {
            let state = state.clone();
            let chat = chat.clone();
            let dev = alice_dev.clone();
            let body = SendBody {
                text: text.to_owned(),
                reply_to: None,
            };
            async move {
                json_of(send_message(State(state), dev, Path(chat), Some(Json(body))).await).await
            }
        };
        let (status, body) = send("@hezbola are you there?").await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let message_id = body["message_id"].as_str().unwrap().to_owned();
        let seq = body["seq"].as_i64().unwrap();

        // The row is work for the agent: a pending `send` command names it (what agentd acts
        // on; the route used to write the row alone) and the @mention resolved to the agent.
        let doc = load_doc(&state, &chat, None).unwrap().unwrap();
        let pending = doc.pending_commands();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].kind, "send");
        assert_eq!(pending[0].payload["messageId"], json!(message_id));
        let row = doc.message(&message_id).expect("row");
        assert_eq!(
            row.rest["mentions"],
            json!([format!("agent:{project}")]),
            "@hezbola resolves to the agent member"
        );
        // Announced (what drives unread, pushes and the mirror) and the mirror job is queued.
        assert!(state.db.chat_event_exists(&chat, &message_id).unwrap());
        assert!(state
            .db
            .matrix_pending_message_ids(&chat)
            .unwrap()
            .contains(&message_id));
        // The sender's own row is read by them.
        assert_eq!(state.db.user_cursor(&alice, &chat).unwrap(), seq);
        // The agent's box: woken, and told to open the room it has no socket on.
        assert_eq!(state.box_state(&alice).await.0, BoxState::Waking);
        let pushes = state.db.take_pushes(&project).unwrap();
        let opens: Vec<&Value> = pushes
            .iter()
            .filter(|(_, k, _)| k == boxes::PEOPLE_CHAT_OPEN_PUSH)
            .map(|(_, _, p)| p)
            .collect();
        assert_eq!(opens.len(), 1, "{pushes:?}");
        assert_eq!(opens[0]["chat_id"], json!(chat));
        assert_eq!(opens[0]["reason"], "rows_pending");

        // The resync (due 4 s later) is a safety net: the announced row's job is pending, so
        // it queues nothing and counts nothing as unannounced.
        let rep = backfill::resync_chat(&state, &chat, None);
        assert_eq!(
            rep,
            backfill::Resync {
                queued: 0,
                pending_skipped: 1,
                ..Default::default()
            }
        );
        assert_eq!(state.matrix.counters.json()["unannounced"], 0);
        assert_eq!(state.matrix.counters.json()["pending_skipped"], 1);

        // Two more rows before the box opens the room: one open push, not one per row.
        send("still there?").await;
        send("hello?").await;
        let pushes = state.db.take_pushes(&project).unwrap();
        assert_eq!(
            pushes
                .iter()
                .filter(|(_, k, _)| k == boxes::PEOPLE_CHAT_OPEN_PUSH)
                .count(),
            1
        );
        // With the box on the room, rows reach it live over the socket: no push.
        let room = state.room(&wire::room_name(PEOPLE_PROJECT, &chat)).await;
        room.box_joined(&alice);
        send("there you are").await;
        assert!(state.db.take_pushes(&project).unwrap().is_empty());
    }

    #[tokio::test]
    async fn posted_message_is_a_normal_row_and_exports_back() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (alice, _) = person(&state, 3012);
        let (bob, bob_dev) = person(&state, 3013);
        let (carol, carol_dev) = person(&state, 3014);
        let chat = dm(&state, &alice, &bob);
        let ids = seed(&state, &chat, &alice, 1);
        let send = |p: Principal, text: &str, reply_to: Option<&str>| {
            let state = state.clone();
            let chat = chat.clone();
            let body = SendBody {
                text: text.to_owned(),
                reply_to: reply_to.map(str::to_owned),
            };
            async move { send_message(State(state), p, Path(chat), Some(Json(body))).await }
        };
        let (status, body) =
            json_of(send(bob_dev.clone(), "  hi from the bot  ", Some(&ids[0])).await).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let posted = body["message_id"].as_str().unwrap().to_owned();
        assert!(body["seq"].as_i64().unwrap() > 0);
        assert_eq!(body["recorded"], true, "announced like an app's notify");

        let (status, _) = json_of(send(bob_dev.clone(), "   ", None).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) = json_of(send(carol_dev, "intruder", None).await).await;
        assert_eq!(
            (status, body["error"]["type"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_member"))
        );
        let _ = carol;

        // The row reads back through the export with the poster as sender and the reply link.
        let (_, body) = json_of(export(&state, &bob_dev, &chat, 0, None, None, None).await).await;
        let msgs = body["messages"].as_array().unwrap();
        let mine = msgs
            .iter()
            .find(|m| m["id"] == posted)
            .expect("posted row exported");
        assert_eq!(mine["text"], "hi from the bot");
        assert_eq!(mine["sender"]["id"], bob);
        assert_eq!(mine["sender"]["handle"], "user3013");
        assert_eq!(mine["reply_to"], ids[0]);
        // Stored as a finished user row from the posting device; the Matrix mirror got a job.
        let doc = load_doc(&state, &chat, None).unwrap().unwrap();
        let row = doc
            .projected_rows()
            .into_iter()
            .find(|r| r["id"] == posted)
            .unwrap();
        assert_eq!(row["role"], "user");
        assert_eq!(row["status"], "done");
        assert_eq!(row["sender"], format!("user:{bob}"));
        // The row itself names its author (§3): a box reading the doc must not need the DB.
        assert_eq!(row["senderHandle"], "user3013");
        let Principal::Device { id: bob_device, .. } = &bob_dev else {
            panic!("device")
        };
        assert_eq!(row["deviceId"], *bob_device);
        assert!(
            state.db.matrix_pending_kind(&chat, "resync").unwrap(),
            "the Matrix mirror was told about the new row"
        );
    }

    #[test]
    fn reader_routes() {
        assert!(reader_may("GET", "/chats"));
        assert!(reader_may("GET", "/chats/01ABC/export"));
        assert!(reader_may("GET", "/export/blob/01ABC/att-1"));
        assert!(!reader_may("POST", "/chats"));
        assert!(!reader_may("GET", "/chats/01ABC"));
        assert!(!reader_may("POST", "/chats/01ABC/notify"));
        assert!(!reader_may("POST", "/chats/01ABC/read"));
        assert!(!reader_may("GET", "/me"));
        assert!(!reader_may("GET", "/servers"));
        assert!(!reader_may("DELETE", "/auth/devices/x"));
    }

    #[test]
    fn blob_signature_binds_chat_key_and_expiry() {
        let k = [7u8; 32];
        let s = blob_sig(&k, "c1", "att-1", 10);
        assert_eq!(s, blob_sig(&k, "c1", "att-1", 10));
        assert_ne!(s, blob_sig(&k, "c2", "att-1", 10));
        assert_ne!(s, blob_sig(&k, "c1", "att-2", 10));
        assert_ne!(s, blob_sig(&k, "c1", "att-1", 11));
        assert!(constant_eq(s.as_bytes(), s.as_bytes()));
        assert!(!constant_eq(s.as_bytes(), b"nope"));
    }
}
