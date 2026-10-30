//! Servers and channels API (D88 C1). The relay is the control plane: it owns the registry,
//! the roles, the invite links and hands each device its Matrix token; the messages flow
//! between the apps and the homeserver through `/_matrix/client/*` (proxied here) and never
//! touch a Loro doc.
//!
//! Endpoints (device tokens unless noted):
//! `GET /servers` · `POST /servers {name}` · `GET /servers/{id}` · `PATCH /servers/{id}` ·
//! `DELETE /servers/{id}` · `POST /servers/{id}/channels {name, restricted?}` ·
//! `PATCH|DELETE /servers/{id}/channels/{cid}` · `POST /servers/{id}/invite/reset` ·
//! `GET /servers/search?q=` (open servers by name) · `POST /servers/join {code|server_id}` ·
//! `GET /s/{code}` (landing, no token) ·
//! `PUT /servers/{id}/members/{uid} {role}` · `DELETE /servers/{id}/members/{uid}[?ban=1]` ·
//! `POST /servers/{id}/leave` · `GET /me/matrix` (token for the SDK) ·
//! `/_matrix/client/{*rest}` (proxy to Synapse; the Matrix token authenticates there).

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

use arbos_core::api::{
    PushEventDto, ServerDeletedPayloadDto, ServerMemberJoinedPayloadDto,
    ServerMemberRemovedPayloadDto, ServerUpdatedPayloadDto, SpaceArchivedPayloadDto,
};
use arbos_core::ids::{now_ms, ulid};

use super::client::Client;
use super::servers_db::{Channel, Server, ServerInvite, ServerMember};
use crate::auth::Principal;
use crate::blobs::{no_such_image, not_an_image, stored_image, StoredImage};
use crate::signin::api_error;
use crate::state::AppState;

/// Blob slot a server's picture is uploaded to (`PUT /blob/servers:{id}/{key}`).
pub const SERVER_BLOB_PREFIX: &str = "servers:";

const NAME_MAX: usize = 60;
const CHANNEL_MAX: usize = 32;
const SERVERS_PER_DAY: i64 = 10;

fn user_of(principal: &Principal) -> Result<String, Box<Response>> {
    match principal {
        Principal::Device { user_id, .. } => Ok(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "devices only",
        ))),
    }
}

fn device_of(principal: &Principal) -> Option<String> {
    match principal {
        Principal::Device { id, .. } => Some(id.clone()),
        Principal::Box { .. } | Principal::Admin => None,
    }
}

fn matrix_required(state: &AppState) -> Result<(), Box<Response>> {
    if state.matrix.cfg.mode.writes() {
        Ok(())
    } else {
        Err(Box::new(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "servers_off",
            "servers are not enabled on this relay",
        )))
    }
}

fn invite_code() -> String {
    ulid().to_ascii_lowercase().chars().rev().take(10).collect()
}

/// Invite links live on the site (`arbos.life/s/<code>` deep-links into the app and falls back
/// to a page) when `ARBOS_SITE_URL` is set; the relay's own `/s/<code>` otherwise.
fn invite_url(state: &AppState, code: &str) -> String {
    let base = std::env::var("ARBOS_SITE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| state.cfg.public_url.clone());
    format!("{}/s/{}", base.trim_end_matches('/'), code)
}

fn invite_json(state: &AppState, i: &ServerInvite) -> Value {
    json!({
        "id": i.id, "code": i.code, "url": invite_url(state, &i.code), "created_by": i.created_by,
        "created_at": i.created_at, "expires_at": i.expires_at, "uses_left": i.uses_left,
        "usable": i.usable(now_ms()),
    })
}

/// Seven days: the only expiry the apps offer (Jacob: "optional 7-day expiry").
const INVITE_EXPIRY_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Channel names are Discord-like: lowercase, digits and dashes, no leading `#`.
fn channel_slug(name: &str) -> Option<String> {
    let slug: String = name
        .trim()
        .trim_start_matches('#')
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == '_' {
                '-'
            } else {
                c
            }
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let slug = slug.trim_matches('-').to_owned();
    (!slug.is_empty() && slug.len() <= CHANNEL_MAX).then_some(slug)
}

fn server_name(name: &str) -> Option<String> {
    let n: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!n.is_empty() && n.chars().count() <= NAME_MAX).then_some(n)
}

/// Membership notice (#1349) in the server's first channel — the one everyone lands in —
/// "Jacob added Doug", "Doug joined", "Jacob removed Doug", "Doug left".
fn server_notice(state: &AppState, server_id: &str, text: &str) {
    let mut channels = state.db.channels_of_server(server_id).unwrap_or_default();
    channels.sort_by_key(|c| c.position);
    if let Some(first) = channels.first() {
        crate::notices::post_channel_notice(state, &first.id, text);
    }
}

fn handle_of(state: &AppState, user_id: &str) -> String {
    state
        .db
        .user_card(user_id)
        .ok()
        .flatten()
        .map(|c| c.handle)
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| user_id.to_owned())
}

/// `archived` is per user (the caller's): the channel is hidden from their list, restorable
/// from the server's Channels tab; on the homeserver it is the `m.lowpriority` tag.
fn channel_json(ch: &Channel, archived: bool) -> Value {
    json!({
        "id": ch.id, "server_id": ch.server_id, "name": ch.name, "topic": ch.topic,
        "restricted": ch.restricted, "position": ch.position, "room_id": ch.room_id,
        "created_at": ch.created_at, "archived": archived,
    })
}

fn channel_json_for(state: &AppState, ch: &Channel, user_id: &str) -> Value {
    let archived = state
        .db
        .chat_archived_at(user_id, &ch.id)
        .ok()
        .flatten()
        .is_some();
    channel_json(ch, archived)
}

fn member_json(state: &AppState, m: &ServerMember) -> Value {
    let card =
        state
            .db
            .user_card(&m.user_id)
            .ok()
            .flatten()
            .unwrap_or(arbos_core::api::UserCardDto {
                id: m.user_id.clone(),
                ..Default::default()
            });
    let mut v =
        serde_json::to_value(crate::people::with_presence(state, card)).unwrap_or(Value::Null);
    v["user_id"] = json!(m.user_id);
    v["role"] = json!(m.role);
    v["joined_at"] = json!(m.joined_at);
    v["avatar_color_index_effective"] =
        json!(crate::avatar::effective_color_index(&m.user_id, None));
    v
}

/// The server as the apps see it. Admin fields (invite link) only for owner/admins.
fn server_json(state: &AppState, s: &Server, me: &ServerMember, detail: bool) -> Value {
    let channels = state.db.channels_of_server(&s.id).unwrap_or_default();
    let members = state.db.server_members(&s.id).unwrap_or_default();
    let mut v = json!({
        "id": s.id,
        "name": s.name,
        "image_key": s.image_key,
        "owner_user_id": s.owner_user_id,
        "space_room_id": s.space_room_id,
        "created_at": s.created_at,
        "open": s.is_open,
        "my_role": me.role,
        "member_count": members.len(),
        "channels": channels.iter().map(|c| channel_json_for(state, c, &me.user_id)).collect::<Vec<_>>(),
        "general_channel_id": channels.iter().find(|c| c.name == "general").or(channels.first()).map(|c| c.id.clone()),
        "avatar_color_index_effective": crate::avatar::effective_color_index(&s.id, None),
        "avatar_leaf": s.avatar_leaf,
        "avatar_leaf_color": s.avatar_leaf_color,
        "archived_at": state.db.space_archived_at(&me.user_id, &s.id).ok().flatten(),
    });
    // Invite links are the owner's fallback (Jacob 2026-10-22: demoted; Add members by name is
    // the main way in). Admins no longer see them.
    if s.owner_user_id == me.user_id {
        v["invite"] = json!({
            "code": s.invite_code,
            "url": invite_url(state, &s.invite_code),
        });
        v["invites"] = json!(state
            .db
            .invites_of_server(&s.id)
            .unwrap_or_default()
            .iter()
            .map(|i| invite_json(state, i))
            .collect::<Vec<_>>());
    }
    if detail {
        v["members"] = json!(members
            .iter()
            .map(|m| member_json(state, m))
            .collect::<Vec<_>>());
    }
    v
}

fn load(state: &AppState, me: &str, id: &str) -> Result<(Server, ServerMember), Box<Response>> {
    let Some(s) = state.db.server(id).ok().flatten() else {
        return Err(Box::new(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such server",
        )));
    };
    match state.db.server_member(id, me).ok().flatten() {
        Some(m) if m.banned_at.is_none() => Ok((s, m)),
        _ => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "not_member",
            "not in this server",
        ))),
    }
}

/// The caller as a non-banned owner / admin of the server (the avatar generate route).
pub(crate) fn admin_of(
    state: &AppState,
    principal: &Principal,
    id: &str,
) -> Result<(String, Server), Box<Response>> {
    let (me, s, _) = settings_server(state, principal, id, "server", &["avatar_generate"])?;
    Ok((me, s))
}

fn admin_required(m: &ServerMember) -> Result<(), Box<Response>> {
    if m.can_admin() {
        Ok(())
    } else {
        Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "admin_only",
            "owners and admins only",
        )))
    }
}

/// The server, if the caller may change its settings (name, picture, join rule, channels):
/// the owner or an admin. Every attempt — allowed, plain member, non-member, box, admin
/// token — leaves an audit row under `servers:{id}` (`settings_audit`).
fn settings_server(
    state: &AppState,
    principal: &Principal,
    id: &str,
    target: &str,
    fields: &[&str],
) -> Result<(String, Server, ServerMember), Box<Response>> {
    let slot = format!("{SERVER_BLOB_PREFIX}{id}");
    let me = match user_of(principal) {
        Ok(u) => u,
        Err(r) => {
            crate::settings_audit::record(
                state,
                &slot,
                target,
                id,
                principal,
                None,
                fields,
                false,
                "devices_only",
            );
            return Err(r);
        }
    };
    let (s, m) = match load(state, &me, id) {
        Ok(v) => v,
        Err(r) => {
            crate::settings_audit::record(
                state,
                &slot,
                target,
                id,
                principal,
                None,
                fields,
                false,
                "not_member",
            );
            return Err(r);
        }
    };
    let allowed = m.can_admin();
    crate::settings_audit::record(
        state,
        &slot,
        target,
        id,
        principal,
        Some(&m.role),
        fields,
        allowed,
        if allowed { "ok" } else { "admin_only" },
    );
    admin_required(&m)?;
    Ok((me, s, m))
}

fn notify_server_members(state: &AppState, server_id: &str, except: &str, event: &PushEventDto) {
    for m in state.db.server_members(server_id).unwrap_or_default() {
        if m.user_id != except {
            let _ = state.db.push_user_event(&m.user_id, event, None);
        }
    }
    let _ = state.db.push_user_event(except, event, None);
}

// ---- servers --------------------------------------------------------------------------------------

/// `GET /servers` — my servers with channels, newest first.
pub async fn list(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let mut out: Vec<Value> = state
        .db
        .servers_of_user(&me)
        .unwrap_or_default()
        .iter()
        .map(|(s, m)| server_json(&state, s, m, false))
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s["created_at"].as_i64().unwrap_or(0)));
    Json(out).into_response()
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    /// `true` (default) also makes `#announcements` (admins post).
    #[serde(default = "yes")]
    pub announcements: bool,
    /// Create screen (D93): the server's leaf picture (0–8) and disc colour (0–9), both or neither.
    #[serde(default)]
    pub avatar_leaf: Option<i64>,
    #[serde(default)]
    pub avatar_leaf_color: Option<i64>,
}

fn yes() -> bool {
    true
}

/// `POST /servers {name, announcements?}` — a new server with `#general` (and
/// `#announcements`), the caller as owner. The Space and rooms follow through the outbox.
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<CreateBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if let Err(r) = matrix_required(&state) {
        return *r;
    }
    let Some(name) = server_name(&b.name) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_name",
            "a server needs a name (≤ 60 characters)",
        );
    };
    if state
        .db
        .rate_event(&me, "server_create", 24 * 3_600_000)
        .unwrap_or(0)
        >= SERVERS_PER_DAY
    {
        return api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many servers today",
        );
    }
    if let Err((st, msg)) = crate::avatar::check_leaf_pick(b.avatar_leaf, b.avatar_leaf_color) {
        return api_error(st, "bad_leaf", msg);
    }
    let leaf = b.avatar_leaf.zip(b.avatar_leaf_color);
    let now = now_ms();
    let server = Server {
        id: ulid(),
        name,
        owner_user_id: me.clone(),
        image_key: None,
        invite_code: invite_code(),
        space_room_id: None,
        created_at: now,
        is_open: false,
        avatar_leaf: leaf.map(|l| l.0),
        avatar_leaf_color: leaf.map(|l| l.1),
    };
    if let Err(e) = state.db.insert_server(&server) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    let mut names = vec![("general", false)];
    if b.announcements {
        names.push(("announcements", true));
    }
    for (i, (n, restricted)) in names.into_iter().enumerate() {
        let ch = Channel {
            id: ulid(),
            server_id: server.id.clone(),
            name: n.to_owned(),
            topic: String::new(),
            restricted,
            position: i as i64,
            room_id: None,
            created_at: now,
        };
        if let Err(e) = state.db.insert_channel(&ch) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        state.matrix.touch_channel(&state, &ch.id);
    }
    state.matrix.touch_server(&state, &server.id);
    tracing::info!(server = %server.id, %me, "server created");
    match load(&state, &me, &server.id) {
        Ok((s, m)) => {
            (StatusCode::CREATED, Json(server_json(&state, &s, &m, true))).into_response()
        }
        Err(r) => *r,
    }
}

/// `GET /servers/{id}` — detail with members.
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match load(&state, &me, &id) {
        Ok((s, m)) => Json(server_json(&state, &s, &m, true)).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
pub struct SpaceArchiveBody {
    pub archived: bool,
}

/// `PUT /servers/{id}/archive {"archived": bool}` — hide (or show again) this server's sidebar
/// row for me on every device; membership is untouched.
pub async fn archive(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<SpaceArchiveBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if let Err(r) = load(&state, &me, &id) {
        return *r;
    }
    let at = match state.db.set_space_archived(&me, &id, "server", b.archived) {
        Ok(at) => at,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let _ = state.db.push_user_except_event(
        &me,
        &device_of(&principal).unwrap_or_default(),
        &PushEventDto::SpaceArchived(SpaceArchivedPayloadDto {
            space_id: id.clone(),
            kind: "server".into(),
            archived_at: at,
        }),
    );
    Json(json!({ "id": id, "archived_at": at })).into_response()
}

#[derive(Deserialize)]
pub struct PatchBody {
    #[serde(default)]
    pub name: Option<String>,
    /// Blob key under `servers:{id}`; JSON `null` clears, absent leaves it.
    #[serde(default)]
    pub image_key: Option<Value>,
    /// `true` = anyone on Arbos can search and join without an invite link.
    #[serde(default)]
    pub open: Option<bool>,
    /// D92 amendment: the server's leaf picture (`null` = none); set together.
    #[serde(
        default,
        deserialize_with = "crate::api::deserialize_double_option_i64"
    )]
    pub avatar_leaf: Option<Option<i64>>,
    #[serde(
        default,
        deserialize_with = "crate::api::deserialize_double_option_i64"
    )]
    pub avatar_leaf_color: Option<Option<i64>>,
}

/// `PATCH /servers/{id} {name?, image_key?, open?}` — owner/admins.
pub async fn patch(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<PatchBody>,
) -> Response {
    let mut fields = Vec::new();
    if b.name.is_some() {
        fields.push("name");
    }
    if b.image_key.is_some() {
        fields.push("image_key");
    }
    if b.open.is_some() {
        fields.push("open");
    }
    if b.avatar_leaf.is_some() || b.avatar_leaf_color.is_some() {
        fields.push("avatar_leaf");
    }
    let (me, _, _) = match settings_server(&state, &principal, &id, "server", &fields) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let name = match b.name.as_deref().map(server_name) {
        Some(None) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_name",
                "a server needs a name (≤ 60 characters)",
            )
        }
        Some(Some(n)) => Some(n),
        None => None,
    };
    let image: Option<Option<String>> = match &b.image_key {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(k))
            if k.len() <= 64
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') =>
        {
            Some(Some(k.clone()))
        }
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "image_key must be a short blob object name",
            )
        }
    };
    // The picture is judged on what was uploaded (Content-Type and bytes), not its name.
    if let Some(Some(key)) = &image {
        match stored_image(&state, &format!("{SERVER_BLOB_PREFIX}{id}"), key).await {
            StoredImage::Image(kind) => {
                tracing::info!(server = %id, %key, %kind, "server picture set");
            }
            StoredImage::Missing => return no_such_image(),
            StoredImage::NotAnImage(why) => return not_an_image(&why),
        }
    }
    if let Err(e) = state.db.update_server(
        &id,
        name.as_deref(),
        image.as_ref().map(|o| o.as_deref()),
        b.open,
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    if b.avatar_leaf.is_some() || b.avatar_leaf_color.is_some() {
        let leaf = match (b.avatar_leaf, b.avatar_leaf_color) {
            (Some(Some(l)), Some(Some(c))) => {
                if let Err((st, msg)) = crate::avatar::check_leaf_pick(Some(l), Some(c)) {
                    return api_error(st, "bad_leaf", msg);
                }
                Some((l, c))
            }
            (Some(Some(_)), _) | (_, Some(Some(_))) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_leaf",
                    "avatar_leaf and avatar_leaf_color go together",
                )
            }
            _ => None,
        };
        if let Err(e) = state.db.update_server_leaf(&id, leaf) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
    }
    state.matrix.touch_server(&state, &id);
    notify_server_members(
        &state,
        &id,
        &me,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: id.clone(),
            channel_id: None,
            deleted: None,
            user_id: None,
            role: None,
        }),
    );
    match load(&state, &me, &id) {
        Ok((s, m)) => Json(server_json(&state, &s, &m, true)).into_response(),
        Err(r) => *r,
    }
}

/// `DELETE /servers/{id}` — owner only; everyone loses it, the rooms are closed.
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner deletes a server",
        );
    }
    let members = state.db.server_members(&id).unwrap_or_default();
    for ch in state.db.channels_of_server(&id).unwrap_or_default() {
        let _ = state.db.delete_channel(&ch.id);
        state.matrix.close_chat(&state, &ch.id);
    }
    if let Err(e) = state.db.delete_server(&id) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.close_chat(&state, &id);
    let event = PushEventDto::ServerDeleted(ServerDeletedPayloadDto {
        server_id: id.clone(),
    });
    for m in members {
        let _ = state.db.push_user_event(&m.user_id, &event, None);
    }
    StatusCode::NO_CONTENT.into_response()
}

// ---- channels -------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ChannelBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub restricted: Option<bool>,
}

/// `POST /servers/{id}/channels {name, topic?, restricted?}` — owner/admins.
pub async fn channel_create(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ChannelBody>,
) -> Response {
    let (me, _, _) = match settings_server(&state, &principal, &id, "channel", &["channel_create"])
    {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(name) = b.name.as_deref().and_then(channel_slug) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_name",
            "a channel name is letters, digits and dashes (≤ 32)",
        );
    };
    if state
        .db
        .channels_of_server(&id)
        .unwrap_or_default()
        .iter()
        .any(|c| c.name == name)
    {
        return api_error(
            StatusCode::CONFLICT,
            "name_taken",
            "a channel with that name exists",
        );
    }
    let ch = Channel {
        id: ulid(),
        server_id: id.clone(),
        name,
        topic: b
            .topic
            .unwrap_or_default()
            .trim()
            .chars()
            .take(200)
            .collect(),
        restricted: b.restricted.unwrap_or(false),
        position: state.db.next_channel_position(&id).unwrap_or(0),
        room_id: None,
        created_at: now_ms(),
    };
    if let Err(e) = state.db.insert_channel(&ch) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.touch_channel(&state, &ch.id);
    notify_server_members(
        &state,
        &id,
        &me,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: id.clone(),
            channel_id: Some(ch.id.clone()),
            deleted: None,
            user_id: None,
            role: None,
        }),
    );
    (StatusCode::CREATED, Json(channel_json(&ch, false))).into_response()
}

/// `PATCH /servers/{id}/channels/{cid} {name?, topic?, restricted?}` — owner/admins.
pub async fn channel_patch(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, cid)): Path<(String, String)>,
    Json(b): Json<ChannelBody>,
) -> Response {
    let mut fields = Vec::new();
    if b.name.is_some() {
        fields.push("name");
    }
    if b.topic.is_some() {
        fields.push("topic");
    }
    if b.restricted.is_some() {
        fields.push("restricted");
    }
    let (me, _, _) = match settings_server(&state, &principal, &id, "channel", &fields) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(ch) = state
        .db
        .channel(&cid)
        .ok()
        .flatten()
        .filter(|c| c.server_id == id)
    else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such channel");
    };
    let name = match b.name.as_deref().map(channel_slug) {
        Some(None) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_name",
                "a channel name is letters, digits and dashes (≤ 32)",
            )
        }
        Some(Some(n)) => Some(n),
        None => None,
    };
    let topic: Option<String> = b.topic.map(|t| t.trim().chars().take(200).collect());
    if let Err(e) = state
        .db
        .update_channel(&ch.id, name.as_deref(), topic.as_deref(), b.restricted)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.touch_channel(&state, &ch.id);
    notify_server_members(
        &state,
        &id,
        &me,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: id.clone(),
            channel_id: Some(ch.id.clone()),
            deleted: None,
            user_id: None,
            role: None,
        }),
    );
    match state.db.channel(&ch.id) {
        Ok(Some(c)) => Json(channel_json_for(&state, &c, &me)).into_response(),
        _ => api_error(StatusCode::NOT_FOUND, "not_found", "no such channel"),
    }
}

#[derive(Deserialize)]
pub struct ArchiveBody {
    pub archived: bool,
}

/// `POST /servers/{id}/channels/{cid}/archive {archived}` — any member: hide the channel from
/// my list (or bring it back). Per user; mirrored as the room's `m.lowpriority` tag so every
/// device of mine agrees. The caller's other devices get a `server_updated` push.
pub async fn channel_archive(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, cid)): Path<(String, String)>,
    Json(b): Json<ArchiveBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if let Err(r) = load(&state, &me, &id) {
        return *r;
    }
    let Some(ch) = state
        .db
        .channel(&cid)
        .ok()
        .flatten()
        .filter(|c| c.server_id == id)
    else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such channel");
    };
    if let Err(e) = state.db.set_chat_archived(&me, &ch.id, b.archived) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.archive(&state, &ch.id, &me, b.archived);
    let _ = state.db.push_user_event(
        &me,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: id.clone(),
            channel_id: Some(ch.id.clone()),
            deleted: None,
            user_id: None,
            role: None,
        }),
        None,
    );
    Json(channel_json(&ch, b.archived)).into_response()
}

/// `DELETE /servers/{id}/channels/{cid}` — owner/admins; `#general` cannot be deleted.
pub async fn channel_delete(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, cid)): Path<(String, String)>,
) -> Response {
    let (me, _, _) = match settings_server(&state, &principal, &id, "channel", &["channel_delete"])
    {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let Some(ch) = state
        .db
        .channel(&cid)
        .ok()
        .flatten()
        .filter(|c| c.server_id == id)
    else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such channel");
    };
    if ch.name == "general" {
        return api_error(
            StatusCode::CONFLICT,
            "general_stays",
            "#general cannot be deleted",
        );
    }
    let _ = state.db.delete_channel(&ch.id);
    state.matrix.close_chat(&state, &ch.id);
    state.matrix.touch_server(&state, &id);
    notify_server_members(
        &state,
        &id,
        &me,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: id.clone(),
            channel_id: Some(ch.id.clone()),
            deleted: Some(true),
            user_id: None,
            role: None,
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

// ---- invites and membership -------------------------------------------------------------------------

/// `POST /servers/{id}/invite/reset` — owner: the old link stops working.
pub async fn invite_reset(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner resets the invite link",
        );
    }
    let _ = state.db.revoke_all_invites(&id);
    let inv = ServerInvite {
        id: ulid(),
        server_id: id.clone(),
        code: invite_code(),
        created_by: me.clone(),
        created_at: now_ms(),
        expires_at: None,
        uses_left: None,
        revoked_at: None,
    };
    if let Err(e) = state
        .db
        .insert_server_invite(&inv)
        .and_then(|_| state.db.set_server_invite(&id, &inv.code))
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    Json(json!({ "code": inv.code, "url": invite_url(&state, &inv.code) })).into_response()
}

#[derive(Deserialize, Default)]
pub struct InviteBody {
    /// Expire after seven days.
    #[serde(default)]
    pub expires: bool,
    /// Number of joins the link allows; absent = unlimited.
    #[serde(default)]
    pub max_uses: Option<i64>,
}

/// `GET /servers/{id}/invites` — the owner's links (live ones; expired / used-up still listed).
pub async fn invites_list(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner sees invite links",
        );
    }
    let list: Vec<Value> = state
        .db
        .invites_of_server(&id)
        .unwrap_or_default()
        .iter()
        .map(|i| invite_json(&state, i))
        .collect();
    Json(json!({ "invites": list })).into_response()
}

/// `POST /servers/{id}/invites {expires?, max_uses?}` — the owner mints another link.
pub async fn invites_create(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    body: Option<Json<InviteBody>>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner makes invite links",
        );
    }
    let b = body.map(|b| b.0).unwrap_or_default();
    let max_uses = b.max_uses.filter(|n| *n > 0).map(|n| n.min(1000));
    let inv = ServerInvite {
        id: ulid(),
        server_id: id.clone(),
        code: invite_code(),
        created_by: me.clone(),
        created_at: now_ms(),
        expires_at: b.expires.then(|| now_ms() + INVITE_EXPIRY_MS),
        uses_left: max_uses,
        revoked_at: None,
    };
    if let Err(e) = state.db.insert_server_invite(&inv) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    if inv.expires_at.is_none() && inv.uses_left.is_none() {
        // Older clients read one code off the server: the newest unlimited one.
        let _ = state.db.set_server_invite(&id, &inv.code);
    }
    (StatusCode::CREATED, Json(invite_json(&state, &inv))).into_response()
}

/// `DELETE /servers/{id}/invites/{iid}` — the owner revokes a link; it stops working at once.
pub async fn invites_revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, iid)): Path<(String, String)>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner revokes invite links",
        );
    }
    match state.db.revoke_server_invite(&id, &iid) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "not_found", "no such invite"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct AddMemberBody {
    pub user_id: String,
}

/// `POST /servers/{id}/members {user_id}` — owner/admin adds someone on Arbos by name (the
/// main way in since 2026-10-22; the picker is the groups' Add members). Banned people need
/// an unban (remove the ban) first.
pub async fn member_add(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<AddMemberBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, m) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(r) = admin_required(&m) {
        return *r;
    }
    let user_id = b.user_id.trim().to_owned();
    if state.db.user(&user_id).ok().flatten().is_none() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such person");
    }
    if let Ok(Some(existing)) = state.db.server_member(&s.id, &user_id) {
        if existing.banned_at.is_none() {
            return Json(server_json(&state, &s, &m, true)).into_response();
        }
    }
    match state.db.add_server_member(&s.id, &user_id, &me) {
        Ok(true) => {}
        Ok(false) => {
            return api_error(
                StatusCode::CONFLICT,
                "banned",
                "they were banned from this server",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_server(&state, &s.id);
    for ch in state.db.channels_of_server(&s.id).unwrap_or_default() {
        state.matrix.touch_channel(&state, &ch.id);
    }
    let joined = PushEventDto::ServerMemberJoined(ServerMemberJoinedPayloadDto {
        server_id: s.id.clone(),
        user_id: user_id.clone(),
        handle: handle_of(&state, &user_id),
    });
    notify_server_members(&state, &s.id, &me, &joined);
    if let Some(line) = crate::notices::added_line(
        &crate::notices::person_name(&state, &me),
        &[crate::notices::person_name(&state, &user_id)],
    ) {
        server_notice(&state, &s.id, &line);
    }
    // The newcomer's own devices learn about the server right away.
    let _ = state.db.push_user_event(
        &user_id,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: s.id.clone(),
            channel_id: None,
            deleted: None,
            user_id: Some(user_id.clone()),
            role: Some("member".into()),
        }),
        None,
    );
    Json(server_json(&state, &s, &m, true)).into_response()
}

#[derive(Deserialize)]
pub struct JoinBody {
    /// Invite link or its code (invite-only and open servers).
    #[serde(default)]
    pub code: Option<String>,
    /// Open server id — join without an invite when `open` is true.
    #[serde(default)]
    pub server_id: Option<String>,
}

/// `POST /servers/join {code}` or `{server_id}` — invite link, or an open server by id
/// (banned people cannot).
pub async fn join(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<JoinBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let mut used_invite: Option<ServerInvite> = None;
    let s = if let Some(id) = b
        .server_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let Some(s) = state.db.server(id).ok().flatten() else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "no such server");
        };
        if !s.is_open {
            return api_error(
                StatusCode::FORBIDDEN,
                "invite_required",
                "this server needs an invite link",
            );
        }
        s
    } else {
        let code = b
            .code
            .as_deref()
            .unwrap_or("")
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        if code.is_empty() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "paste an invite link or pick an open server",
            );
        }
        let Some(inv) = state
            .db
            .invite_by_code(&code)
            .ok()
            .flatten()
            .filter(|i| i.usable(now_ms()))
        else {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "that invite link is not valid any more",
            );
        };
        let Some(s) = state.db.server(&inv.server_id).ok().flatten() else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "no such server");
        };
        used_invite = Some(inv);
        s
    };
    // Already a member: return the server (search → open, or re-tapping Join).
    if let Ok(Some(m)) = state.db.server_member(&s.id, &me) {
        if m.banned_at.is_none() {
            return Json(server_json(&state, &s, &m, true)).into_response();
        }
    }
    if let Some(inv) = &used_invite {
        if inv.uses_left.is_some() && !state.db.consume_invite_use(&inv.id).unwrap_or(false) {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "that invite link has been used up",
            );
        }
    }
    match state.db.add_server_member(&s.id, &me, &me) {
        Ok(true) => {}
        Ok(false) => {
            return api_error(
                StatusCode::FORBIDDEN,
                "banned",
                "you cannot join this server",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_server(&state, &s.id);
    for ch in state.db.channels_of_server(&s.id).unwrap_or_default() {
        state.matrix.touch_channel(&state, &ch.id);
    }
    notify_server_members(
        &state,
        &s.id,
        &me,
        &PushEventDto::ServerMemberJoined(ServerMemberJoinedPayloadDto {
            server_id: s.id.clone(),
            user_id: me.clone(),
            handle: handle_of(&state, &me),
        }),
    );
    server_notice(
        &state,
        &s.id,
        &format!("{} joined", crate::notices::person_name(&state, &me)),
    );
    match load(&state, &me, &s.id) {
        Ok((s, m)) => Json(server_json(&state, &s, &m, true)).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
}

const OPEN_SEARCH_LIMIT: i64 = 20;

/// Slim open-server hit for New Message search (not a member yet — no invite, no channels).
fn open_hit_json(s: &Server, member_count: i64) -> Value {
    json!({
        "id": s.id,
        "name": s.name,
        "image_key": s.image_key,
        "open": true,
        "member_count": member_count,
        "avatar_color_index_effective": crate::avatar::effective_color_index(&s.id, None),
        "avatar_leaf": s.avatar_leaf,
        "avatar_leaf_color": s.avatar_leaf_color,
        "my_role": "",
        "channels": [],
    })
}

/// `GET /servers/search?q=` — open servers by name so anyone can join without an invite.
pub async fn search(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<SearchQuery>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if let Err(r) = matrix_required(&state) {
        return *r;
    }
    let needle = q.q.trim();
    if needle.chars().count() < 2 {
        return Json(Value::Array(Vec::new())).into_response();
    }
    let mine: std::collections::HashSet<String> = state
        .db
        .servers_of_user(&me)
        .unwrap_or_default()
        .into_iter()
        .map(|(s, _)| s.id)
        .collect();
    let hits: Vec<Value> = state
        .db
        .search_open_servers(needle, OPEN_SEARCH_LIMIT)
        .unwrap_or_default()
        .into_iter()
        .filter(|(s, _)| !mine.contains(&s.id))
        .map(|(s, n)| open_hit_json(&s, n))
        .collect();
    Json(hits).into_response()
}

/// `GET /s/{code}` — what an invite link opens: JSON for the apps, a page for browsers.
/// `GET /s/{code}` — the invite link. JSON for the apps (server name, member count, whether
/// the link still works); for a browser an HTML page that first tries the installed app
/// (`arbos://join/<code>`) and otherwise explains how to join, with the download.
pub async fn landing(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    let inv = state.db.invite_by_code(&code).ok().flatten();
    let s = inv
        .as_ref()
        .and_then(|i| state.db.server(&i.server_id).ok().flatten());
    let (Some(inv), Some(s)) = (inv, s) else {
        return if wants_html {
            page("<p>This invite link is not valid.</p>")
        } else {
            api_error(StatusCode::NOT_FOUND, "not_found", "no such invite")
        };
    };
    let usable = inv.usable(now_ms());
    let members = state.db.server_members(&s.id).map(|m| m.len()).unwrap_or(0);
    let download = format!(
        "{}/download/mac",
        state.cfg.public_url.trim_end_matches('/')
    );
    if !wants_html {
        return Json(json!({
            "code": code, "usable": usable, "expires_at": inv.expires_at, "uses_left": inv.uses_left,
            "server": { "id": s.id, "name": s.name, "member_count": members, "image_key": s.image_key },
            "owner_handle": handle_of(&state, &s.owner_user_id),
            "download_url": download,
        }))
        .into_response();
    }
    if !usable {
        let why = if inv.revoked_at.is_some() {
            "was revoked by the server's owner"
        } else if inv.uses_left == Some(0) {
            "has been used up"
        } else {
            "has expired"
        };
        return page(&format!(
            "<p>This invite to <strong>{}</strong> {why}. Ask someone in the server to add you, or for a new link.</p>",
            html_escape(&s.name)
        ));
    }
    let safe_code = html_escape(&code);
    // The site's CSP allows no inline script: a meta refresh hands the link to the installed
    // app (the browser asks "Open in Arbos?"); the button does the same on a tap.
    page_with_head(
        &format!("<meta http-equiv=\"refresh\" content=\"0;url=arbos://join/{safe_code}\">"),
        &format!(
            "<p>You are invited to the server <strong>{name}</strong> on Arbos ({members} members).</p>\
             <p><a class=\"btn\" href=\"arbos://join/{safe_code}\">Open in Arbos</a></p>\
             <p>No Arbos yet? <a href=\"{download}\">Download for Mac</a>, then open this link again.</p>",
            name = html_escape(&s.name),
        ),
    )
}

fn page(body: &str) -> Response {
    page_with_head("", body)
}

fn page_with_head(head: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Arbos server invite</title>{head}\
         <style>body{{font:16px/1.5 -apple-system,system-ui,sans-serif;background:#0b0b0d;color:#e8e8ea;display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}}\
         main{{max-width:28rem;padding:2rem}}h1{{font-size:1.4rem;margin:0 0 1rem}}.btn{{display:inline-block;background:#e8e8ea;color:#0b0b0d;padding:.6rem 1rem;border-radius:.6rem;text-decoration:none;font-weight:600}}</style></head>\
         <body><main><h1>Arbos</h1>{body}</main></body></html>"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Deserialize)]
pub struct RoleBody {
    pub role: String,
}

/// `PUT /servers/{id}/members/{uid} {role: admin|member}` — owner only.
pub async fn member_role(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, uid)): Path<(String, String)>,
    Json(b): Json<RoleBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner changes roles",
        );
    }
    if uid == me || !matches!(b.role.as_str(), "admin" | "member") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_role",
            "role is admin or member, for someone else",
        );
    }
    match state.db.set_server_role(&id, &uid, &b.role) {
        Ok(true) => {}
        Ok(false) => return api_error(StatusCode::NOT_FOUND, "not_found", "not a member"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_server(&state, &id);
    for ch in state.db.channels_of_server(&id).unwrap_or_default() {
        state.matrix.touch_channel(&state, &ch.id);
    }
    notify_server_members(
        &state,
        &id,
        &me,
        &PushEventDto::ServerUpdated(ServerUpdatedPayloadDto {
            server_id: id.clone(),
            channel_id: None,
            deleted: None,
            user_id: Some(uid),
            role: Some(b.role),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// `DELETE /servers/{id}/members/{uid}[?ban=1]` — owner/admins remove (or ban) a member;
/// admins cannot remove the owner or other admins.
pub async fn member_remove(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, uid)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, m) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(r) = admin_required(&m) {
        return *r;
    }
    if uid == me {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "use leave to remove yourself",
        );
    }
    if uid == s.owner_user_id {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_stays",
            "the owner cannot be removed",
        );
    }
    let target = state.db.server_member(&id, &uid).ok().flatten();
    if m.role == "admin" && target.as_ref().is_some_and(|t| t.role == "admin") {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner removes an admin",
        );
    }
    let ban = q.get("ban").is_some_and(|b| b == "1");
    match state.db.remove_server_member(&id, &uid, ban) {
        Ok(true) => {}
        Ok(false) => return api_error(StatusCode::NOT_FOUND, "not_found", "not a member"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_server(&state, &id);
    for ch in state.db.channels_of_server(&id).unwrap_or_default() {
        state.matrix.touch_channel(&state, &ch.id);
    }
    let event = PushEventDto::ServerMemberRemoved(ServerMemberRemovedPayloadDto {
        server_id: id.clone(),
        user_id: uid.clone(),
        banned: Some(ban),
        handle: Some(handle_of(&state, &uid)),
    });
    let _ = state.db.push_user_event(&uid, &event, None);
    notify_server_members(&state, &id, &me, &event);
    if let Some(line) = crate::notices::removed_line(
        &crate::notices::person_name(&state, &me),
        &[crate::notices::person_name(&state, &uid)],
    ) {
        server_notice(&state, &id, &line);
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /servers/{id}/leave` — members and admins; the owner deletes instead.
pub async fn leave(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (s, _) = match load(&state, &me, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if s.owner_user_id == me {
        return api_error(
            StatusCode::CONFLICT,
            "owner_stays",
            "the owner deletes the server instead of leaving it",
        );
    }
    let _ = state.db.remove_server_member(&id, &me, false);
    state.matrix.touch_server(&state, &id);
    for ch in state.db.channels_of_server(&id).unwrap_or_default() {
        state.matrix.touch_channel(&state, &ch.id);
    }
    notify_server_members(
        &state,
        &id,
        &me,
        &PushEventDto::ServerMemberRemoved(ServerMemberRemovedPayloadDto {
            server_id: id.clone(),
            user_id: me.clone(),
            banned: None,
            handle: Some(handle_of(&state, &me)),
        }),
    );
    server_notice(
        &state,
        &id,
        &crate::notices::left_line(&crate::notices::person_name(&state, &me)),
    );
    StatusCode::NO_CONTENT.into_response()
}

// ---- the device's Matrix identity and the proxy ---------------------------------------------------

/// `GET /me/matrix` → `{homeserver, user_id, access_token, device_id}` for this device. The
/// token is minted once through the appservice login (`m.login.application_service`), stored
/// encrypted, and handed back on every call; sign-out revokes it with the device.
pub async fn me_matrix(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let Some(device_id) = device_of(&principal) else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    if let Err(r) = matrix_required(&state) {
        return *r;
    }
    let homeserver = state.cfg.public_url.trim_end_matches('/').to_owned();
    let mxid = state.matrix.cfg.user_mxid(&me);
    if let Ok(Some((mx_device, enc))) = state.db.matrix_device(&device_id) {
        if let Ok(token) = state.relay_key.decrypt(&enc) {
            return Json(json!({ "homeserver": homeserver, "user_id": mxid, "access_token": token, "device_id": mx_device }))
                .into_response();
        }
    }
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    if let Err(e) = client.register(&mxid).await {
        return api_error(StatusCode::BAD_GATEWAY, "homeserver", e.to_string());
    }
    let (token, mx_device) = match client.login_appservice(&mxid, &device_id).await {
        Ok(v) => v,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, "homeserver", e.to_string()),
    };
    let enc = match state.relay_key.encrypt(&token) {
        Ok(e) => e,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "crypto", e.to_string()),
    };
    if let Err(e) = state
        .db
        .matrix_insert_device(&device_id, &me, &mxid, &mx_device, &enc)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    // A device that can read channels can be told about them: the relay becomes this user's
    // push gateway (every live device; reads.rs `on_notify` fans channel messages out).
    state.matrix.pushers(&state, &me, true);
    Json(json!({ "homeserver": homeserver, "user_id": mxid, "access_token": token, "device_id": mx_device }))
        .into_response()
}

/// A signed-out Arbos device takes its Matrix device with it (`/logout` with its own token).
pub async fn revoke_device(state: &AppState, device_id: &str) {
    let Ok(Some((_, enc))) = state.db.matrix_delete_device(device_id) else {
        return;
    };
    let Ok(token) = state.relay_key.decrypt(&enc) else {
        return;
    };
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    if let Err(e) = client.logout_token(&token).await {
        tracing::warn!(device = %device_id, error = %e, "matrix: device logout failed");
    }
}

/// `/_matrix/client/{*rest}` → Synapse, body and headers streamed both ways. The Matrix access
/// token in the request authenticates at the homeserver; the relay adds nothing.
pub async fn proxy(State(state): State<AppState>, req: Request) -> Response {
    if !state.matrix.cfg.mode.writes() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no homeserver");
    }
    let path_q = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let url = format!("{}{}", state.matrix.cfg.hs_url, path_q);
    let method = req.method().clone();
    let mut headers = req.headers().clone();
    for h in [
        header::HOST,
        header::CONNECTION,
        header::CONTENT_LENGTH,
        header::TRANSFER_ENCODING,
    ] {
        headers.remove(h);
    }
    let body = match axum::body::to_bytes(req.into_body(), 64 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "bad_request", e.to_string()),
    };
    let upstream = state
        .http
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await;
    match upstream {
        Ok(r) => {
            let status = r.status();
            let mut out = Response::builder().status(status);
            for (k, v) in r.headers() {
                if k != header::TRANSFER_ENCODING && k != header::CONNECTION {
                    out = out.header(k, v);
                }
            }
            out.body(Body::from_stream(r.bytes_stream()))
                .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(e) => api_error(StatusCode::BAD_GATEWAY, "homeserver", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_slugs() {
        assert_eq!(
            channel_slug("# General Chat").as_deref(),
            Some("general-chat")
        );
        assert_eq!(channel_slug("dev_ops").as_deref(), Some("dev-ops"));
        assert_eq!(channel_slug("--").as_deref(), None);
        assert_eq!(channel_slug("").as_deref(), None);
        assert_eq!(
            server_name("  Launch   Crew ").as_deref(),
            Some("Launch Crew")
        );
        assert!(server_name("").is_none());
        assert_eq!(invite_code().len(), 10);
    }
}

#[cfg(test)]
mod settings_permission_tests {
    //! Server and channel settings (name, picture, join rule; channel create / rename /
    //! topic / restricted / delete) are the owner's and the admins'. Plain members, boxes,
    //! strangers and the admin token get 403; every attempt is audited under `servers:{id}`.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use super::*;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    struct Cast {
        server: String,
        channel: String,
        owner: Principal,
        admin: Principal,
        member: Principal,
        agent: Principal,
        stranger: Principal,
    }

    fn cast(state: &AppState, seed: i64) -> Cast {
        let (owner, owner_dev) = person(state, seed);
        let (admin, admin_dev) = person(state, seed + 1);
        let (member, member_dev) = person(state, seed + 2);
        let (_, stranger_dev) = person(state, seed + 3);
        let server = Server {
            id: ulid(),
            name: "Launch Crew".into(),
            owner_user_id: owner.clone(),
            image_key: None,
            invite_code: format!("crew{seed}"),
            space_room_id: None,
            created_at: now_ms(),
            is_open: false,
            avatar_leaf: None,
            avatar_leaf_color: None,
        };
        state.db.insert_server(&server).unwrap();
        state
            .db
            .add_server_member(&server.id, &admin, &owner)
            .unwrap();
        state
            .db
            .set_server_role(&server.id, &admin, "admin")
            .unwrap();
        state
            .db
            .add_server_member(&server.id, &member, &owner)
            .unwrap();
        let channel = Channel {
            id: ulid(),
            server_id: server.id.clone(),
            name: "dev".into(),
            topic: "builds".into(),
            restricted: false,
            position: 1,
            room_id: None,
            created_at: now_ms(),
        };
        state.db.insert_channel(&channel).unwrap();
        Cast {
            server: server.id,
            channel: channel.id,
            owner: owner_dev,
            admin: admin_dev,
            member: member_dev,
            agent: Principal::Box { user_id: member },
            stranger: stranger_dev,
        }
    }

    fn patch_body() -> PatchBody {
        PatchBody {
            name: None,
            image_key: None,
            open: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
        }
    }

    fn channel_body() -> ChannelBody {
        ChannelBody {
            name: None,
            topic: None,
            restricted: None,
        }
    }

    async fn server_patch(state: &AppState, who: &Principal, id: &str, b: PatchBody) -> StatusCode {
        patch(
            State(state.clone()),
            who.clone(),
            Path(id.to_owned()),
            Json(b),
        )
        .await
        .status()
    }

    async fn ch_create(state: &AppState, who: &Principal, id: &str, name: &str) -> StatusCode {
        channel_create(
            State(state.clone()),
            who.clone(),
            Path(id.to_owned()),
            Json(ChannelBody {
                name: Some(name.into()),
                ..channel_body()
            }),
        )
        .await
        .status()
    }

    async fn ch_patch(state: &AppState, who: &Principal, c: &Cast, b: ChannelBody) -> StatusCode {
        channel_patch(
            State(state.clone()),
            who.clone(),
            Path((c.server.clone(), c.channel.clone())),
            Json(b),
        )
        .await
        .status()
    }

    async fn ch_delete(state: &AppState, who: &Principal, c: &Cast) -> StatusCode {
        channel_delete(
            State(state.clone()),
            who.clone(),
            Path((c.server.clone(), c.channel.clone())),
        )
        .await
        .status()
    }

    #[tokio::test]
    async fn server_and_channel_settings_are_owner_and_admin_only() {
        let state = test_state(AuthMode::Github);
        let c = cast(&state, 8100);
        let denied: [(&str, &Principal); 4] = [
            ("member", &c.member),
            ("agent", &c.agent),
            ("stranger", &c.stranger),
            ("admin-token", &Principal::Admin),
        ];
        for (who, p) in denied {
            for (field, b) in [
                (
                    "name",
                    PatchBody {
                        name: Some("Hijacked".into()),
                        ..patch_body()
                    },
                ),
                (
                    "open",
                    PatchBody {
                        open: Some(true),
                        ..patch_body()
                    },
                ),
                (
                    "avatar_leaf",
                    PatchBody {
                        avatar_leaf: Some(Some(1)),
                        avatar_leaf_color: Some(Some(2)),
                        ..patch_body()
                    },
                ),
            ] {
                assert_eq!(
                    server_patch(&state, p, &c.server, b).await,
                    StatusCode::FORBIDDEN,
                    "{who} changing server {field}"
                );
            }
            assert_eq!(
                ch_create(&state, p, &c.server, "spam").await,
                StatusCode::FORBIDDEN,
                "{who} creating a channel"
            );
            for (field, b) in [
                (
                    "name",
                    ChannelBody {
                        name: Some("renamed".into()),
                        ..channel_body()
                    },
                ),
                (
                    "topic",
                    ChannelBody {
                        topic: Some("hijacked".into()),
                        ..channel_body()
                    },
                ),
                (
                    "restricted",
                    ChannelBody {
                        restricted: Some(true),
                        ..channel_body()
                    },
                ),
            ] {
                assert_eq!(
                    ch_patch(&state, p, &c, b).await,
                    StatusCode::FORBIDDEN,
                    "{who} changing channel {field}"
                );
            }
            assert_eq!(
                ch_delete(&state, p, &c).await,
                StatusCode::FORBIDDEN,
                "{who} deleting a channel"
            );
        }
        let s = state.db.server(&c.server).unwrap().unwrap();
        assert_eq!(s.name, "Launch Crew");
        assert!(!s.is_open);
        let ch = state.db.channel(&c.channel).unwrap().unwrap();
        assert_eq!(
            (ch.name.as_str(), ch.topic.as_str(), ch.restricted),
            ("dev", "builds", false)
        );
        assert_eq!(state.db.channels_of_server(&c.server).unwrap().len(), 1);
        let slot = format!("{SERVER_BLOB_PREFIX}{}", c.server);
        let rows = state.db.audit(&slot, 1000).unwrap();
        assert_eq!(rows.len(), 4 * 8);
        assert!(rows.iter().all(|r| r["allowed"] == false));
        assert_eq!(
            rows.iter().filter(|r| r["reason"] == "admin_only").count(),
            8
        );

        // Admin and owner: every one of those goes through.
        assert_eq!(
            server_patch(
                &state,
                &c.admin,
                &c.server,
                PatchBody {
                    name: Some("Launch Crew II".into()),
                    ..patch_body()
                }
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            server_patch(
                &state,
                &c.owner,
                &c.server,
                PatchBody {
                    open: Some(true),
                    ..patch_body()
                }
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            ch_create(&state, &c.admin, &c.server, "ops").await,
            StatusCode::CREATED
        );
        assert_eq!(
            ch_patch(
                &state,
                &c.owner,
                &c,
                ChannelBody {
                    topic: Some("nightly builds".into()),
                    ..channel_body()
                }
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            ch_delete(&state, &c.admin, &c).await,
            StatusCode::NO_CONTENT
        );
        let s = state.db.server(&c.server).unwrap().unwrap();
        assert_eq!((s.name.as_str(), s.is_open), ("Launch Crew II", true));
        let rows = state.db.audit(&slot, 1000).unwrap();
        assert_eq!(rows.iter().filter(|r| r["allowed"] == true).count(), 5);
        assert!(rows
            .iter()
            .any(|r| r["allowed"] == true && r["fields"] == json!(["channel_delete"])));
    }
}

#[cfg(test)]
mod picture_tests {
    //! `PATCH /servers/{id} {image_key}` runs the same picture gate as groups.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use super::*;
    use crate::image_sniff::samples;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// A blob as `PUT /blob/servers:{id}/{key}` would leave it: the bytes plus the
    /// `.mime` sidecar.
    fn store(state: &AppState, server: &str, key: &str, mime: Option<&str>, bytes: &[u8]) {
        let dir = state
            .cfg
            .data_dir
            .join("blobs")
            .join(format!("{SERVER_BLOB_PREFIX}{server}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(key), bytes).unwrap();
        if let Some(m) = mime {
            std::fs::write(dir.join(key).with_extension("mime"), m).unwrap();
        }
    }

    async fn set_picture(
        state: &AppState,
        who: &Principal,
        server: &str,
        key: Value,
    ) -> StatusCode {
        patch(
            State(state.clone()),
            who.clone(),
            Path(server.to_owned()),
            Json(PatchBody {
                name: None,
                image_key: Some(key),
                open: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
            }),
        )
        .await
        .status()
    }

    #[tokio::test]
    async fn server_picture_must_be_an_image_by_type_and_bytes() {
        let state = test_state(AuthMode::Github);
        let (owner, owner_dev) = person(&state, 51);
        let server = Server {
            id: ulid(),
            name: "Launch Crew".into(),
            owner_user_id: owner.clone(),
            image_key: None,
            invite_code: "crew1234".into(),
            space_room_id: None,
            created_at: now_ms(),
            is_open: false,
            avatar_leaf: None,
            avatar_leaf_color: None,
        };
        state.db.insert_server(&server).unwrap();
        let picture = |state: &AppState| state.db.server(&server.id).unwrap().unwrap().image_key;

        store(
            &state,
            &server.id,
            "model.onnx",
            Some("application/octet-stream"),
            samples::ONNX,
        );
        assert_eq!(
            set_picture(&state, &owner_dev, &server.id, json!("model.onnx")).await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        store(
            &state,
            &server.id,
            "fake.png",
            Some("image/png"),
            samples::ONNX,
        );
        assert_eq!(
            set_picture(&state, &owner_dev, &server.id, json!("fake.png")).await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        store(&state, &server.id, "untyped.gif", None, samples::GIF);
        assert_eq!(
            set_picture(&state, &owner_dev, &server.id, json!("untyped.gif")).await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            set_picture(&state, &owner_dev, &server.id, json!("missing.png")).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(picture(&state), None);

        store(
            &state,
            &server.id,
            "crew.gif",
            Some("image/gif"),
            samples::GIF,
        );
        assert_eq!(
            set_picture(&state, &owner_dev, &server.id, json!("crew.gif")).await,
            StatusCode::OK
        );
        assert_eq!(picture(&state).as_deref(), Some("crew.gif"));
        assert_eq!(
            set_picture(&state, &owner_dev, &server.id, Value::Null).await,
            StatusCode::OK
        );
        assert_eq!(picture(&state), None);
    }
}
