//! People chats (D72 v1, permissive): find a person by GitHub handle, open a DM, list chats,
//! per-device read marks, blocks, per-user notifications, and the room ACL. A people chat is a
//! Loro chat doc in the room `people/<chat_id>` (`WS /room/people/{chat_id}`); the relay holds
//! membership and enforces access by **user**, so every device of a member is a peer.
//!
//! No request gate: the first DM opens for both people at once. Backstops only: block, 200 row
//! pushes per person per minute across people rooms, 50 new chats per person per day (both
//! halved for accounts under seven days old).

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use arbos_core::api::{
    ChatArchivedPayloadDto, ChatMeDto, ChatMemberDto, ChatMemberProjectDto,
    PeopleAgentAddedPayloadDto, PeopleAgentRemovedPayloadDto, PeopleChatCreatedPayloadDto,
    PeopleChatDeletedPayloadDto, PeopleChatDto, PeopleChatUpdatedPayloadDto,
    PeopleMemberAddedPayloadDto, PeopleMemberRemovedPayloadDto, PeopleMessagePayloadDto,
    PeopleProjectSharedPayloadDto, ProjectMemberDto, ProjectMemberRemovedPayloadDto,
    ProjectMembersDto, PushEventDto, UserCardDto, UsersPageDto,
};
use arbos_core::ids::{now_ms, ulid};
use arbos_core::wire;

use crate::auth::Principal;
use crate::blobs::{no_such_image, not_an_image, stored_image, StoredImage};
use crate::db_people::{ChatWithMembers, GroupInvite, Invite, Membership, PeopleChat};
use crate::notices;
use crate::signin::api_error;
use crate::state::AppState;

/// Project slot used for people rooms and blobs: `/room/people/{chat}`, `/blob/people:{chat}/…`.
pub const PEOPLE_PROJECT: &str = "people";
pub const PEOPLE_ACL_PREFIX: &str = "people:";

const SEARCH_LIMIT: i64 = 10;
const SEARCH_PER_MIN: i64 = 60;
const MSGS_PER_MIN: i64 = 200;
const NEW_CHATS_PER_DAY: i64 = 50;
const YOUNG_ACCOUNT_MS: i64 = 7 * 24 * 3_600_000;
const GITHUB_MISS_TTL_MS: i64 = 3_600_000;

pub fn room_of(chat_id: &str) -> String {
    wire::room_name(PEOPLE_PROJECT, chat_id)
}

// ---- ACL ----------------------------------------------------------------------------------------

/// May this principal read and write the room of a people chat? Devices: their user must be a
/// live member. Boxes: one of the chat's agents must be a project of the box's user. Admin: yes.
pub fn room_access(state: &AppState, principal: &Principal, chat_id: &str) -> bool {
    if state.db.people_chat(chat_id).ok().flatten().is_none() {
        return false;
    }
    match principal {
        Principal::Admin => true,
        Principal::Device { user_id, .. } => state
            .db
            .membership(chat_id, &format!("user:{user_id}"))
            .ok()
            .flatten()
            .is_some(),
        Principal::Box { user_id } => state
            .db
            .members_of(chat_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|m| m.project_id().map(str::to_owned))
            .any(|pid| {
                state
                    .db
                    .project(&pid)
                    .ok()
                    .flatten()
                    .is_some_and(|p| p.user_id == *user_id)
            }),
    }
}

/// The user behind a principal, or a 403.
fn user_of(principal: &Principal) -> Result<String, Box<Response>> {
    match principal {
        Principal::Device { user_id, .. } => Ok(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        ))),
    }
}

fn device_of(principal: &Principal) -> String {
    match principal {
        Principal::Device { id, .. } => id.clone(),
        Principal::Box { user_id } => format!("box:{user_id}"),
        Principal::Admin => "admin".to_owned(),
    }
}

fn young_account(state: &AppState, user_id: &str) -> bool {
    state
        .db
        .user(user_id)
        .ok()
        .flatten()
        .is_some_and(|u| now_ms() - u.created_at < YOUNG_ACCOUNT_MS)
}

fn rate_limited(retry_s: i64) -> Response {
    let mut r = api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "slow down");
    r.headers_mut().insert(
        header::RETRY_AFTER,
        retry_s.to_string().parse().expect("retry-after"),
    );
    r
}

/// May `adder` put `target` into a people group under the target's `group_add` privacy?
fn may_add_to_group(state: &AppState, adder: &str, target: &str) -> bool {
    let policy = state
        .db
        .group_add_policy(target)
        .unwrap_or_else(|_| "anyone".into());
    match policy.as_str() {
        "nobody" => false,
        "people_i_know" => state.db.dm_between(adder, target).ok().flatten().is_some(),
        _ => true, // anyone (default)
    }
}

/// Opaque "not available" when a group-add privacy check fails (same as a block).
fn group_add_forbidden() -> Response {
    api_error(
        StatusCode::FORBIDDEN,
        "not_available",
        "this person is not available",
    )
}

fn normalize_handle(h: &str) -> Option<String> {
    let h = h.trim().trim_start_matches('@').trim();
    let ok = !h.is_empty()
        && h.len() <= 39
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !h.starts_with('-')
        && !h.ends_with('-');
    ok.then(|| h.to_owned())
}

// ---- GitHub lookup for people who have not signed in ----------------------------------------

/// Resolve a GitHub login that is not in `users` yet by asking GitHub. With `create` the person
/// becomes an `invited` placeholder keyed by the GitHub id (a DM is about to open); without it
/// the card is returned as `status: "github"` and nothing is stored. Misses are remembered for
/// an hour in `meta`.
async fn resolve_via_github(state: &AppState, login: &str, create: bool) -> Option<UserCardDto> {
    let miss_key = format!("gh_miss:{}", login.to_lowercase());
    if let Ok(Some(at)) = state.db.meta_get(&miss_key) {
        if at
            .parse::<i64>()
            .is_ok_and(|t| now_ms() - t < GITHUB_MISS_TTL_MS)
        {
            return None;
        }
    }
    let mut req = state
        .http
        .get(format!("https://api.github.com/users/{login}"))
        .header(header::ACCEPT, "application/vnd.github+json")
        .header(header::USER_AGENT, "arbos-relay");
    if let Some(tok) = std::env::var("ARBOS_GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .filter(|s| !s.trim().is_empty())
    {
        req = req.bearer_auth(tok.trim());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, login, "github user lookup");
            return None;
        }
    };
    if resp.status() == StatusCode::NOT_FOUND {
        let _ = state.db.meta_set(&miss_key, &now_ms().to_string());
        return None;
    }
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), login, "github user lookup");
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    if v.get("type").and_then(Value::as_str) == Some("Organization") {
        let _ = state.db.meta_set(&miss_key, &now_ms().to_string());
        return None;
    }
    let id = v.get("id").and_then(Value::as_i64)?;
    let gh_login = v.get("login").and_then(Value::as_str)?.to_owned();
    let avatar = v.get("avatar_url").and_then(Value::as_str);
    let name = v.get("name").and_then(Value::as_str);
    if !create {
        return Some(github_status_card(id, &gh_login, avatar, name));
    }
    let user_id = state
        .db
        .upsert_invited_user(id, &gh_login, avatar, name)
        .ok()?;
    state.db.user_card(&user_id).ok().flatten()
}

/// Card for a GitHub login that is not stored locally (`status: "github"`).
pub(crate) fn github_status_card(
    github_id: i64,
    login: &str,
    avatar_url: Option<&str>,
    name: Option<&str>,
) -> UserCardDto {
    UserCardDto {
        id: format!("gh_{github_id}"),
        handle: login.to_owned(),
        avatar_url: avatar_url.map(str::to_owned),
        name: name.map(str::to_owned),
        status: "github".into(),
        created_at: None,
        test: None,
        last_seen_at: None,
        online: None,
        avatar_color_index_effective: None,
    }
}

/// A person by handle for a DM or a block: local first, GitHub second (stored as `invited`).
async fn find_person(state: &AppState, handle: &str) -> Option<UserCardDto> {
    if let Ok(Some(u)) = state.db.user_by_handle(handle) {
        return Some(u);
    }
    resolve_via_github(state, handle, true).await
}

// ---- JSON shapes --------------------------------------------------------------------------------

/// A person counts as online while a device of theirs talked to the relay this recently
/// (presence frames come every 15 s; HTTP polls every 30 s).
pub const ONLINE_MS: i64 = 90_000;

/// Add `last_seen_at` (floored to the minute) and `online` to a user card.
pub fn with_presence(state: &AppState, mut card: UserCardDto) -> UserCardDto {
    let uid = card.id.clone();
    let seen = state.db.user_last_seen(&uid).ok().flatten();
    card.last_seen_at = Some(seen.map(|t| t - t % 60_000));
    card.online = Some(seen.is_some_and(|t| now_ms() - t < ONLINE_MS));
    // D79: people's initials colour derives from the user id; no override for people.
    card.avatar_color_index_effective = Some(i64::from(crate::avatar::color_index(&uid)));
    card
}

#[allow(dead_code)] // helper kept for wire/fixture tests; people_chat_dto uses member_dto
pub(crate) fn member_json(state: &AppState, m: &Membership) -> Value {
    serde_json::to_value(member_dto(state, m)).expect("ChatMemberDto")
}

pub(crate) fn member_dto(state: &AppState, m: &Membership) -> ChatMemberDto {
    let mut dto = ChatMemberDto {
        member: m.member.clone(),
        role: m.role.clone(),
        added_by: m.added_by.clone(),
        joined_at: m.joined_at,
        handle: None,
        avatar_url: None,
        name: None,
        status: None,
        user_id: None,
        last_seen_at: None,
        online: None,
        read_receipts: None,
        silenced: None,
        silenced_until: None,
        can_mute_ban: None,
        avatar: None,
        avatar_color: None,
        avatar_color_index: None,
        avatar_color_index_effective: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        avatar_leaf_effective: None,
        avatar_leaf_color_effective: None,
        project: None,
        description: None,
        owner: None,
        owner_handle: None,
        settings: None,
    };
    if let Some(uid) = m.user_id() {
        if let Ok(Some(card)) = state.db.user_card(uid) {
            dto.handle = Some(card.handle);
            // Always present for humans (null or string).
            dto.avatar_url = Some(card.avatar_url);
            dto.name = Some(card.name);
            dto.status = Some(card.status);
        }
        dto.user_id = Some(uid.to_owned());
        let seen = state.db.user_last_seen(uid).ok().flatten();
        dto.last_seen_at = Some(seen.map(|t| t - t % 60_000));
        dto.online = Some(seen.is_some_and(|t| now_ms() - t < ONLINE_MS));
        dto.avatar_color_index_effective = Some(i64::from(crate::avatar::color_index(uid)));
        dto.read_receipts = Some(
            m.settings
                .get("read_receipts")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        );
        let silenced = membership_is_silenced(m);
        dto.silenced = Some(silenced);
        if silenced {
            dto.silenced_until = m.silenced_until;
        }
        if m.role == "admin" {
            dto.can_mute_ban = Some(member_can_mute_ban(m));
        }
    }
    if let Some(pid) = m.project_id() {
        if let Ok(Some(p)) = state.db.project(pid) {
            dto.handle = Some(agent_slug(&p.name));
            let avatar = p
                .avatar
                .clone()
                .unwrap_or_else(|| crate::avatar::default_for(&p.id, &state.cfg.avatar_slugs));
            let avatar_color = p.avatar_color.clone().unwrap_or_else(|| {
                crate::avatar::default_color_for(&p.id, &state.cfg.avatar_colors)
            });
            let effective = i64::from(crate::avatar::effective_color_index(
                &p.id,
                p.avatar_color_index,
            ));
            dto.avatar = Some(avatar.clone());
            dto.avatar_color = Some(avatar_color.clone());
            let leaf = crate::avatar::effective_leaf(&p);
            dto.avatar_color_index = Some(p.avatar_color_index);
            dto.avatar_color_index_effective = Some(effective);
            dto.avatar_leaf = Some(p.avatar_leaf);
            dto.avatar_leaf_color = Some(p.avatar_leaf_color);
            dto.avatar_leaf_effective = Some(i64::from(leaf.leaf));
            dto.avatar_leaf_color_effective = Some(i64::from(leaf.color));
            dto.project = Some(ChatMemberProjectDto {
                id: p.id.clone(),
                name: p.name.clone(),
                icon: p.icon.clone(),
                color: p.color.clone(),
                avatar,
                avatar_color,
                avatar_color_index: p.avatar_color_index,
                avatar_color_index_effective: effective,
                avatar_leaf: p.avatar_leaf,
                avatar_leaf_color: p.avatar_leaf_color,
                avatar_leaf_effective: Some(i64::from(leaf.leaf)),
                avatar_leaf_color_effective: Some(i64::from(leaf.color)),
                image_key: p.image_key.clone(),
                avatar_version: Some(p.avatar_version),
                description: p.description.clone(),
                model: p
                    .models
                    .get("talking_model")
                    .and_then(|m| m.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                owner: crate::api::project_owner_dto(state, &p.user_id),
            });
            dto.description = Some(p.description.clone());
            dto.owner = Some(p.user_id.clone());
            dto.owner_handle = Some(
                state
                    .db
                    .user_card(&p.user_id)
                    .ok()
                    .flatten()
                    .map(|c| c.handle),
            );
        }
        dto.settings = Some(if m.settings.is_object() {
            m.settings.clone()
        } else {
            json!({ "reply_policy": "mention", "restrict_guests": false })
        });
    }
    dto
}

/// The `@slug` a project agent answers to — the shared rule (`arbos_core::avatar::identity`),
/// so the roster's `handle`, agentd's attribution and both clients' lookups agree (#1592).
pub fn agent_slug(name: &str) -> String {
    arbos_core::avatar::identity::agent_slug(name)
}

/// An agent's `@slug` must not be a person's `@handle`: when Jacob (@unarbos) named an agent
/// "unarbos", `@unarbos` in the group resolved to him and the agent's mention gate never fired
/// (#1538). The name that keeps the slug clear of every signed-in user's login: `name` itself,
/// else `<name> agent`, `<name> 2`, `<name> 3`, … Returns `(name, changed)`.
pub fn agent_name_clear_of_people(state: &AppState, name: &str) -> (String, bool) {
    let taken = |candidate: &str| {
        let slug = agent_slug(candidate);
        !slug.is_empty() && state.db.user_by_handle(&slug).ok().flatten().is_some()
    };
    let name = name.trim();
    if !taken(name) {
        return (name.to_owned(), false);
    }
    let first = format!("{name} agent");
    if !taken(&first) {
        return (first, true);
    }
    for n in 2..100 {
        let c = format!("{name} {n}");
        if !taken(&c) {
            return (c, true);
        }
    }
    (format!("{name} agent {}", now_ms() % 1000), true)
}

/// One-shot at boot (#1538): every live agent whose slug is a person's handle gets the same
/// suffix a new one would, and its box hears `project_renamed`. Idempotent; logs each rename.
pub fn rename_agents_that_shadow_people(state: &AppState) {
    let Ok(projects) = state.db.list_projects() else {
        return;
    };
    for p in projects {
        let (clear, changed) = agent_name_clear_of_people(state, &p.name);
        if !changed {
            continue;
        }
        if let Err(e) = state.db.update_project(
            &p.id,
            Some(&clear),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ) {
            tracing::warn!(project = %p.id, error = %e, "handle-collision rename failed");
            continue;
        }
        let _ = state.db.enqueue_push(
            &p.id,
            "project_renamed",
            &json!({ "from": p.name, "to": clear, "at": now_ms(), "reason": "handle_collision" }),
        );
        tracing::info!(project = %p.id, from = %p.name, to = %clear, "agent renamed: its @slug was a person's handle");
    }
}

/// Soft-deleted agents leave every list at once; do not linger as @mention ghosts that
/// never wake (workspace already moved to `/work/.archive/`). Humans always pass.
fn member_alive(state: &AppState, m: &Membership) -> bool {
    match m.project_id() {
        None => true,
        Some(pid) => state.db.project(pid).ok().flatten().is_some(),
    }
}

pub fn chat_json(state: &AppState, cm: &ChatWithMembers, me: &str, device_id: &str) -> Value {
    serde_json::to_value(people_chat_dto(state, cm, me, device_id)).expect("PeopleChatDto")
}

pub fn people_chat_dto(
    state: &AppState,
    cm: &ChatWithMembers,
    me: &str,
    device_id: &str,
) -> PeopleChatDto {
    let (head_seq, last_row_at) = state
        .db
        .room_activity(&room_of(&cm.chat.id))
        .unwrap_or((0, 0));
    // One read cursor per user (every device advances it); the older per-device mark still
    // counts so a cursor written before this relay does not read as "unread again".
    let (device_seq, _) = state
        .db
        .read_mark(&cm.chat.id, device_id)
        .unwrap_or((0, false));
    let read_seq = state
        .db
        .user_cursor(me, &cm.chat.id)
        .unwrap_or(0)
        .max(device_seq);
    // Unread is what someone else announced after the user's read mark — never their own rows
    // (from any device), never system lines, never a row nobody announced.
    let unread = state.db.unread_in_chat(me, &cm.chat.id).unwrap_or(0) > 0;
    // Drop soft-deleted agent memberships from the client roster (purge clears the rows later).
    let live: Vec<&Membership> = cm
        .members
        .iter()
        .filter(|m| member_alive(state, m))
        .collect();
    let members: Vec<ChatMemberDto> = live.iter().map(|m| member_dto(state, m)).collect();
    let others: Vec<&ChatMemberDto> = members
        .iter()
        .filter(|m| m.member.starts_with("user:") && m.member != format!("user:{me}"))
        .collect();
    // Untitled group: the other people's handles — or, with no other human
    // (a group of my agents), the agents' handles.
    let named: Vec<&ChatMemberDto> = if others.is_empty() {
        members
            .iter()
            .filter(|m| m.member.starts_with("agent:"))
            .collect()
    } else {
        others.clone()
    };
    let computed = named
        .iter()
        .filter_map(|m| m.handle.as_deref())
        .collect::<Vec<_>>()
        .join(", ");
    let title = cm
        .chat
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(computed.as_str())
        .to_owned();
    let pending = others
        .iter()
        .any(|m| m.status.as_deref() == Some("invited"));
    let has_agent = live.iter().any(|m| m.project_id().is_some());
    let mine = live.iter().find(|m| m.user_id() == Some(me)).copied();
    let archived_at = state.db.chat_archived_at(me, &cm.chat.id).ok().flatten();
    let can_see_banned =
        mine.is_some_and(|m| m.role == "owner" || (m.role == "admin" && member_can_mute_ban(m)));
    let banned = if can_see_banned {
        state
            .db
            .banned_members_of(&cm.chat.id)
            .unwrap_or_default()
            .iter()
            .filter(|m| m.user_id().is_some())
            .map(|m| member_dto(state, m))
            .collect()
    } else {
        Vec::new()
    };
    PeopleChatDto {
        id: cm.chat.id.clone(),
        archived_at,
        kind: cm.chat.kind.clone(),
        created_by: cm.chat.created_by.clone(),
        created_at: cm.chat.created_at,
        title,
        image_key: cm.chat.image_key.clone(),
        avatar_leaf: cm.chat.avatar_leaf,
        avatar_leaf_color: cm.chat.avatar_leaf_color,
        description: cm
            .chat
            .description
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
        pending,
        members,
        banned,
        room: format!("/room/{PEOPLE_PROJECT}/{}", cm.chat.id),
        head_seq,
        last_activity_at: if last_row_at > 0 {
            last_row_at
        } else {
            cm.chat.created_at
        },
        read_seq,
        unread,
        // Members this viewer blocked: their rows are theirs to hide (groups), as `user:<id>`.
        blocked_senders: human_user_ids(cm)
            .into_iter()
            .filter(|u| u != me && state.db.blocks(me, u).unwrap_or(false))
            .map(|u| format!("user:{u}"))
            .collect(),
        lock_state: if has_agent {
            "visible_to_agents".into()
        } else {
            "people_only".into()
        },
        me: ChatMeDto {
            role: Some(mine.map(|m| m.role.clone())),
            muted: Some(mine.is_some_and(|m| m.muted)),
            silenced: Some(mine.is_some_and(membership_is_silenced)),
            silenced_until: mine
                .filter(|m| membership_is_silenced(m))
                .and_then(|m| m.silenced_until),
            archived_at,
            read_receipts: Some(
                mine.map(|m| {
                    m.settings
                        .get("read_receipts")
                        .and_then(Value::as_bool)
                        .unwrap_or(true)
                })
                .unwrap_or(true),
            ),
        },
    }
}

pub(crate) fn load_chat(state: &AppState, chat_id: &str) -> Result<ChatWithMembers, Box<Response>> {
    let chat = state
        .db
        .people_chat(chat_id)
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
                "no such chat",
            ))
        })?;
    let members = state.db.members_of(chat_id).map_err(|e| {
        Box::new(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db",
            e.to_string(),
        ))
    })?;
    Ok(ChatWithMembers { chat, members })
}

/// The chat, if the caller's user is a live member.
pub(crate) fn member_chat(
    state: &AppState,
    principal: &Principal,
    chat_id: &str,
) -> Result<(String, ChatWithMembers), Box<Response>> {
    let me = user_of(principal)?;
    let cm = load_chat(state, chat_id)?;
    if !cm.members.iter().any(|m| m.user_id() == Some(me.as_str())) {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "not_member",
            "you are not in this chat",
        )));
    }
    Ok((me, cm))
}

/// What `PATCH /chats/{id}` and the group's avatar Generate answer to a member who is neither
/// owner nor admin. One plain sentence: the clients show it as-is.
pub(crate) const SETTINGS_ADMIN_ONLY: &str =
    "Only the group's owner and admins can change its name, picture and description.";

/// The chat, if the caller may change its settings (name, picture, description, leaf): the
/// owner or an admin. Everyone else — plain members, agents' boxes, non-members, the admin
/// token — gets 403, and every attempt leaves an audit row (`settings_audit`).
///
/// Why: until 2026-10-27 any live human member could rename a group or swap its picture
/// (D72 "fully permissive"); members of Arbonites did exactly that.
pub(crate) fn settings_chat(
    state: &AppState,
    principal: &Principal,
    chat_id: &str,
    fields: &[&str],
) -> Result<(String, ChatWithMembers), Box<Response>> {
    let slot = format!("{PEOPLE_ACL_PREFIX}{chat_id}");
    let (me, cm) = match member_chat(state, principal, chat_id) {
        Ok(v) => v,
        Err(r) => {
            let reason = match principal {
                Principal::Device { .. } => "not_member",
                Principal::Box { .. } | Principal::Admin => "devices_only",
            };
            crate::settings_audit::record(
                state, &slot, "chat", chat_id, principal, None, fields, false, reason,
            );
            return Err(r);
        }
    };
    let role = my_role(&cm, &me);
    let allowed = matches!(role, Some("owner") | Some("admin"));
    crate::settings_audit::record(
        state,
        &slot,
        "chat",
        chat_id,
        principal,
        role,
        fields,
        allowed,
        if allowed { "ok" } else { "admin_only" },
    );
    if !allowed {
        return Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "admin_only",
            SETTINGS_ADMIN_ONLY,
        )));
    }
    Ok((me, cm))
}

pub fn handle_of(state: &AppState, user_id: &str) -> String {
    state
        .db
        .user_card(user_id)
        .ok()
        .flatten()
        .map(|c| c.handle)
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| user_id.to_owned())
}

pub fn human_user_ids(cm: &ChatWithMembers) -> Vec<String> {
    cm.members
        .iter()
        .filter_map(|m| m.user_id().map(str::to_owned))
        .collect()
}

// ---- handlers -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
}

/// `GET /users/search?q=@jac`
pub async fn users_search(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<SearchQuery>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let Some(prefix) = normalize_handle(&q.q) else {
        return Json(Value::Array(Vec::new())).into_response();
    };
    match state.db.rate_event(&me, "search", 60_000) {
        Ok(n) if n > SEARCH_PER_MIN => return rate_limited(60),
        _ => {}
    }
    let mut found = state
        .db
        .search_users(&prefix, SEARCH_LIMIT)
        .unwrap_or_default();
    found.retain(|u| u.id != me);
    let exact = found.iter().any(|u| u.handle.eq_ignore_ascii_case(&prefix));
    if !exact {
        if let Some(card) = resolve_via_github(&state, &prefix, false).await {
            if card.id != me {
                found.insert(0, card);
                found.truncate(SEARCH_LIMIT as usize);
            }
        }
    }
    let found: Vec<UserCardDto> = found
        .into_iter()
        .map(|u| with_presence(&state, u))
        .collect();
    Json(found).into_response()
}

/// `GET /avatar-colors` — the one 7-entry initials palette every client renders from (D79).
pub async fn avatar_colors() -> Response {
    Json(crate::avatar::palette_json()).into_response()
}

/// `GET /avatar-leaves` — the D92 leaf shapes, colours and index rule (public, no auth).
pub async fn avatar_leaves() -> Response {
    Json(crate::avatar::leaves_json()).into_response()
}

/// `GET /users/{id}` — one person's card for a profile page: handle, name, avatar, status,
/// `last_seen_at` (to the minute) and `online`; any signed-in device may read members.
pub async fn user_profile(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = user_of(&principal) {
        return *r;
    }
    match state.db.user_card(&id) {
        Ok(Some(card)) => Json(with_presence(&state, card)).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "no such user"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct DirectoryQuery {
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /users?offset=0&limit=100` — the directory (members and invited people except me),
/// alphabetical by handle, with `total` and `next_offset` for paging.
pub async fn users_directory(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<DirectoryQuery>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    match state.db.list_directory(&me, offset, limit) {
        Ok((users, total)) => {
            let users: Vec<UserCardDto> = users
                .into_iter()
                .map(|u| with_presence(&state, u))
                .collect();
            let next = offset + users.len() as i64;
            Json(UsersPageDto {
                users,
                total,
                offset,
                next_offset: if next < total { Some(next) } else { None },
            })
            .into_response()
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct CreateGroupBody {
    pub kind: String,
    /// GitHub handles of the other people (the caller is added as owner).
    pub members: Vec<String>,
    /// The caller's own projects to add as agents (same rule as `POST /chats/{id}/agents`).
    /// A group may be people, agents, or both — never nobody.
    #[serde(default)]
    pub agent_ids: Vec<String>,
    /// Create screen (D93): the group's name and its leaf picture; all optional.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub avatar_leaf: Option<i64>,
    #[serde(default)]
    pub avatar_leaf_color: Option<i64>,
    /// Optional about text (≤200 chars).
    #[serde(default)]
    pub description: Option<String>,
}

/// `POST /chats {kind: "group", members: [handles…], agent_ids: [project ids…]}` — a new people
/// group. The caller is the owner; each handle becomes a member (invited via GitHub if they are
/// not on Arbos yet); each agent id must be one of the caller's own projects. At least one person
/// or one agent. Always a fresh chat — unlike a DM, it does not reuse an existing 1:1.
pub async fn chats_create_group(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<CreateGroupBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    if b.kind != "group" {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_kind",
            "kind must be \"group\"",
        );
    }
    let mut other_ids: Vec<String> = Vec::new();
    for raw in &b.members {
        let Some(handle) = normalize_handle(raw) else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_handle",
                "that is not a GitHub login",
            );
        };
        let Some(other) = find_person(&state, &handle).await else {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no GitHub account with that login",
            );
        };
        let other_id = other.id.clone();
        if other_id.is_empty() || other_id == me {
            continue;
        }
        if other_ids.iter().any(|id| id == &other_id) {
            continue;
        }
        if state.db.blocked_either_way(&me, &other_id).unwrap_or(false) {
            return api_error(
                StatusCode::FORBIDDEN,
                "not_available",
                "this person is not available",
            );
        }
        if state
            .db
            .blocked_by_any(&other_id, &other_ids)
            .unwrap_or(false)
        {
            return api_error(
                StatusCode::FORBIDDEN,
                "not_available",
                "this person is not available",
            );
        }
        if !may_add_to_group(&state, &me, &other_id) {
            return group_add_forbidden();
        }
        other_ids.push(other_id);
    }
    // Agents are checked before anything is written: a foreign project fails the whole call.
    let mut agent_projects = Vec::new();
    for pid in &b.agent_ids {
        if agent_projects
            .iter()
            .any(|p: &crate::db::Project| &p.id == pid)
        {
            continue;
        }
        match own_project(&state, &me, pid) {
            Ok(p) => agent_projects.push(p),
            Err(r) => return *r,
        }
    }
    if other_ids.is_empty() && agent_projects.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "need_members",
            "add at least one person or one of your agents",
        );
    }
    let limit = if young_account(&state, &me) {
        NEW_CHATS_PER_DAY / 2
    } else {
        NEW_CHATS_PER_DAY
    };
    let today = crate::proxy::start_of_utc_day_ms();
    if state.db.people_chats_created_since(&me, today).unwrap_or(0) >= limit {
        return rate_limited(((today + 86_400_000 - now_ms()) / 1000).max(1));
    }
    let leaf = match leaf_patch(b.avatar_leaf.map(Some), b.avatar_leaf_color.map(Some)) {
        Ok(l) => l.flatten(),
        Err((st, msg)) => return api_error(st, "bad_leaf", msg),
    };
    let chat = PeopleChat {
        id: ulid(),
        kind: "group".into(),
        created_by: me.clone(),
        created_at: now_ms(),
        title: b
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(80).collect()),
        image_key: None,
        avatar_leaf: leaf.map(|l| l.0),
        avatar_leaf_color: leaf.map(|l| l.1),
        description: b
            .description
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(200).collect()),
    };
    if let Err(e) = state.db.insert_people_chat(&chat) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    if let Err(e) = state
        .db
        .add_member(&chat.id, &format!("user:{me}"), "owner", &me, &json!({}))
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    for uid in &other_ids {
        if let Err(e) =
            state
                .db
                .add_member(&chat.id, &format!("user:{uid}"), "member", &me, &json!({}))
        {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        let _ = state.db.push_user_event(
            uid,
            &PushEventDto::PeopleChatCreated(PeopleChatCreatedPayloadDto {
                chat_id: chat.id.clone(),
                by: format!("user:{me}"),
                by_handle: handle_of(&state, &me),
            }),
            Some(&chat.id),
        );
    }
    for project in &agent_projects {
        if let Err(e) = state.db.add_member(
            &chat.id,
            &format!("agent:{}", project.id),
            "agent",
            &me,
            &default_agent_settings(),
        ) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        let _ = state.db.enqueue_push(
            &project.id,
            "people_agent_added",
            &json!({ "chat_id": chat.id }),
        );
        crate::boxes::ensure_awake(&state, &project.user_id, "people").await;
    }
    tracing::info!(
        chat = %chat.id,
        %me,
        members = other_ids.len(),
        agents = agent_projects.len(),
        "people group created"
    );
    state.matrix.touch_chat(&state, &chat.id, None);
    match load_chat(&state, &chat.id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
pub struct DmBody {
    pub handle: String,
}

/// `POST /chats/dm {"handle"}` → the DM with that person (existing or new; opens for both).
pub async fn dm_create(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<DmBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let Some(handle) = normalize_handle(&b.handle) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_handle",
            "that is not a GitHub login",
        );
    };
    let Some(other) = find_person(&state, &handle).await else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no GitHub account with that login",
        );
    };
    let other_id = other.id.clone();
    if other_id == me {
        return api_error(StatusCode::BAD_REQUEST, "bad_handle", "that is you");
    }
    if state.db.blocked_either_way(&me, &other_id).unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_available",
            "this person is not available",
        );
    }
    let device = device_of(&principal);
    if let Ok(Some(existing)) = state.db.dm_between(&me, &other_id) {
        return match load_chat(&state, &existing.id) {
            Ok(cm) => Json(chat_json(&state, &cm, &me, &device)).into_response(),
            Err(r) => *r,
        };
    }
    let limit = if young_account(&state, &me) {
        NEW_CHATS_PER_DAY / 2
    } else {
        NEW_CHATS_PER_DAY
    };
    let today = crate::proxy::start_of_utc_day_ms();
    if state.db.people_chats_created_since(&me, today).unwrap_or(0) >= limit {
        return rate_limited(((today + 86_400_000 - now_ms()) / 1000).max(1));
    }
    let chat = PeopleChat {
        id: ulid(),
        kind: "dm".into(),
        created_by: me.clone(),
        created_at: now_ms(),
        title: None,
        image_key: None,
        avatar_leaf: None,
        avatar_leaf_color: None,
        description: None,
    };
    if let Err(e) = state.db.insert_people_chat(&chat) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    for uid in [&me, &other_id] {
        if let Err(e) =
            state
                .db
                .add_member(&chat.id, &format!("user:{uid}"), "owner", &me, &json!({}))
        {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
    }
    let _ = state.db.push_user_event(
        &other_id,
        &PushEventDto::PeopleChatCreated(PeopleChatCreatedPayloadDto {
            chat_id: chat.id.clone(),
            by: format!("user:{me}"),
            by_handle: handle_of(&state, &me),
        }),
        Some(&chat.id),
    );
    tracing::info!(chat = %chat.id, %me, other = %other_id, "dm opened");
    state.matrix.touch_chat(&state, &chat.id, None);
    match load_chat(&state, &chat.id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device)).into_response(),
        Err(r) => *r,
    }
}

/// `GET /chats` — my people chats, newest activity first; hidden-on-this-device ones excluded.
pub async fn chats_list(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let device = device_of(&principal);
    let chats = match state.db.chats_of_member(&format!("user:{me}")) {
        Ok(c) => c,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let mut out: Vec<Value> = chats
        .iter()
        .filter(|cm| {
            !state
                .db
                .read_mark(&cm.chat.id, &device)
                .map(|(_, hidden)| hidden)
                .unwrap_or(false)
        })
        .map(|cm| chat_json(&state, cm, &me, &device))
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c["last_activity_at"].as_i64().unwrap_or(0)));
    Json(out).into_response()
}

/// `GET /chats/{id}`
pub async fn chat_get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    match member_chat(&state, &principal, &id) {
        Ok((me, cm)) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
pub struct PatchChatBody {
    /// Custom group name. Empty string clears it (falls back to member handles).
    #[serde(default)]
    pub title: Option<String>,
    /// Blob key under `people:{chat}` for the group picture. Empty clears it.
    #[serde(default)]
    pub image_key: Option<String>,
    /// D92 amendment: the group's leaf picture (0–8) and disc colour (0–9), both or neither;
    /// `null` for both clears it (back to the member cluster).
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
    /// Optional about text. Empty string clears it.
    #[serde(default)]
    pub description: Option<String>,
}

/// `None` = untouched, `Some(None)` = clear, `Some(Some((leaf, colour)))` = set.
type LeafPatch = Option<Option<(i64, i64)>>;

/// `avatar_leaf` + `avatar_leaf_color` from a body, validated.
fn leaf_patch(
    leaf: Option<Option<i64>>,
    color: Option<Option<i64>>,
) -> Result<LeafPatch, (StatusCode, String)> {
    match (leaf, color) {
        (None, None) => Ok(None),
        (Some(None), Some(None)) | (Some(None), None) | (None, Some(None)) => Ok(Some(None)),
        (Some(Some(l)), Some(Some(c))) => {
            crate::avatar::check_leaf_pick(Some(l), Some(c))?;
            Ok(Some(Some((l, c))))
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            "avatar_leaf and avatar_leaf_color go together".to_owned(),
        )),
    }
}

fn safe_image_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':')
}

/// The settings fields a `PATCH /chats/{id}` body touches, for the audit row.
fn patched_fields(b: &PatchChatBody) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if b.title.is_some() {
        fields.push("title");
    }
    if b.image_key.is_some() {
        fields.push("image_key");
    }
    if b.avatar_leaf.is_some() || b.avatar_leaf_color.is_some() {
        fields.push("avatar_leaf");
    }
    if b.description.is_some() {
        fields.push("description");
    }
    fields
}

/// `PATCH /chats/{id} {"title"?, "image_key"?, "description"?}` — the group's owner and
/// admins name the group, set an optional picture (uploaded first via
/// `PUT /blob/people:{id}/…`), and set an optional description. Any other member gets 403
/// (`settings_chat`); an empty body is a read any member may do.
pub async fn chat_patch(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<PatchChatBody>,
) -> Response {
    let fields = patched_fields(&b);
    if fields.is_empty() {
        return match member_chat(&state, &principal, &id) {
            Ok((me, cm)) => {
                Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response()
            }
            Err(r) => *r,
        };
    }
    let (me, cm) = match settings_chat(&state, &principal, &id, &fields) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let leaf = match leaf_patch(b.avatar_leaf, b.avatar_leaf_color) {
        Ok(l) => l,
        Err((st, msg)) => return api_error(st, "bad_leaf", msg),
    };
    let title = b.title.as_ref().map(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(80).collect::<String>())
        }
    });
    let description = b.description.as_ref().map(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(200).collect::<String>())
        }
    });
    let image_key = match &b.image_key {
        None => None,
        Some(k) if k.trim().is_empty() => Some(None),
        Some(k) if safe_image_key(k.trim()) => Some(Some(k.trim().to_owned())),
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_image_key",
                "image_key must be a short blob object name",
            );
        }
    };
    // The picture is judged on what was uploaded (Content-Type and bytes), not its name.
    if let Some(Some(key)) = &image_key {
        match stored_image(&state, &format!("{PEOPLE_ACL_PREFIX}{id}"), key).await {
            StoredImage::Image(kind) => {
                tracing::info!(chat = %id, %key, %kind, "group picture set");
            }
            StoredImage::Missing => return no_such_image(),
            StoredImage::NotAnImage(why) => return not_an_image(&why),
        }
    }
    let title_ref = title.as_ref().map(|o| o.as_ref().map(|s| s.as_str()));
    let image_ref = image_key.as_ref().map(|o| o.as_ref().map(|s| s.as_str()));
    let description_ref = description.as_ref().map(|o| o.as_ref().map(|s| s.as_str()));
    if let Err(e) =
        state
            .db
            .update_people_chat_profile(&id, title_ref, image_ref, leaf, description_ref)
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    // Other members, and this user's other devices (e.g. Mac), refresh the list.
    let event = PushEventDto::PeopleChatUpdated(PeopleChatUpdatedPayloadDto {
        chat_id: id.clone(),
    });
    notify_members(&state, &cm, &me, &event);
    state.matrix.touch_chat(&state, &id, None);
    let _ = state
        .db
        .push_user_except_event(&me, &device_of(&principal), &event);
    match member_chat(&state, &principal, &id) {
        Ok((me, cm)) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

#[derive(Deserialize)]
pub struct ReadBody {
    pub seq: i64,
}

/// `PUT /chats/{id}/read {"seq"}` — the user has seen rows up to `seq` (on this device; the
/// cursor is per user, so their other devices read it too).
pub async fn chat_read(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ReadBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let is_people = state.db.people_chat(&id).ok().flatten().is_some();
    let chat_ref = if is_people {
        if let Err(r) = member_chat(&state, &principal, &id) {
            return *r;
        }
        if let Err(e) = state
            .db
            .set_read_mark(&id, &device_of(&principal), b.seq.max(0))
        {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        crate::notify::ChatRef::people(&id)
    } else {
        // A project chat (own or shared): the same call marks it read for unread counts.
        let Ok(Some(project_id)) = state.db.chat_project_id(&id) else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
        };
        if !state.allows(&principal, &project_id) {
            return api_error(StatusCode::FORBIDDEN, "not_member", "not your chat");
        }
        crate::notify::ChatRef::project(&project_id, &id)
    };
    if let Err(e) = state
        .db
        .mark_chat_read(&me, &id)
        .and_then(|()| state.db.advance_user_cursor(&me, &id, b.seq.max(0)))
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.read(&state, &id, &me);
    // Read here = cleared everywhere else: the other devices drop the chat's banners and
    // take the new badge (`/me/pushes` now; the APNs badge alert while push is on).
    let device = device_of(&principal);
    let read_upto = format!("seq:{}", b.seq.max(0));
    crate::notify::clear_on_read(&state, &me, &device, &chat_ref, &read_upto).await;
    crate::notify::badge_refresh(&state, &me, &device).await;
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /chats/{id}/hide` — delete-for-me on this device (a new message un-hides it).
pub async fn chat_hide(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = member_chat(&state, &principal, &id) {
        return *r;
    }
    match state.db.set_hidden(&id, &device_of(&principal), true) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `POST /chats/{id}/leave`
pub async fn chat_leave(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(e) = state.db.remove_member(&id, &format!("user:{me}"), false) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.touch_chat(&state, &id, None);
    notify_members(
        &state,
        &cm,
        &me,
        &PushEventDto::PeopleMemberRemoved(PeopleMemberRemovedPayloadDto {
            chat_id: id.clone(),
            user_id: me.clone(),
            handle: handle_of(&state, &me),
            by_handle: handle_of(&state, &me),
            banned: None,
        }),
    );
    // Only a group keeps a chat for the others to read; a DM just ends.
    if cm.chat.kind == "group" {
        let line = notices::left_line(&notices::person_name(&state, &me));
        let n = notices::Notice {
            kind: "left",
            actor: me.clone(),
            subjects: vec![],
        };
        if let Err(e) = notices::post_people_notice(&state, &id, &line, n).await {
            tracing::warn!(chat = %id, error = %e, "membership notice not written");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `DELETE /chats/{id}` — chat owner; rows, checkpoints, and blobs go.
pub async fn chat_delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if !cm
        .members
        .iter()
        .any(|m| m.user_id() == Some(me.as_str()) && m.role == "owner")
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only an owner can delete the chat",
        );
    }
    if let Err(e) = state.db.delete_people_chat(&id) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    state.matrix.close_chat(&state, &id);
    let _ = state.db.delete_room(&room_of(&id));
    let _ = std::fs::remove_dir_all(
        state
            .cfg
            .data_dir
            .join("blobs")
            .join(format!("{PEOPLE_ACL_PREFIX}{id}")),
    );
    state
        .room(&room_of(&id))
        .await
        .tx
        .send(crate::state::RoomBroadcast {
            from_conn: 0,
            seq: 0,
            bytes: wire::Frame::new(
                wire::FrameType::Error,
                json!({ "code": "chat_deleted", "message": "this chat was deleted" }),
                bytes::Bytes::new(),
            )
            .encode(),
        })
        .ok();
    notify_members(
        &state,
        &cm,
        &me,
        &PushEventDto::PeopleChatDeleted(PeopleChatDeletedPayloadDto {
            chat_id: id,
            by_handle: handle_of(&state, &me),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /me/pushes` — this device's pending notifications, delivered once.
pub async fn me_pushes(State(state): State<AppState>, principal: Principal) -> Response {
    let Principal::Device { id, .. } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "devices only");
    };
    match state.db.take_user_pushes(id) {
        Ok(items) => Json(
            items
                .into_iter()
                .map(|(id, kind, payload, at)| json!({ "id": id.to_string(), "kind": kind, "payload": payload, "created_at": at }))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct BlockBody {
    pub handle: String,
}

/// `POST /blocks {"handle"}`
pub async fn block_create(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<BlockBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let Some(handle) = normalize_handle(&b.handle) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_handle",
            "that is not a GitHub login",
        );
    };
    let Some(other) = find_person(&state, &handle).await else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no GitHub account with that login",
        );
    };
    let other_id = other.id.as_str();
    if other_id == me {
        return api_error(StatusCode::BAD_REQUEST, "bad_handle", "that is you");
    }
    match state.db.block(&me, other_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `DELETE /blocks/{user_id}`
pub async fn block_delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(user_id): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match state.db.unblock(&me, &user_id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "not_found", "not blocked"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `GET /blocks`
pub async fn blocks_list(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let rows = state.db.blocks_of(&me).unwrap_or_default();
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(uid, at)| {
            let card = state
                .db
                .user_card(&uid)
                .ok()
                .flatten()
                .unwrap_or(UserCardDto {
                    id: uid,
                    ..Default::default()
                });
            let mut v = serde_json::to_value(card).unwrap_or(Value::Null);
            v["created_at"] = json!(at);
            v
        })
        .collect();
    Json(out).into_response()
}

// ---- agents in a chat (step 2) ------------------------------------------------------------------

fn default_agent_settings() -> Value {
    json!({ "reply_policy": "mention", "restrict_guests": false })
}

/// The project, if it belongs to the caller.
fn own_project(
    state: &AppState,
    me: &str,
    project_id: &str,
) -> Result<crate::db::Project, Box<Response>> {
    match state.db.project(project_id) {
        Ok(Some(p)) if p.user_id == me => Ok(p),
        Ok(Some(_)) => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "own_project_only",
            "you can only add or change your own Project's agent",
        ))),
        Ok(None) => Err(Box::new(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such project",
        ))),
        Err(e) => Err(Box::new(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db",
            e.to_string(),
        ))),
    }
}

#[derive(Deserialize)]
pub struct AddAgentBody {
    pub project_id: String,
}

/// `POST /chats/{id}/agents {"project_id"}` — any member adds their OWN Project's agent. The
/// owner's box is told (push) and woken so the agent joins the room.
pub async fn agent_add(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<AddAgentBody>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let project = match own_project(&state, &me, &b.project_id) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let humans = human_user_ids(&cm);
    if state.db.blocked_by_any(&me, &humans).unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_available",
            "this chat is not available",
        );
    }
    let member = format!("agent:{}", project.id);
    if let Err(e) = state
        .db
        .add_member(&id, &member, "agent", &me, &default_agent_settings())
    {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    // Same as adding a person: a DM with an agent is a group (header, members, profile).
    if cm.chat.kind == "dm" {
        let _ = state.db.set_people_chat_kind(&id, "group");
    }
    // The notice row goes out first (ARB-393): every device in the room sees "<me> added
    // <agent>" as soon as the membership row exists, before the box wake, the pushes and the
    // Matrix mirror have had their turn.
    crate::outside::on_membership(&state, &project.id, &id, true);
    let line = notices::added_line(
        &notices::person_name(&state, &me),
        std::slice::from_ref(&project.name),
    );
    if let Some(line) = line {
        let n = notices::Notice {
            kind: "added",
            actor: me.clone(),
            subjects: vec![notices::subject_agent(&project.id)],
        };
        if let Err(e) = notices::post_people_notice(&state, &id, &line, n).await {
            tracing::warn!(chat = %id, error = %e, "membership notice not written");
        }
    }
    state.matrix.touch_chat(&state, &id, None);
    agent_joined(&state, &cm, &me, &id, &project).await;
    match load_chat(&state, &id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// After an agent's membership row exists: wake its box, tell the agents already in the chat
/// to re-read members (they see the newcomer by handle), and push `people_agent_added`.
async fn agent_joined(
    state: &AppState,
    cm: &ChatWithMembers,
    me: &str,
    chat_id: &str,
    project: &crate::db::Project,
) {
    let _ = state.db.enqueue_push(
        &project.id,
        "people_agent_added",
        &json!({ "chat_id": chat_id }),
    );
    crate::boxes::ensure_awake(state, &project.user_id, "people").await;
    for m in &cm.members {
        let Some(pid) = m.project_id() else { continue };
        if pid == project.id {
            continue;
        }
        let _ = state
            .db
            .enqueue_push(pid, "people_agent_added", &json!({ "chat_id": chat_id }));
        crate::boxes::ensure_awake_project(state, pid, "people").await;
    }
    notify_members(
        state,
        cm,
        me,
        &PushEventDto::PeopleAgentAdded(PeopleAgentAddedPayloadDto {
            chat_id: chat_id.to_owned(),
            project_id: project.id.clone(),
            by_handle: handle_of(state, me),
        }),
    );
    tracing::info!(chat = %chat_id, project = %project.id, %me, "agent added to a people chat");
}

#[derive(Deserialize)]
pub struct AgentSettingsBody {
    #[serde(default)]
    pub reply_policy: Option<String>,
    #[serde(default)]
    pub restrict_guests: Option<bool>,
}

/// `PUT /chats/{id}/agents/{project_id}/settings` — the agent's owner only.
pub async fn agent_settings(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, project_id)): Path<(String, String)>,
    Json(b): Json<AgentSettingsBody>,
) -> Response {
    let (me, _cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(r) = own_project(&state, &me, &project_id) {
        return *r;
    }
    let member = format!("agent:{project_id}");
    let Ok(Some(m)) = state.db.membership(&id, &member) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "that agent is not in this chat",
        );
    };
    let mut settings = if m.settings.is_object() {
        m.settings
    } else {
        default_agent_settings()
    };
    if let Some(p) = b.reply_policy {
        if p != "mention" && p != "every_message" {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_policy",
                "reply_policy must be mention or every_message",
            );
        }
        settings["reply_policy"] = json!(p);
    }
    if let Some(r) = b.restrict_guests {
        settings["restrict_guests"] = json!(r);
    }
    if let Err(e) = state.db.set_member_settings(&id, &member, &settings) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    // The box re-reads members and settings through the same push as a join.
    let _ = state
        .db
        .enqueue_push(&project_id, "people_agent_added", &json!({ "chat_id": id }));
    match load_chat(&state, &id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// Leave every people chat this agent is in: box stop (`people_agent_removed`) + human roster
/// refresh. Used by `DELETE /chats/{id}/agents/{project}` and by project soft-delete so a
/// deleted agent does not linger as an @mention ghost that never replies.
pub fn detach_agent_from_people_chats(state: &AppState, project_id: &str, by_user: &str) {
    let member = format!("agent:{project_id}");
    let chats = state.db.chats_of_member(&member).unwrap_or_default();
    let by_handle = handle_of(state, by_user);
    for cm in chats {
        match state.db.remove_member(&cm.chat.id, &member, false) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::warn!(
                    chat = %cm.chat.id,
                    project_id,
                    error = %e,
                    "detach deleted agent from people chat"
                );
                continue;
            }
        }
        let _ = state.db.enqueue_push(
            project_id,
            "people_agent_removed",
            &json!({ "chat_id": cm.chat.id }),
        );
        notify_members(
            state,
            &cm,
            by_user,
            &PushEventDto::PeopleAgentRemoved(PeopleAgentRemovedPayloadDto {
                chat_id: cm.chat.id.clone(),
                project_id: project_id.to_owned(),
                by_handle: by_handle.clone(),
            }),
        );
    }
}

/// `DELETE /chats/{id}/agents/{project_id}` — the agent's owner only. The agent's queue ends at
/// once and its memory of the chat is soft-invalidated on the box.
pub async fn agent_remove(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, project_id)): Path<(String, String)>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Err(r) = own_project(&state, &me, &project_id) {
        return *r;
    }
    match state
        .db
        .remove_member(&id, &format!("agent:{project_id}"), false)
    {
        Ok(true) => {}
        Ok(false) => {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "that agent is not in this chat",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_chat(&state, &id, None);
    let _ = state.db.enqueue_push(
        &project_id,
        "people_agent_removed",
        &json!({ "chat_id": id }),
    );
    notify_members(
        &state,
        &cm,
        &me,
        &PushEventDto::PeopleAgentRemoved(PeopleAgentRemovedPayloadDto {
            chat_id: id.clone(),
            project_id: project_id.clone(),
            by_handle: handle_of(&state, &me),
        }),
    );
    crate::outside::on_membership(&state, &project_id, &id, false);
    let line = notices::removed_line(
        &notices::person_name(&state, &me),
        &[notices::agent_name(&state, &project_id)],
    );
    if let Some(line) = line {
        let n = notices::Notice {
            kind: "removed",
            actor: me.clone(),
            subjects: vec![notices::subject_agent(&project_id)],
        };
        if let Err(e) = notices::post_people_notice(&state, &id, &line, n).await {
            tracing::warn!(chat = %id, error = %e, "membership notice not written");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct AddMemberBody {
    /// One person (older clients).
    #[serde(default)]
    pub handle: Option<String>,
    /// Several people at once — one membership notice for the batch (#1349).
    #[serde(default)]
    pub handles: Vec<String>,
    /// The caller's own projects to add as agents, in the same batch.
    #[serde(default)]
    pub agent_ids: Vec<String>,
}

/// `POST /chats/{id}/members {"handle"}` or `{"handles": […], "agent_ids": […]}` — any human
/// member adds people (and their own agents). Everything is checked before anything is written;
/// one "<me> added A, B and C" line goes into the chat for the whole batch. A DM becomes a group
/// when a third member joins so the two can still open a fresh DM later.
pub async fn member_add(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<AddMemberBody>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let raw_handles: Vec<&str> = b
        .handle
        .iter()
        .map(String::as_str)
        .chain(b.handles.iter().map(String::as_str))
        .collect();
    let mut people: Vec<UserCardDto> = Vec::new();
    for raw in raw_handles {
        let Some(handle) = normalize_handle(raw) else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_handle",
                "that is not a GitHub login",
            );
        };
        let Some(other) = find_person(&state, &handle).await else {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no GitHub account with that login",
            );
        };
        let other_id = other.id.clone();
        if other_id == me {
            return api_error(StatusCode::BAD_REQUEST, "bad_handle", "that is you");
        }
        if people.iter().any(|p| p.id == other_id) {
            continue;
        }
        if cm
            .members
            .iter()
            .any(|m| m.user_id() == Some(other_id.as_str()))
        {
            return api_error(
                StatusCode::CONFLICT,
                "already_member",
                "already in this chat",
            );
        }
        if state.db.blocked_either_way(&me, &other_id).unwrap_or(false) {
            return api_error(
                StatusCode::FORBIDDEN,
                "not_available",
                "this person is not available",
            );
        }
        let humans = human_user_ids(&cm);
        if state.db.blocked_by_any(&other_id, &humans).unwrap_or(false) {
            return api_error(
                StatusCode::FORBIDDEN,
                "not_available",
                "this person is not available",
            );
        }
        if !may_add_to_group(&state, &me, &other_id) {
            return group_add_forbidden();
        }
        people.push(other);
    }
    let mut agents: Vec<crate::db::Project> = Vec::new();
    for pid in &b.agent_ids {
        if agents.iter().any(|p| &p.id == pid) {
            continue;
        }
        let project = match own_project(&state, &me, pid) {
            Ok(p) => p,
            Err(r) => return *r,
        };
        if cm
            .members
            .iter()
            .any(|m| m.project_id() == Some(project.id.as_str()))
        {
            return api_error(
                StatusCode::CONFLICT,
                "already_member",
                "that agent is already in this chat",
            );
        }
        agents.push(project);
    }
    if people.is_empty() && agents.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "need_members",
            "name at least one person or one of your agents",
        );
    }
    let mut names: Vec<String> = Vec::new();
    let mut subjects: Vec<serde_json::Value> = Vec::new();
    for other in &people {
        let other_id = other.id.clone();
        if state
            .db
            .member_is_banned(&id, &format!("user:{other_id}"))
            .unwrap_or(false)
        {
            return api_error(
                StatusCode::FORBIDDEN,
                "banned",
                "they were banned from this group",
            );
        }
        if let Err(e) =
            state
                .db
                .add_member(&id, &format!("user:{other_id}"), "member", &me, &json!({}))
        {
            if e.to_string().contains("banned") {
                return api_error(
                    StatusCode::FORBIDDEN,
                    "banned",
                    "they were banned from this group",
                );
            }
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        let event = PushEventDto::PeopleMemberAdded(PeopleMemberAddedPayloadDto {
            chat_id: id.clone(),
            user_id: other_id.clone(),
            handle: handle_of(&state, &other_id),
            by_handle: handle_of(&state, &me),
        });
        let _ = state.db.push_user_event(&other_id, &event, Some(&id));
        notify_members(&state, &cm, &me, &event);
        tracing::info!(chat = %id, user = %other_id, %me, "person added to a people chat");
        names.push(notices::person_name(&state, &other_id));
        subjects.push(notices::subject_user(&other_id));
    }
    for project in &agents {
        if let Err(e) = state.db.add_member(
            &id,
            &format!("agent:{}", project.id),
            "agent",
            &me,
            &default_agent_settings(),
        ) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
        names.push(project.name.clone());
        subjects.push(notices::subject_agent(&project.id));
        crate::outside::on_membership(&state, &project.id, &id, true);
    }
    if cm.chat.kind == "dm" {
        let _ = state.db.set_people_chat_kind(&id, "group");
    }
    // Notice row first (ARB-393), then the slower follow-up work: box wakes, agent pushes
    // and the Matrix mirror.
    if let Some(line) = notices::added_line(&notices::person_name(&state, &me), &names) {
        let n = notices::Notice {
            kind: "added",
            actor: me.clone(),
            subjects,
        };
        if let Err(e) = notices::post_people_notice(&state, &id, &line, n).await {
            tracing::warn!(chat = %id, error = %e, "membership notice not written");
        }
    }
    state.matrix.touch_chat(&state, &id, None);
    for project in &agents {
        agent_joined(&state, &cm, &me, &id, project).await;
    }
    match load_chat(&state, &id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// Owner or admin of a people group (not a plain member).
fn can_manage_members(cm: &ChatWithMembers, me: &str) -> bool {
    cm.members
        .iter()
        .any(|m| m.user_id() == Some(me) && (m.role == "owner" || m.role == "admin"))
}

/// My role in the chat, if I am a live human member.
fn my_role<'a>(cm: &'a ChatWithMembers, me: &str) -> Option<&'a str> {
    cm.members
        .iter()
        .find(|m| m.user_id() == Some(me))
        .map(|m| m.role.as_str())
}

/// True while an admin mute still blocks posting (honours timed `silenced_until`).
fn membership_is_silenced(m: &Membership) -> bool {
    if !m.silenced {
        return false;
    }
    !matches!(m.silenced_until, Some(until) if until <= now_ms())
}

/// Owner always; an admin only when the owner left `can_mute_ban` on (default true).
fn member_can_mute_ban(m: &Membership) -> bool {
    if m.role == "owner" {
        return true;
    }
    if m.role != "admin" {
        return false;
    }
    m.settings
        .get("can_mute_ban")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// May I remove this human target? Owners manage everyone else; admins manage
/// members only (not the owner or other admins).
/// Returns `Some(error response)` when refused.
fn can_moderate_member(cm: &ChatWithMembers, me: &str, target_id: &str) -> Option<Response> {
    if !can_manage_members(cm, me) {
        return Some(api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only an owner or admin can manage members",
        ));
    }
    if target_id == me {
        return Some(api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "use leave to remove yourself",
        ));
    }
    let target = cm.members.iter().find(|m| m.user_id() == Some(target_id));
    let Some(target) = target else {
        return Some(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "not a member",
        ));
    };
    if target.role == "owner" {
        return Some(api_error(
            StatusCode::FORBIDDEN,
            "owner_stays",
            "the owner cannot be removed",
        ));
    }
    if my_role(cm, me) == Some("admin") && target.role == "admin" {
        return Some(api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the owner manages an admin",
        ));
    }
    None
}

/// May I mute or ban this human? Same hierarchy as remove, plus the owner can
/// turn off mute/ban for individual admins (`can_mute_ban`).
fn can_mute_ban_member(cm: &ChatWithMembers, me: &str, target_id: &str) -> Option<Response> {
    if let Some(r) = can_moderate_member(cm, me, target_id) {
        return Some(r);
    }
    let Some(actor) = cm.members.iter().find(|m| m.user_id() == Some(me)) else {
        return Some(api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only an owner or admin can mute or ban",
        ));
    };
    if !member_can_mute_ban(actor) {
        return Some(api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "the owner has not given you mute and ban rights",
        ));
    }
    None
}

/// `DELETE /chats/{id}/members/{user_id}[?ban=1]` — owner/admin removes (or bans) a person.
pub async fn member_remove(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, user_id)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let ban = q.get("ban").is_some_and(|b| b == "1");
    // Ban needs mute/ban rights; plain remove is any owner/admin.
    if ban {
        if let Some(r) = can_mute_ban_member(&cm, &me, &user_id) {
            return r;
        }
    } else if let Some(r) = can_moderate_member(&cm, &me, &user_id) {
        return r;
    }
    match state.db.remove_member(&id, &format!("user:{user_id}"), ban) {
        Ok(true) => {}
        Ok(false) => return api_error(StatusCode::NOT_FOUND, "not_found", "not a member"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_chat(&state, &id, None);
    let event = PushEventDto::PeopleMemberRemoved(PeopleMemberRemovedPayloadDto {
        chat_id: id.clone(),
        user_id: user_id.clone(),
        handle: handle_of(&state, &user_id),
        by_handle: handle_of(&state, &me),
        banned: Some(ban),
    });
    let _ = state.db.push_user_event(&user_id, &event, None);
    notify_members(&state, &cm, &me, &event);
    let line = notices::removed_line(
        &notices::person_name(&state, &me),
        &[notices::person_name(&state, &user_id)],
    );
    if let Some(line) = line {
        let n = notices::Notice {
            kind: "removed",
            actor: me.clone(),
            subjects: vec![notices::subject_user(&user_id)],
        };
        if let Err(e) = notices::post_people_notice(&state, &id, &line, n).await {
            tracing::warn!(chat = %id, error = %e, "membership notice not written");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct MemberPatchBody {
    /// Mute (true) or unmute (false) from posting in this group.
    pub silenced: Option<bool>,
    /// When a mute ends (ms). Omit or null with `silenced: true` = until unmuted.
    #[serde(default)]
    pub silenced_until: Option<i64>,
    /// Owner only: set this person's role to `admin` or `member`.
    pub role: Option<String>,
    /// Owner only: whether this admin may mute and ban members (default true).
    pub can_mute_ban: Option<bool>,
}

/// `PUT /chats/{id}/members/{user_id}` — mute/unmute (with optional duration), or
/// owner sets role / mute-ban rights for an admin.
pub async fn member_patch(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, user_id)): Path<(String, String)>,
    Json(b): Json<MemberPatchBody>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let has_silence = b.silenced.is_some();
    let has_role = b.role.is_some();
    let has_rights = b.can_mute_ban.is_some();
    if !has_silence && !has_role && !has_rights {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "set silenced, role, or can_mute_ban",
        );
    }

    // Role / mute-ban rights: owner only, and only for someone else who is not the owner.
    if has_role || has_rights {
        if my_role(&cm, &me) != Some("owner") {
            return api_error(
                StatusCode::FORBIDDEN,
                "owner_only",
                "only the owner sets admin roles and mute-ban rights",
            );
        }
        if user_id == me {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "cannot change your own role or mute-ban rights",
            );
        }
        let target = cm
            .members
            .iter()
            .find(|m| m.user_id() == Some(user_id.as_str()));
        let Some(target) = target else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "not a member");
        };
        if target.role == "owner" {
            return api_error(
                StatusCode::FORBIDDEN,
                "owner_stays",
                "the owner cannot be demoted",
            );
        }
        if let Some(role) = b.role.as_deref() {
            if role != "admin" && role != "member" {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "role is admin or member",
                );
            }
            match state
                .db
                .set_member_role(&id, &format!("user:{user_id}"), role)
            {
                Ok(true) => {}
                Ok(false) => return api_error(StatusCode::NOT_FOUND, "not_found", "not a member"),
                Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
            }
            // Fresh admins get mute/ban on unless the same request sets it.
            if role == "admin" && b.can_mute_ban.is_none() {
                let mut settings = if target.settings.is_object() {
                    target.settings.clone()
                } else {
                    json!({})
                };
                settings["can_mute_ban"] = json!(true);
                let _ = state
                    .db
                    .set_member_settings(&id, &format!("user:{user_id}"), &settings);
            }
            if role == "member" {
                let mut settings = if target.settings.is_object() {
                    target.settings.clone()
                } else {
                    json!({})
                };
                if settings.get("can_mute_ban").is_some() {
                    if let Some(obj) = settings.as_object_mut() {
                        obj.remove("can_mute_ban");
                    }
                    let _ =
                        state
                            .db
                            .set_member_settings(&id, &format!("user:{user_id}"), &settings);
                }
            }
        }
        if let Some(can) = b.can_mute_ban {
            // Reload role after a possible promotion in this request.
            let role_now = b.role.as_deref().unwrap_or(target.role.as_str());
            if role_now != "admin" {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "mute-ban rights apply to admins only",
                );
            }
            let fresh = state
                .db
                .membership(&id, &format!("user:{user_id}"))
                .ok()
                .flatten();
            let mut settings = fresh
                .as_ref()
                .map(|m| {
                    if m.settings.is_object() {
                        m.settings.clone()
                    } else {
                        json!({})
                    }
                })
                .unwrap_or_else(|| json!({}));
            settings["can_mute_ban"] = json!(can);
            if let Err(e) = state
                .db
                .set_member_settings(&id, &format!("user:{user_id}"), &settings)
            {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
            }
        }
    }

    if let Some(silenced) = b.silenced {
        if let Some(r) = can_mute_ban_member(&cm, &me, &user_id) {
            return r;
        }
        let until = if silenced { b.silenced_until } else { None };
        match state
            .db
            .set_member_silenced(&id, &format!("user:{user_id}"), silenced, until)
        {
            Ok(true) => {}
            Ok(false) => return api_error(StatusCode::NOT_FOUND, "not_found", "not a member"),
            Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        }
    }

    state.matrix.touch_chat(&state, &id, None);
    let event = PushEventDto::PeopleChatUpdated(PeopleChatUpdatedPayloadDto {
        chat_id: id.clone(),
    });
    let _ = state.db.push_user_event(&user_id, &event, None);
    notify_members(&state, &cm, &me, &event);
    match load_chat(&state, &id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// `POST /chats/{id}/members/{user_id}/unban` — clear a ban so they can be added again.
pub async fn member_unban(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, user_id)): Path<(String, String)>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let mine = cm.members.iter().find(|m| m.user_id() == Some(me.as_str()));
    let allowed = match mine {
        Some(m) if m.role == "owner" => true,
        Some(m) if m.role == "admin" && member_can_mute_ban(m) => true,
        _ => false,
    };
    if !allowed {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only an owner or an admin with mute and ban rights can unban",
        );
    }
    match state.db.unban_member(&id, &format!("user:{user_id}")) {
        Ok(true) => {}
        Ok(false) => {
            return api_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "that person is not banned",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    state.matrix.touch_chat(&state, &id, None);
    notify_members(
        &state,
        &cm,
        &me,
        &PushEventDto::PeopleChatUpdated(PeopleChatUpdatedPayloadDto {
            chat_id: id.clone(),
        }),
    );
    match load_chat(&state, &id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// `GET /box/people/{chat_id}` — the chat as the owner's box sees it (members and settings).
pub async fn box_people_chat(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let Principal::Box { user_id } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "box only");
    };
    if !room_access(&state, &principal, &id) {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_member",
            "no agent of yours is in this chat",
        );
    }
    match load_chat(&state, &id) {
        Ok(cm) => Json(box_chat_json(&state, &cm, user_id, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// People chats a project's agent is in, for `/box/projects`.
pub fn people_chats_of_project(state: &AppState, project_id: &str, owner: &str) -> Vec<Value> {
    state
        .db
        .chats_of_member(&format!("agent:{project_id}"))
        .unwrap_or_default()
        .iter()
        .map(|cm| box_chat_json(state, cm, owner, "box"))
        .collect()
}

/// How many device ids a box-facing roster lists per person.
const MEMBER_DEVICES_MAX: usize = 64;

/// The chat as a box sees it: the apps' roster plus, on every human member, `devices` — the
/// ids that person's devices write into `deviceId`. A row that carries no `sender: user:<id>`
/// (an older app, `arbosctl`) is attributed by the box through this list instead of being
/// guessed as the owner's (#1580). Apps never get it; they have the row's author fields.
pub fn box_chat_json(state: &AppState, cm: &ChatWithMembers, me: &str, device_id: &str) -> Value {
    let mut chat = chat_json(state, cm, me, device_id);
    if let Some(members) = chat.get_mut("members").and_then(Value::as_array_mut) {
        for m in members {
            let Some(uid) = m.get("user_id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let ids = state
                .db
                .device_ids_of_user(&uid, MEMBER_DEVICES_MAX)
                .unwrap_or_default();
            if let Some(o) = m.as_object_mut() {
                o.insert("devices".into(), json!(ids));
            }
        }
    }
    chat
}

// ---- projects together (step 3) ----------------------------------------------------------------

pub(crate) fn project_members_json(state: &AppState, project: &crate::db::Project) -> Value {
    serde_json::to_value(project_members_dto(state, project)).expect("ProjectMembersDto")
}

fn project_member_dto(
    state: &AppState,
    user_id: &str,
    role: &str,
    joined_at: i64,
    added_by: Option<String>,
) -> ProjectMemberDto {
    let card = state
        .db
        .user_card(user_id)
        .ok()
        .flatten()
        .unwrap_or(UserCardDto {
            id: user_id.to_owned(),
            ..Default::default()
        });
    let seen = state.db.user_last_seen(user_id).ok().flatten();
    ProjectMemberDto {
        id: if card.id.is_empty() {
            user_id.to_owned()
        } else {
            card.id
        },
        handle: (!card.handle.is_empty()).then_some(card.handle),
        avatar_url: card.avatar_url,
        name: card.name,
        status: card.status,
        created_at: card.created_at.unwrap_or(0),
        user_id: user_id.to_owned(),
        role: role.to_owned(),
        joined_at,
        added_by,
        last_seen_at: seen.map(|t| t - t % 60_000),
        online: seen.is_some_and(|t| now_ms() - t < ONLINE_MS),
        avatar_color_index_effective: i64::from(crate::avatar::color_index(user_id)),
    }
}

pub(crate) fn project_members_dto(
    state: &AppState,
    project: &crate::db::Project,
) -> ProjectMembersDto {
    let mut members = vec![project_member_dto(
        state,
        &project.user_id,
        "owner",
        project.created_at,
        None,
    )];
    for (uid, added_by, joined_at) in state.db.project_members(&project.id).unwrap_or_default() {
        members.push(project_member_dto(
            state,
            &uid,
            "member",
            joined_at,
            Some(added_by),
        ));
    }
    ProjectMembersDto {
        project_id: project.id.clone(),
        owner: project.user_id.clone(),
        members,
    }
}

fn project_for(state: &AppState, id: &str) -> Result<crate::db::Project, Box<Response>> {
    match state.db.project(id) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(Box::new(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such project",
        ))),
        Err(e) => Err(Box::new(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db",
            e.to_string(),
        ))),
    }
}

#[derive(Deserialize)]
pub struct ProjectMemberBody {
    pub handle: String,
}

/// `POST /projects/{id}/members {"handle"}` — the owner invites a person into the project: they
/// see its chats in their rail and drive its agent.
pub async fn project_member_add(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ProjectMemberBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let project = match project_for(&state, &id) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !state.owns(&principal, &project.id) {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the project's owner invites people",
        );
    }
    let Some(handle) = normalize_handle(&b.handle) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_handle",
            "that is not a GitHub login",
        );
    };
    let Some(other) = find_person(&state, &handle).await else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no GitHub account with that login",
        );
    };
    let other_id = other.id.clone();
    if other_id == project.user_id {
        return api_error(StatusCode::BAD_REQUEST, "bad_handle", "that is the owner");
    }
    if state.db.blocked_either_way(&me, &other_id).unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_available",
            "this person is not available",
        );
    }
    match state.db.add_project_member(&project.id, &other_id, &me) {
        Ok(true) => state.matrix.touch_project(&state, &project.id),
        Ok(false) => {
            return api_error(
                StatusCode::CONFLICT,
                "already_member",
                "already in this project",
            )
        }
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    let _ = state.db.push_user_event(
        &other_id,
        &PushEventDto::PeopleProjectShared(PeopleProjectSharedPayloadDto {
            project_id: project.id.clone(),
            by_handle: handle_of(&state, &me),
        }),
        None,
    );
    tracing::info!(project = %project.id, guest = %other_id, %me, "project shared");
    Json(project_members_json(&state, &project)).into_response()
}

/// `GET /projects/{id}/members` — any member.
pub async fn project_members_list(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let project = match project_for(&state, &id) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !state.allows(&principal, &project.id) {
        return api_error(StatusCode::FORBIDDEN, "not_member", "not your project");
    }
    Json(project_members_json(&state, &project)).into_response()
}

/// `DELETE /projects/{id}/members/{user_id}` — owner only; a guest may remove themselves.
pub async fn project_member_remove(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, user_id)): Path<(String, String)>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let project = match project_for(&state, &id) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    if !state.owns(&principal, &project.id) && user_id != me {
        return api_error(
            StatusCode::FORBIDDEN,
            "owner_only",
            "only the project's owner removes people",
        );
    }
    match state.db.remove_project_member(&project.id, &user_id) {
        Ok(true) => state.matrix.touch_project(&state, &project.id),
        Ok(false) => return api_error(StatusCode::NOT_FOUND, "not_found", "not a member"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
    let _ = state.db.push_user_event(
        &user_id,
        &PushEventDto::ProjectMemberRemoved(ProjectMemberRemovedPayloadDto {
            project_id: project.id.clone(),
            by_handle: handle_of(&state, &me),
        }),
        None,
    );
    StatusCode::NO_CONTENT.into_response()
}

// ---- per-member chat settings: read receipts, mute -------------------------------------------

#[derive(Deserialize)]
pub struct ChatSettingsBody {
    /// Share "Seen by @me" with the other people in this chat (default true).
    #[serde(default)]
    pub read_receipts: Option<bool>,
    #[serde(default)]
    pub muted: Option<bool>,
    /// Archive (hide) this chat for me on every device; `false` brings it back (D78).
    #[serde(default)]
    pub archived: Option<bool>,
    /// Notification level for me in this chat: `all` | `mentions` | `none` | `inherit`
    /// (the account default, `PUT /me/notifications`). Any chat kind.
    #[serde(default)]
    pub notifications: Option<String>,
}

/// Flip the archive state for the caller and tell their other devices (`chat_archived`).
fn apply_archive(
    state: &AppState,
    principal: &Principal,
    me: &str,
    chat_id: &str,
    project_label: &str,
    archived: bool,
) -> Result<Option<i64>, Box<Response>> {
    let at = state
        .db
        .set_chat_archived(me, chat_id, archived)
        .map_err(|e| {
            Box::new(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db",
                e.to_string(),
            ))
        })?;
    let _ = state.db.push_user_except_event(
        me,
        &device_of(principal),
        &PushEventDto::ChatArchived(ChatArchivedPayloadDto {
            chat_id: chat_id.to_owned(),
            project_id: project_label.to_owned(),
            archived_at: at,
        }),
    );
    state.matrix.archive(state, chat_id, me, archived);
    Ok(at)
}

/// `PUT /chats/{id}/settings {"read_receipts"?, "muted"?, "archived"?, "notifications"?}` — my
/// own settings for this chat. The answer carries `notifications` (the level set here, or
/// `inherit`).
/// Receipts themselves travel in the room (Presence frames `{seen: <message id>}`); clients of
/// a member with `read_receipts: false` simply do not send them, and others do not show theirs
/// to that member.
pub async fn chat_settings(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<ChatSettingsBody>,
) -> Response {
    // A project chat (plain or group, own or shared) reaches here too: only `archived` applies.
    if state.db.people_chat(&id).ok().flatten().is_none() {
        let me = match user_of(&principal) {
            Ok(u) => u,
            Err(r) => return *r,
        };
        let Ok(Some(project_id)) = state.db.chat_project_id(&id) else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat");
        };
        if !state.allows(&principal, &project_id) {
            return api_error(StatusCode::FORBIDDEN, "not_member", "not your chat");
        }
        if b.read_receipts.is_some() || b.muted.is_some() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "only archived and notifications apply to a project chat",
            );
        }
        if let Some(a) = b.archived {
            if let Err(r) = apply_archive(&state, &principal, &me, &id, &project_id, a) {
                return *r;
            }
        }
        if let Err(r) = apply_notify_level(&state, &me, &id, b.notifications.as_deref()) {
            return *r;
        }
        return match state.db.chat(&project_id, &id) {
            Ok(Some(c)) => {
                let mut v = crate::api::chat_json_for(&state, &c, Some(&me));
                v["notifications"] = json!(notify_level_of(&state, &me, &id));
                Json(v).into_response()
            }
            Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "no such chat"),
            Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
        };
    }
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if let Some(a) = b.archived {
        if let Err(r) = apply_archive(&state, &principal, &me, &id, PEOPLE_PROJECT, a) {
            return *r;
        }
    }
    let member = format!("user:{me}");
    let mut settings = cm
        .members
        .iter()
        .find(|m| m.member == member)
        .map(|m| m.settings.clone())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(r) = b.read_receipts {
        settings["read_receipts"] = json!(r);
    }
    if let Err(e) = state.db.set_member_settings(&id, &member, &settings) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    if let Some(m) = b.muted {
        if let Err(e) = state.db.set_member_muted(&id, &member, m) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
        }
    }
    if let Err(r) = apply_notify_level(&state, &me, &id, b.notifications.as_deref()) {
        return *r;
    }
    match load_chat(&state, &id) {
        Ok(cm) => {
            let mut v = chat_json(&state, &cm, &me, &device_of(&principal));
            v["notifications"] = json!(notify_level_of(&state, &me, &id));
            Json(v).into_response()
        }
        Err(r) => *r,
    }
}

/// Store the caller's notification level for a chat when the body names one.
fn apply_notify_level(
    state: &AppState,
    me: &str,
    chat_id: &str,
    level: Option<&str>,
) -> Result<(), Box<Response>> {
    let Some(word) = level else {
        return Ok(());
    };
    let Some(level) = crate::push::eligibility::Level::parse(word) else {
        return Err(Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "notifications must be all, mentions, none or inherit",
        )));
    };
    state
        .db
        .set_chat_notify_level(me, chat_id, level)
        .map_err(|e| {
            Box::new(api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db",
                e.to_string(),
            ))
        })
}

fn notify_level_of(state: &AppState, me: &str, chat_id: &str) -> &'static str {
    state
        .db
        .chat_notify_level(me, chat_id)
        .unwrap_or(crate::push::eligibility::Level::Inherit)
        .as_str()
}

// ---- invite links (people who are not on Arbos yet) -----------------------------------------

const INVITE_TTL_MS: i64 = 30 * 24 * 3_600_000;

fn invite_code() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut raw = [0u8; 10];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut raw);
    raw.iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

fn invite_json(state: &AppState, inv: &Invite) -> Value {
    let mut v = serde_json::to_value(inv).unwrap_or_default();
    v["url"] = json!(format!(
        "{}/i/{}",
        state.cfg.site_url.trim_end_matches('/'),
        inv.code
    ));
    v["inviter_handle"] = json!(handle_of(state, &inv.inviter_user_id));
    v["handle"] = json!(inv.github_login);
    if inv.target_kind == "project" {
        if let Ok(Some(p)) = state.db.project(&inv.target_id) {
            v["project"] = json!({ "id": p.id, "name": p.name, "icon": p.icon, "color": p.color });
        }
    }
    v
}

#[derive(Deserialize)]
pub struct InviteBody {
    pub handle: String,
    /// The DM to land in (default: the DM with that person, opened now if needed).
    #[serde(default)]
    pub chat_id: Option<String>,
    /// A project to share instead (the person becomes a member on sign-in; the DM is not made).
    #[serde(default)]
    pub project_id: Option<String>,
}

/// `POST /invites {"handle", "chat_id"? | "project_id"?}` — a link for someone who has not
/// signed in yet. The person becomes an `invited` placeholder bound to their GitHub id; the DM
/// (or project membership) exists for them already, so the link only has to get them to sign in
/// as that login. 30 days; the same open invite is returned when asked again.
pub async fn invite_create(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<InviteBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let Some(handle) = normalize_handle(&b.handle) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_handle",
            "that is not a GitHub login",
        );
    };
    let Some(other) = find_person(&state, &handle).await else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no GitHub account with that login",
        );
    };
    let other_id = other.id.clone();
    let login = if other.handle.is_empty() {
        handle.clone()
    } else {
        other.handle.clone()
    };
    if other_id == me {
        return api_error(StatusCode::BAD_REQUEST, "bad_handle", "that is you");
    }
    let (kind, target) = if let Some(pid) = b.project_id.as_deref() {
        let project = match project_for(&state, pid) {
            Ok(p) => p,
            Err(r) => return *r,
        };
        if !state.owns(&principal, &project.id) {
            return api_error(
                StatusCode::FORBIDDEN,
                "owner_only",
                "only the project's owner invites people",
            );
        }
        let _ = state.db.add_project_member(&project.id, &other_id, &me);
        state.matrix.touch_project(&state, &project.id);
        ("project", project.id)
    } else if let Some(cid) = b.chat_id.as_deref() {
        let (_, cm) = match member_chat(&state, &principal, cid) {
            Ok(v) => v,
            Err(r) => return *r,
        };
        if !cm
            .members
            .iter()
            .any(|m| m.user_id() == Some(other_id.as_str()))
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                "not_member",
                "that person is not in this chat",
            );
        }
        ("dm", cm.chat.id)
    } else {
        let chat = match state.db.dm_between(&me, &other_id) {
            Ok(Some(c)) => c,
            _ => {
                if state.db.blocked_either_way(&me, &other_id).unwrap_or(false) {
                    return api_error(
                        StatusCode::FORBIDDEN,
                        "not_available",
                        "this person is not available",
                    );
                }
                let chat = PeopleChat {
                    id: ulid(),
                    kind: "dm".into(),
                    created_by: me.clone(),
                    created_at: now_ms(),
                    title: None,
                    image_key: None,
                    avatar_leaf: None,
                    avatar_leaf_color: None,
                    description: None,
                };
                if let Err(e) = state.db.insert_people_chat(&chat) {
                    return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
                }
                for uid in [&me, &other_id] {
                    let _ = state.db.add_member(
                        &chat.id,
                        &format!("user:{uid}"),
                        "owner",
                        &me,
                        &json!({}),
                    );
                }
                state.matrix.touch_chat(&state, &chat.id, None);
                chat
            }
        };
        ("dm", chat.id)
    };
    if let Ok(Some(existing)) = state.db.open_invite_for(&login, kind, &target) {
        return Json(invite_json(&state, &existing)).into_response();
    }
    let inv = Invite {
        code: invite_code(),
        github_login: login,
        inviter_user_id: me,
        target_kind: kind.into(),
        target_id: target,
        created_at: now_ms(),
        expires_at: now_ms() + INVITE_TTL_MS,
        accepted_at: None,
        accepted_user_id: None,
        revoked_at: None,
    };
    if let Err(e) = state.db.insert_invite(&inv) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    (StatusCode::CREATED, Json(invite_json(&state, &inv))).into_response()
}

/// `GET /invites` — the caller's invites, newest first.
pub async fn invites_list(State(state): State<AppState>, principal: Principal) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let out: Vec<Value> = state
        .db
        .invites_by(&me)
        .unwrap_or_default()
        .iter()
        .map(|i| invite_json(&state, i))
        .collect();
    Json(out).into_response()
}

/// `DELETE /invites/{code}` — the inviter withdraws an open invite.
pub async fn invite_revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path(code): Path<String>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    match state.db.revoke_invite(&code, &me) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no open invite with that code",
        ),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

/// `GET /i/{code}` — the landing behind `arbos.life/i/<code>`: JSON for apps, a small HTML page
/// for browsers (who invited, as which login to sign in, the download link). No auth.
pub async fn invite_landing(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let inv = match state.db.invite(&code) {
        Ok(Some(i)) => i,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "no such invite"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let open = inv.accepted_at.is_none() && inv.revoked_at.is_none() && inv.expires_at > now_ms();
    let mut v = invite_json(&state, &inv);
    v["open"] = json!(open);
    v["download_url"] = json!(format!(
        "{}/download/mac",
        state.cfg.public_url.trim_end_matches('/')
    ));
    v["sign_in_hint"] = json!(format!("Sign in with GitHub as @{}", inv.github_login));
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    if !wants_html {
        return Json(v).into_response();
    }
    let inviter = v["inviter_handle"].as_str().unwrap_or("someone").to_owned();
    let what = match inv.target_kind.as_str() {
        "project" => format!(
            "work with them in the project “{}”",
            v["project"]["name"].as_str().unwrap_or("a project")
        ),
        _ => "chat with them".to_owned(),
    };
    let body = if open {
        format!(
            "<p><strong>@{inviter}</strong> invited you to Arbos to {what}.</p>\
             <p>Download Arbos, then sign in with GitHub as <strong>@{}</strong> — the chat is already waiting for you.</p>\
             <p><a class=\"btn\" href=\"{}\">Download for Mac</a></p>",
            html_escape(&inv.github_login),
            v["download_url"].as_str().unwrap_or("/download/mac")
        )
    } else {
        "<p>This invite is no longer open.</p>".to_owned()
    };
    let page = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Arbos invite</title>\
         <style>body{{font:16px/1.5 -apple-system,system-ui,sans-serif;background:#0b0b0d;color:#e8e8ea;display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}}\
         main{{max-width:28rem;padding:2rem}}h1{{font-size:1.4rem;margin:0 0 1rem}}.btn{{display:inline-block;background:#e8e8ea;color:#0b0b0d;padding:.6rem 1rem;border-radius:.6rem;text-decoration:none;font-weight:600}}</style></head>\
         <body><main><h1>Arbos</h1>{body}</main></body></html>"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---- group chat invite links (join by sharing a link) -------------------------------------------

/// Seven days: the only expiry the apps offer (same as server invites).
const GROUP_INVITE_EXPIRY_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Group invite links live on the site (`arbos.life/g/<code>` deep-links into the app) when
/// `ARBOS_SITE_URL` is set; the relay's own `/g/<code>` otherwise.
fn group_invite_url(state: &AppState, code: &str) -> String {
    let base = std::env::var("ARBOS_SITE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| state.cfg.public_url.clone());
    format!("{}/g/{}", base.trim_end_matches('/'), code)
}

fn group_invite_json(state: &AppState, i: &GroupInvite) -> Value {
    json!({
        "id": i.id, "code": i.code, "url": group_invite_url(state, &i.code),
        "chat_id": i.chat_id, "created_by": i.created_by,
        "created_at": i.created_at, "expires_at": i.expires_at, "uses_left": i.uses_left,
        "usable": i.usable(now_ms()),
    })
}

#[derive(Deserialize, Default)]
pub struct GroupInviteBody {
    /// Expire after seven days.
    #[serde(default)]
    pub expires: bool,
    /// Number of joins the link allows; absent = unlimited.
    #[serde(default)]
    pub max_uses: Option<i64>,
}

/// `GET /chats/{id}/invites` — invite links for a people group (any human member).
pub async fn group_invites_list(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let (_me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if cm.chat.kind != "group" {
        return api_error(
            StatusCode::BAD_REQUEST,
            "not_group",
            "invite links are for group chats",
        );
    }
    let list: Vec<Value> = state
        .db
        .invites_of_group(&id)
        .unwrap_or_default()
        .iter()
        .map(|i| group_invite_json(&state, i))
        .collect();
    Json(json!({ "invites": list })).into_response()
}

/// `POST /chats/{id}/invites {expires?, max_uses?}` — any human member mints a link.
pub async fn group_invites_create(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    body: Option<Json<GroupInviteBody>>,
) -> Response {
    let (me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if cm.chat.kind != "group" {
        return api_error(
            StatusCode::BAD_REQUEST,
            "not_group",
            "invite links are for group chats",
        );
    }
    let b = body.map(|b| b.0).unwrap_or_default();
    let max_uses = b.max_uses.filter(|n| *n > 0).map(|n| n.min(1000));
    let inv = GroupInvite {
        id: ulid(),
        chat_id: id.clone(),
        code: invite_code(),
        created_by: me,
        created_at: now_ms(),
        expires_at: b.expires.then(|| now_ms() + GROUP_INVITE_EXPIRY_MS),
        uses_left: max_uses,
        revoked_at: None,
    };
    if let Err(e) = state.db.insert_group_invite(&inv) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    (StatusCode::CREATED, Json(group_invite_json(&state, &inv))).into_response()
}

/// `DELETE /chats/{id}/invites/{iid}` — any human member revokes a link.
pub async fn group_invites_revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, iid)): Path<(String, String)>,
) -> Response {
    let (_me, cm) = match member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if cm.chat.kind != "group" {
        return api_error(
            StatusCode::BAD_REQUEST,
            "not_group",
            "invite links are for group chats",
        );
    }
    match state.db.revoke_group_invite(&id, &iid) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "not_found", "no such invite"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct GroupJoinBody {
    /// Invite link or its code.
    pub code: String,
}

/// `POST /chats/join {code}` — join a people group from an invite link.
pub async fn group_join(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<GroupJoinBody>,
) -> Response {
    let me = match user_of(&principal) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let code = b
        .code
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    if code.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "paste a group invite link",
        );
    }
    let Some(inv) = state
        .db
        .group_invite_by_code(&code)
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
    let cm = match load_chat(&state, &inv.chat_id) {
        Ok(cm) => cm,
        Err(r) => return *r,
    };
    if cm.chat.kind != "group" {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "that invite link is not valid any more",
        );
    }
    // Already a member: return the chat (re-tapping the link).
    if cm.members.iter().any(|m| m.user_id() == Some(me.as_str())) {
        return Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response();
    }
    if state
        .db
        .blocked_either_way(&me, &cm.chat.created_by)
        .unwrap_or(false)
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_available",
            "this group is not available",
        );
    }
    let humans = human_user_ids(&cm);
    if state.db.blocked_by_any(&me, &humans).unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_available",
            "this group is not available",
        );
    }
    if inv.uses_left.is_some() && !state.db.consume_group_invite_use(&inv.id).unwrap_or(false) {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "that invite link has been used up",
        );
    }
    if let Err(e) = state.db.add_member(
        &inv.chat_id,
        &format!("user:{me}"),
        "member",
        &me,
        &json!({}),
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string());
    }
    let event = PushEventDto::PeopleMemberAdded(PeopleMemberAddedPayloadDto {
        chat_id: inv.chat_id.clone(),
        user_id: me.clone(),
        handle: handle_of(&state, &me),
        by_handle: handle_of(&state, &inv.created_by),
    });
    notify_members(&state, &cm, &me, &event);
    state.matrix.touch_chat(&state, &inv.chat_id, None);
    let line = format!("{} joined", notices::person_name(&state, &me));
    let n = notices::Notice {
        kind: "joined",
        actor: me.clone(),
        subjects: vec![],
    };
    if let Err(e) = notices::post_people_notice(&state, &inv.chat_id, &line, n).await {
        tracing::warn!(chat = %inv.chat_id, error = %e, "join notice not written");
    }
    match load_chat(&state, &inv.chat_id) {
        Ok(cm) => Json(chat_json(&state, &cm, &me, &device_of(&principal))).into_response(),
        Err(r) => *r,
    }
}

/// `GET /g/{code}` — the group invite link. JSON for the apps; for a browser an HTML page that
/// opens `arbos://join-group/<code>` and otherwise explains how to join.
pub async fn group_invite_landing(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    let inv = state.db.group_invite_by_code(&code).ok().flatten();
    let chat = inv
        .as_ref()
        .and_then(|i| state.db.people_chat(&i.chat_id).ok().flatten());
    let (Some(inv), Some(chat)) = (inv, chat) else {
        return if wants_html {
            group_invite_page("<p>This invite link is not valid.</p>")
        } else {
            api_error(StatusCode::NOT_FOUND, "not_found", "no such invite")
        };
    };
    let usable = inv.usable(now_ms());
    let members = state
        .db
        .members_of(&chat.id)
        .map(|m| m.iter().filter(|m| m.user_id().is_some()).count())
        .unwrap_or(0);
    let title = chat
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("a group");
    let download = format!(
        "{}/download/mac",
        state.cfg.public_url.trim_end_matches('/')
    );
    if !wants_html {
        return Json(json!({
            "code": code, "usable": usable, "expires_at": inv.expires_at, "uses_left": inv.uses_left,
            "chat": { "id": chat.id, "title": title, "member_count": members, "image_key": chat.image_key },
            "download_url": download,
        }))
        .into_response();
    }
    if !usable {
        let why = if inv.revoked_at.is_some() {
            "was revoked"
        } else if inv.uses_left == Some(0) {
            "has been used up"
        } else {
            "has expired"
        };
        return group_invite_page(&format!(
            "<p>This invite to <strong>{}</strong> {why}. Ask someone in the group for a new link.</p>",
            html_escape(title)
        ));
    }
    let safe_code = html_escape(&code);
    group_invite_page_with_head(
        &format!("<meta http-equiv=\"refresh\" content=\"0;url=arbos://join-group/{safe_code}\">"),
        &format!(
            "<p>You are invited to the group <strong>{name}</strong> on Arbos ({members} members).</p>\
             <p><a class=\"btn\" href=\"arbos://join-group/{safe_code}\">Open in Arbos</a></p>\
             <p>No Arbos yet? <a href=\"{download}\">Download for Mac</a>, then open this link again.</p>",
            name = html_escape(title),
        ),
    )
}

fn group_invite_page(body: &str) -> Response {
    group_invite_page_with_head("", body)
}

fn group_invite_page_with_head(head: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Arbos group invite</title>{head}\
         <style>body{{font:16px/1.5 -apple-system,system-ui,sans-serif;background:#0b0b0d;color:#e8e8ea;display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}}\
         main{{max-width:28rem;padding:2rem}}h1{{font-size:1.4rem;margin:0 0 1rem}}.btn{{display:inline-block;background:#e8e8ea;color:#0b0b0d;padding:.6rem 1rem;border-radius:.6rem;text-decoration:none;font-weight:600}}</style></head>\
         <body><main><h1>Arbos</h1>{body}</main></body></html>"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

// ---- room hooks ---------------------------------------------------------------------------------

/// Why a row push into a people room was refused (sent back as an `error` frame). The wire
/// code stays `rate_limited` / `not_available` — fielded clients keep a room Live only for
/// those two (`arbos-client::room::on_error`), so a new code would make every old client drop
/// and redial the room on each retry. A `not_available` frame says *why* in `reason`
/// ([feedback blocked-state-relay-2026-10-26]): `blocked_by`, `blocks_member`, `silenced`,
/// `chat_unavailable`; a block also names the other party as `byHandle` / `byName`. New clients
/// key on `reason`; old clients behave as before.
#[derive(Debug)]
pub enum PushRefused {
    /// Over the message rate limit. `retry_s` is seconds until the oldest counted event leaves
    /// the 60 s window (carried as `retryS` on the error frame).
    RateLimited { retry_s: i64 },
    /// A block stands between the two people of a DM: `blocked_by` = the other person blocked
    /// the sender; `blocks_member` = the sender blocked them. Groups are never refused for a
    /// block (the rows are hidden from the blocker alone).
    Blocked {
        reason: BlockReason,
        by_handle: String,
        by_name: Option<String>,
    },
    /// An admin muted the sender in this chat.
    Silenced,
    /// No such chat (or nothing else the relay can name).
    NotAvailable,
}

/// Which way the block runs, from the sender's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Someone in the chat has blocked the sender.
    BlockedBy,
    /// The sender has blocked the other person (a DM: nothing to post into).
    BlocksMember,
}

impl BlockReason {
    pub fn code(self) -> &'static str {
        match self {
            BlockReason::BlockedBy => "blocked_by",
            BlockReason::BlocksMember => "blocks_member",
        }
    }
}

impl PushRefused {
    pub fn code(&self) -> &'static str {
        match self {
            PushRefused::RateLimited { .. } => "rate_limited",
            PushRefused::Blocked { .. } | PushRefused::Silenced | PushRefused::NotAvailable => {
                "not_available"
            }
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            PushRefused::RateLimited { .. } => "slow down",
            PushRefused::Blocked {
                reason: BlockReason::BlockedBy,
                ..
            } => "someone in this chat has blocked you",
            PushRefused::Blocked {
                reason: BlockReason::BlocksMember,
                ..
            } => "you have blocked this person",
            PushRefused::Silenced => "you are muted in this chat",
            PushRefused::NotAvailable => "this chat is not available",
        }
    }

    pub fn retry_s(&self) -> Option<i64> {
        match self {
            PushRefused::RateLimited { retry_s } => Some(*retry_s),
            PushRefused::Blocked { .. } | PushRefused::Silenced | PushRefused::NotAvailable => None,
        }
    }

    /// The `reason` field of a `not_available` frame; `None` on `rate_limited`.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            PushRefused::RateLimited { .. } => None,
            PushRefused::Blocked { reason, .. } => Some(reason.code()),
            PushRefused::Silenced => Some("silenced"),
            PushRefused::NotAvailable => Some("chat_unavailable"),
        }
    }

    /// `(byHandle, byName)` of the other party on a block.
    pub fn other_party(&self) -> Option<(&str, Option<&str>)> {
        match self {
            PushRefused::Blocked {
                by_handle, by_name, ..
            } => Some((by_handle.as_str(), by_name.as_deref())),
            PushRefused::RateLimited { .. } | PushRefused::Silenced | PushRefused::NotAvailable => {
                None
            }
        }
    }
}

/// The block that stops `user_id` posting into `cm`, if any — **DMs only** (Jacob,
/// 2026-10-26 17:28 CR): the other person blocked the sender (`blocked_by`) or the sender
/// blocked them (`blocks_member`). In a group a block never refuses the room: the blocked
/// person's rows are hidden from the blocker alone (`blocked_senders` on the chat DTO, the
/// blocker's pushes / alerts / unread skip them) and everyone else sees them as before.
fn block_in_the_way(state: &AppState, user_id: &str, cm: &ChatWithMembers) -> Option<PushRefused> {
    if cm.chat.kind != "dm" {
        return None;
    }
    let humans = human_user_ids(cm);
    let other_party = |other: &str| {
        let card = state.db.user_card(other).ok().flatten();
        let by_handle = card
            .as_ref()
            .map(|c| c.handle.clone())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| other.to_owned());
        let by_name = card.and_then(|c| c.name).filter(|n| !n.trim().is_empty());
        (by_handle, by_name)
    };
    for other in humans.iter().filter(|o| o.as_str() != user_id) {
        if state.db.blocks(other, user_id).unwrap_or(false) {
            let (by_handle, by_name) = other_party(other);
            return Some(PushRefused::Blocked {
                reason: BlockReason::BlockedBy,
                by_handle,
                by_name,
            });
        }
        if state.db.blocks(user_id, other).unwrap_or(false) {
            let (by_handle, by_name) = other_party(other);
            return Some(PushRefused::Blocked {
                reason: BlockReason::BlocksMember,
                by_handle,
                by_name,
            });
        }
    }
    None
}

/// Called on every row a device pushes into `people/<chat>`: rate limit, block check, a
/// coalesced `people_message` push to the other people in the chat. Returns the users whose
/// boxes host an agent of this chat, so the caller can wake them.
/// May this person write into this chat at all? A DM partner's block either way, or an admin
/// silence, says no. Shared by the room push and `/chats/{id}/notify` (#2012), so the
/// announcement path is never looser than the row path.
fn sender_allowed(
    state: &AppState,
    user_id: &str,
    cm: &ChatWithMembers,
) -> Result<(), PushRefused> {
    if let Some(r) = block_in_the_way(state, user_id, cm) {
        return Err(r);
    }
    // Admin mute from posting: silenced members cannot send into the room.
    if cm
        .members
        .iter()
        .any(|m| m.user_id() == Some(user_id) && membership_is_silenced(m))
    {
        return Err(PushRefused::Silenced);
    }
    Ok(())
}

/// [`sender_allowed`] for a principal: devices are gated as their person, boxes (agents) pass,
/// a chat that does not load is `NotAvailable`. One `warn!` per refusal, like the room push.
pub fn sender_gate(
    state: &AppState,
    principal: &Principal,
    chat_id: &str,
) -> Result<(), PushRefused> {
    let Principal::Device { user_id, .. } = principal else {
        return Ok(());
    };
    let Ok(cm) = load_chat(state, chat_id) else {
        return Err(PushRefused::NotAvailable);
    };
    sender_allowed(state, user_id, &cm).inspect_err(|r| {
        tracing::warn!(
            user = %user_id,
            chat = %chat_id,
            code = r.code(),
            reason = r.reason().unwrap_or("-"),
            by = r.other_party().map(|(h, _)| h).unwrap_or("-"),
            "people notify refused"
        );
    })
}

/// The agents in a people chat, with the user whose box serves each (soft-deleted agents are
/// gone already: `project()` is `None` for them).
pub fn serving_agents(state: &AppState, cm: &ChatWithMembers) -> Vec<crate::boxes::ServingAgent> {
    let mut agents: Vec<crate::boxes::ServingAgent> = cm
        .members
        .iter()
        .filter_map(|m| m.project_id())
        // Outside agents are served by the relay, not by a box (#2140).
        .filter(|pid| !crate::outside::is_outside(state, pid))
        .filter_map(|pid| state.db.project(pid).ok().flatten())
        .map(|p| crate::boxes::ServingAgent {
            project_id: p.id,
            owner: p.user_id,
        })
        .collect();
    agents.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    agents.dedup();
    agents
}

/// The two collective mentions the apps offer (Jacob, 2026-10-24): `@here` is everyone in the
/// chat — its people and its agents — and `@agents` is every agent, so it names no person. The
/// apps expand them into member ids on the row (`expand_collectives`,
/// `arbos-mac/src/sync/mod.rs`), so a reader of the row sees members and never these words,
/// while a reader of the announce sees only the words.
pub const COLLECTIVE_HERE: &str = "here";
pub const COLLECTIVE_AGENTS: &str = "agents";

/// `@token` as a whole word in `lower` (already lower-case), by the apps' rule: the character
/// before the `@` is not part of a word and the token is not continued by one. Stricter than
/// the handle scan in `push::eligibility`, which looks only at the character after — that is
/// tolerable for a handle nobody writes in an address, and not for `@here`, which would
/// otherwise make a group-wide ping out of `mail me@here.example`.
pub fn at_token(lower: &str, token: &str) -> bool {
    let needle = format!("@{token}");
    lower.match_indices(&needle).any(|(i, _)| {
        let before = lower[..i].chars().last();
        let after = lower[i + needle.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric() && c != '@')
            && after.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
    })
}

/// Does `lower` (already lower-case) name a collective?
pub fn names_a_collective(lower: &str) -> bool {
    at_token(lower, COLLECTIVE_HERE) || at_token(lower, COLLECTIVE_AGENTS)
}

/// Who `text` mentions, as the row's `mentions`: every `@handle` that names a member
/// (`agent:<project>` by the agent's slug, `user:<id>` by the person's handle), then the
/// collectives expanded into member ids the way the apps expand them — `@here` to everyone but
/// `me`, `@agents` to the chat's agents. `me` is the writer's own member id, who is never a
/// mention of their own message; pass an empty string when there is no writer to exclude.
///
/// A collective that names nobody (`@agents` where the chat has none) adds nothing: the array is
/// a list of members, so when there are no members to name it stays as it was. The prose still
/// says the word, which is what `push::eligibility::mentioned` reads, so a person on
/// `mentions` is still told either way.
pub fn resolve_mentions(
    state: &AppState,
    cm: &ChatWithMembers,
    text: &str,
    me: &str,
) -> Vec<String> {
    let mut by_handle: Vec<(String, String)> = Vec::new();
    for m in &cm.members {
        if let Some(pid) = m.project_id() {
            if let Ok(Some(p)) = state.db.project(pid) {
                by_handle.push((agent_slug(&p.name), format!("agent:{pid}")));
            }
        } else if let Some(uid) = m.user_id() {
            if let Ok(Some(card)) = state.db.user_card(uid) {
                if !card.handle.is_empty() {
                    by_handle.push((card.handle.to_lowercase(), format!("user:{uid}")));
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    for token in text.split(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ':' | ';' | '!' | '?' | '(' | ')' | '"' | '\'')
    }) {
        let Some(handle) = token.strip_prefix('@') else {
            continue;
        };
        let handle = handle.trim_end_matches('.').to_lowercase();
        if let Some((_, member)) = by_handle.iter().find(|(h, _)| *h == handle) {
            if !out.contains(member) {
                out.push(member.clone());
            }
        }
    }
    let lower = text.to_lowercase();
    let here = at_token(&lower, COLLECTIVE_HERE);
    if here || at_token(&lower, COLLECTIVE_AGENTS) {
        for m in &cm.members {
            // A soft-deleted agent is no longer a member to name, as the handle scan above has
            // it: `project()` is `None` for one.
            let names_it = match m.project_id() {
                Some(pid) => state.db.project(pid).ok().flatten().is_some(),
                None => here,
            };
            if names_it && m.member != me && !out.contains(&m.member) {
                out.push(m.member.clone());
            }
        }
    }
    out
}

/// Gates a person's row into a people chat and returns the agents whose boxes it must reach.
pub fn on_room_push(
    state: &AppState,
    principal: &Principal,
    chat_id: &str,
) -> Result<Vec<crate::boxes::ServingAgent>, PushRefused> {
    let Principal::Device { user_id, .. } = principal else {
        return Ok(Vec::new()); // boxes (agents) are metered by their owner's credits, not here
    };
    let refused = |r: PushRefused| {
        // One line per refusal (#1990): who, where, why — the Arbonites outage had none.
        tracing::warn!(
            user = %user_id,
            chat = %chat_id,
            code = r.code(),
            reason = r.reason().unwrap_or("-"),
            by = r.other_party().map(|(h, _)| h).unwrap_or("-"),
            "people push refused"
        );
        Err(r)
    };
    let Ok(cm) = load_chat(state, chat_id) else {
        return refused(PushRefused::NotAvailable);
    };
    let humans = human_user_ids(&cm);
    if let Err(r) = sender_allowed(state, user_id, &cm) {
        return refused(r);
    }
    let limit = if young_account(state, user_id) {
        MSGS_PER_MIN / 2
    } else {
        MSGS_PER_MIN
    };
    // Count first; insert only when this push is allowed. A refused reconnect storm must not
    // keep the user over the limit forever.
    match state.db.rate_check_and_note(user_id, "msg", 60_000, limit) {
        Ok(true) => {}
        Ok(false) => {
            let retry_s = state.db.rate_retry_s(user_id, "msg", 60_000).unwrap_or(1);
            return refused(PushRefused::RateLimited { retry_s });
        }
        Err(_) => {
            return refused(PushRefused::RateLimited { retry_s: 1 });
        }
    }
    let (head, _) = state.db.room_activity(&room_of(chat_id)).unwrap_or((0, 0));
    let sender_dto = crate::notify::Sender::user(state, user_id).dto();
    let from_handle = handle_of(state, user_id);
    let chat_ref = crate::notify::ChatRef::people(chat_id);
    for other in humans.iter().filter(|u| *u != user_id) {
        // Group name per recipient (an untitled group is the *other* members' handles).
        let meta = crate::notify::chat_meta(state, &chat_ref, other);
        let event = PushEventDto::PeopleMessage(PeopleMessagePayloadDto {
            chat_id: chat_id.to_owned(),
            from: format!("user:{user_id}"),
            from_handle: from_handle.clone(),
            seq: head + 1,
            sender: sender_dto.clone(),
            project_id: None,
            text: None,
            chat_kind: Some(meta.kind.to_owned()),
            chat_title: meta.title,
            chat_image_key: meta.image_key,
        });
        // A member who blocked the sender hears nothing of this row (hidden for them alone).
        if state.db.blocks(other, user_id).unwrap_or(false) {
            continue;
        }
        // Level `none` (or the old mute): no in-app banner either. Mentions cannot be judged
        // here (no text yet); the announcement path handles mentions-only chats.
        if crate::push::eligibility::silenced_by_level(state, other, &chat_ref) {
            continue;
        }
        // Archived for them: hidden on purpose, no notification (D78).
        if state
            .db
            .chat_archived_at(other, chat_id)
            .ok()
            .flatten()
            .is_none()
        {
            let _ = state.db.push_user_event(other, &event, Some(chat_id));
        }
        // A message un-hides a chat that was "deleted for me" on any of their devices.
        if let Ok(devices) = state.db.devices_of_user(other) {
            for d in devices {
                if let Some(id) = d["id"].as_str() {
                    let _ = state.db.set_hidden(chat_id, id, false);
                }
            }
        }
    }
    Ok(serving_agents(state, &cm))
}

/// Notify every human member except `skip`.
pub fn notify_members(state: &AppState, cm: &ChatWithMembers, skip: &str, event: &PushEventDto) {
    for uid in human_user_ids(cm) {
        if uid != skip {
            let _ = state.db.push_user_event(&uid, event, None);
        }
    }
}

/// After a project rename (#1481): every people chat that has this agent pushes
/// `people_chat_updated` so clients refresh `AgentMember.handle` from the live name
/// (handles are not stored on the membership row — they are derived at DTO time).
pub fn notify_agent_renamed(state: &AppState, project_id: &str) {
    let member = format!("agent:{project_id}");
    let Ok(chats) = state.db.chats_of_member(&member) else {
        return;
    };
    for cm in chats {
        let event = PushEventDto::PeopleChatUpdated(PeopleChatUpdatedPayloadDto {
            chat_id: cm.chat.id.clone(),
        });
        // Empty skip → every human member (including the renamer's other devices).
        notify_members(state, &cm, "", &event);
    }
}

/// Upper bound on the people told about one agent picture change (an agent in very large
/// groups would otherwise write thousands of push rows per change).
const AGENT_PROFILE_FAN_OUT_MAX: usize = 2000;

/// The agents' `profile_updated`: an avatar change (`PATCH /projects/{id}` with `image_key`,
/// a leaf or a colour pick) reaches every device that draws the agent — the owner's other
/// devices (`except_device` already has the answer), the project's guests and every human in
/// a people chat the agent sits on — as one `agent_profile_updated` push each, carrying the
/// picture now in effect and the stamp every copy must reach. Without it (Jacob, 2026-10-25)
/// a group's roster kept the old picture until some other event refreshed it, and the chat
/// flipped between the stale and the current avatar.
pub fn notify_agent_avatar_changed(
    state: &AppState,
    project: &crate::db::Project,
    except_device: &str,
) {
    let event = PushEventDto::AgentProfileUpdated(arbos_core::api::AgentProfileUpdatedPayloadDto {
        project_id: project.id.clone(),
        avatar_version: project.avatar_version,
        image_key: project.image_key.clone().filter(|k| !k.is_empty()),
    });
    if except_device.is_empty() {
        let _ = state.db.push_user_event(&project.user_id, &event, None);
    } else {
        let _ = state
            .db
            .push_user_except_event(&project.user_id, except_device, &event);
    }
    for uid in agent_profile_audience(state, project) {
        let _ = state.db.push_user_event(&uid, &event, None);
    }
}

/// Who else draws the agent: the project's guests and the humans in its people chats, each
/// once, never the owner, capped at `AGENT_PROFILE_FAN_OUT_MAX`.
pub fn agent_profile_audience(state: &AppState, project: &crate::db::Project) -> Vec<String> {
    let mut audience: Vec<String> = state
        .db
        .project_members(&project.id)
        .unwrap_or_default()
        .into_iter()
        .map(|(uid, _, _)| uid)
        .collect();
    for cm in state
        .db
        .chats_of_member(&format!("agent:{}", project.id))
        .unwrap_or_default()
    {
        audience.extend(human_user_ids(&cm));
    }
    audience.sort();
    audience.dedup();
    audience.retain(|u| u != &project.user_id);
    audience.truncate(AGENT_PROFILE_FAN_OUT_MAX);
    audience
}

#[cfg(test)]
mod push_rate_tests {
    //! Only accepted people pushes insert a rate event (#1570).

    use super::tests_support::{dm, person};
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[test]
    fn refused_push_adds_no_rate_event_and_push_after_window_is_accepted() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let limit = 3;
        for _ in 0..limit {
            assert!(state
                .db
                .rate_check_and_note(&alice, "msg", 60_000, limit)
                .unwrap());
        }
        assert_eq!(state.db.rate_count(&alice, "msg", 60_000).unwrap(), limit);

        // Directly at the gate: over the limit → RateLimited, count unchanged.
        // on_room_push uses MSGS_PER_MIN (or half); fill that window instead.
        let real_limit = MSGS_PER_MIN / 2; // young account
        while state.db.rate_count(&alice, "msg", 60_000).unwrap() < real_limit {
            assert!(state
                .db
                .rate_check_and_note(&alice, "msg", 60_000, real_limit)
                .unwrap());
        }
        let before = state.db.rate_count(&alice, "msg", 60_000).unwrap();
        let err = on_room_push(&state, &alice_dev, &chat).unwrap_err();
        assert!(matches!(err, PushRefused::RateLimited { retry_s } if retry_s >= 1));
        assert_eq!(
            state.db.rate_count(&alice, "msg", 60_000).unwrap(),
            before,
            "refused push must not insert"
        );

        // Age every event out of the window, then a push is accepted and counted once.
        state
            .db
            .with(|c| {
                c.execute("UPDATE rate_events SET at = at - 120000", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(state.db.rate_count(&alice, "msg", 60_000).unwrap(), 0);
        assert!(on_room_push(&state, &alice_dev, &chat).is_ok());
        assert_eq!(state.db.rate_count(&alice, "msg", 60_000).unwrap(), 1);
    }

    #[test]
    fn rate_check_and_note_counts_first() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        assert!(state
            .db
            .rate_check_and_note(&alice, "msg", 60_000, 2)
            .unwrap());
        assert!(state
            .db
            .rate_check_and_note(&alice, "msg", 60_000, 2)
            .unwrap());
        assert!(!state
            .db
            .rate_check_and_note(&alice, "msg", 60_000, 2)
            .unwrap());
        assert_eq!(state.db.rate_count(&alice, "msg", 60_000).unwrap(), 2);
    }
}

#[cfg(test)]
mod block_refusal_tests {
    //! Blocks and people rooms ([feedback blocked-state-relay-2026-10-26], Jacob 2026-10-26
    //! 17:28 CR): a DM between the two is refused with a named `reason`; a group is never
    //! refused — the blocked person's rows are hidden from the blocker alone (no push event,
    //! no alert, no unread for them; `blocked_senders` on their chat DTO) and the other
    //! members see and receive them as before.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use arbos_core::ids::ulid;

    use super::tests_support::{dm, group, person};
    use super::*;
    use crate::notify::{notify_people_chat, NotifyBody};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn pushes_of(state: &AppState, dev: &Principal) -> Vec<String> {
        state
            .db
            .take_user_pushes(&device_of(dev))
            .unwrap()
            .into_iter()
            .map(|(_, kind, _, _)| kind)
            .collect()
    }

    async fn say(state: &AppState, dev: &Principal, chat: &str, text: &str) {
        let r = notify_people_chat(
            State(state.clone()),
            dev.clone(),
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
        .await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_group_is_never_refused_and_the_blocked_rows_are_hidden_from_the_blocker_alone() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (carol, carol_dev) = person(&state, 3);
        let chat = group(&state, &[&alice, &bob, &carol]);
        // Bob blocks Alice. Alice still posts; Bob is told nothing, Carol everything.
        state.db.block(&bob, &alice).unwrap();
        assert!(
            on_room_push(&state, &alice_dev, &chat).is_ok(),
            "a group block never refuses the room"
        );
        let _ = pushes_of(&state, &bob_dev);
        let _ = pushes_of(&state, &carol_dev);
        assert!(on_room_push(&state, &alice_dev, &chat).is_ok());
        assert!(
            pushes_of(&state, &bob_dev)
                .iter()
                .all(|k| k != "people_message"),
            "the blocker gets no push event for the blocked sender's row"
        );
        assert!(
            pushes_of(&state, &carol_dev)
                .iter()
                .any(|k| k == "people_message"),
            "a third member gets it as before"
        );
        // Announce (the alert + unread path): Bob's unread stays 0, Carol's counts it.
        say(&state, &alice_dev, &chat, "hello all").await;
        assert_eq!(
            state.db.unread_in_chat(&bob, &chat).unwrap(),
            0,
            "no unread for the blocker"
        );
        assert_eq!(state.db.unread_in_chat(&carol, &chat).unwrap(), 1);
        assert_eq!(state.db.unread_total(&bob).unwrap(), 0);
        // The hint on Bob's DTO names Alice; Carol's and Alice's carry none.
        let cm = load_chat(&state, &chat).unwrap();
        let bobs = people_chat_dto(&state, &cm, &bob, &device_of(&bob_dev));
        assert_eq!(bobs.blocked_senders, vec![format!("user:{alice}")]);
        assert!(people_chat_dto(&state, &cm, &carol, &device_of(&carol_dev))
            .blocked_senders
            .is_empty());
        assert!(people_chat_dto(&state, &cm, &alice, &device_of(&alice_dev))
            .blocked_senders
            .is_empty());
        // Bob's own posts are not refused by his block, and Alice (who blocked nobody) still
        // receives them.
        let _ = pushes_of(&state, &alice_dev);
        assert!(on_room_push(&state, &bob_dev, &chat).is_ok());
        assert!(pushes_of(&state, &alice_dev)
            .iter()
            .any(|k| k == "people_message"));
        // Unblock reveals: the hint is gone and Alice's next row counts for Bob.
        state.db.unblock(&bob, &alice).unwrap();
        let cm = load_chat(&state, &chat).unwrap();
        assert!(people_chat_dto(&state, &cm, &bob, &device_of(&bob_dev))
            .blocked_senders
            .is_empty());
        say(&state, &alice_dev, &chat, "again").await;
        assert_eq!(
            state.db.unread_in_chat(&bob, &chat).unwrap(),
            2,
            "past rows count again too"
        );
    }

    #[test]
    fn in_a_dm_my_own_block_refuses_my_post_as_blocks_member() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        state.db.block(&alice, &bob).unwrap();
        let mine = on_room_push(&state, &alice_dev, &chat).unwrap_err();
        assert_eq!(mine.code(), "not_available");
        assert_eq!(
            (mine.reason(), mine.other_party()),
            (Some("blocks_member"), Some(("user2", None))),
            "Alice blocked Bob and writes to him: {mine:?}"
        );
        let theirs = on_room_push(&state, &bob_dev, &chat).unwrap_err();
        assert_eq!(
            (theirs.reason(), theirs.other_party()),
            (Some("blocked_by"), Some(("user1", None))),
            "Bob writes to Alice, who blocked him: {theirs:?}"
        );
        state.db.unblock(&alice, &bob).unwrap();
        assert!(on_room_push(&state, &alice_dev, &chat).is_ok());
        assert!(on_room_push(&state, &bob_dev, &chat).is_ok());
    }

    #[test]
    fn a_missing_chat_is_still_not_available() {
        let state = test_state(AuthMode::Github);
        let (_, alice_dev) = person(&state, 1);
        let err = on_room_push(&state, &alice_dev, "chat_missing").unwrap_err();
        assert_eq!(err.code(), "not_available");
        assert_eq!(err.reason(), Some("chat_unavailable"));
        assert!(err.other_party().is_none());
    }

    #[test]
    fn every_refusal_keeps_a_wire_code_old_clients_know() {
        let refused = PushRefused::Blocked {
            reason: BlockReason::BlockedBy,
            by_handle: "sami".into(),
            by_name: Some("Sami Kassab".into()),
        };
        assert_eq!(refused.code(), "not_available");
        assert_eq!(refused.reason(), Some("blocked_by"));
        assert_eq!(refused.other_party(), Some(("sami", Some("Sami Kassab"))));
        assert_eq!(refused.retry_s(), None);
        assert_eq!(BlockReason::BlocksMember.code(), "blocks_member");
        assert_eq!(PushRefused::Silenced.code(), "not_available");
        assert_eq!(PushRefused::Silenced.reason(), Some("silenced"));
        assert_eq!(PushRefused::RateLimited { retry_s: 3 }.reason(), None);
        for r in [
            PushRefused::RateLimited { retry_s: 1 },
            PushRefused::Silenced,
            PushRefused::NotAvailable,
            refused,
        ] {
            assert!(
                matches!(r.code(), "rate_limited" | "not_available"),
                "{r:?}: fielded clients only stay Live for these two"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    //! ACL invariant: only a member's devices (and the box of a project whose agent is in the
    //! chat) may touch a people room.

    use arbos_core::ids::ulid;

    use super::tests_support::{dm, person, project_of};
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// #1538: an agent named after a person ("unarbos" by @unarbos) gets a suffix so `@unarbos`
    /// keeps meaning the person; the boot sweep fixes existing ones; a free name is untouched.
    #[test]
    fn agent_names_stay_clear_of_people_handles() {
        let state = test_state(AuthMode::Paircode);
        let (jacob, _) = person(&state, 268993031); // login user268993031
        assert_eq!(
            agent_name_clear_of_people(&state, "User268993031"),
            ("User268993031 agent".to_owned(), true)
        );
        assert_eq!(
            agent_name_clear_of_people(&state, "Research"),
            ("Research".to_owned(), false)
        );
        // An existing collision is renamed at boot and the box is told.
        let pid = project_of(&state, &jacob, "user268993031");
        rename_agents_that_shadow_people(&state);
        let p = state.db.project(&pid).unwrap().unwrap();
        assert_eq!(p.name, "user268993031 agent");
        assert_ne!(agent_slug(&p.name), "user268993031");
        let pushes = state.db.take_pushes(&pid).unwrap_or_default();
        assert!(
            pushes.iter().any(|(_, kind, _)| kind == "project_renamed"),
            "{pushes:?}"
        );
        // Idempotent.
        rename_agents_that_shadow_people(&state);
        assert_eq!(
            state.db.project(&pid).unwrap().unwrap().name,
            "user268993031 agent"
        );
    }

    #[test]
    fn non_member_cannot_read_a_people_room() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (_carol, carol_dev) = person(&state, 3);
        let chat = dm(&state, &alice, &bob);
        let key = format!("{PEOPLE_ACL_PREFIX}{chat}");

        assert!(state.allows(&alice_dev, &key));
        assert!(state.allows(&bob_dev, &key));
        assert!(!state.allows(&carol_dev, &key), "a third person is refused");
        assert!(state.allows(&Principal::Admin, &key));
        assert!(
            !state.allows(&alice_dev, &format!("{PEOPLE_ACL_PREFIX}{}", ulid())),
            "unknown chat"
        );

        // A box is admitted only through one of its own projects being an agent in the chat.
        let carol_box = Principal::Box {
            user_id: "gh_3".into(),
        };
        let alice_box = Principal::Box {
            user_id: alice.clone(),
        };
        assert!(!state.allows(&alice_box, &key), "no agent in the chat yet");
        let project_id = project_of(&state, &alice, "Research");
        state
            .db
            .add_member(
                &chat,
                &format!("agent:{project_id}"),
                "agent",
                &alice,
                &json!({}),
            )
            .expect("agent member");
        assert!(
            state.allows(&alice_box, &key),
            "the owner's box joins for its agent"
        );
        assert!(
            !state.allows(&carol_box, &key),
            "another user's box is still refused"
        );

        // Leaving drops access at once.
        state
            .db
            .remove_member(&chat, &format!("user:{bob}"), false)
            .expect("leave");
        assert!(!state.allows(&bob_dev, &key));
    }

    /// #1580: the box roster lists each person's device ids (revoked ones too) so a row with
    /// only a `deviceId` is attributed to its author; the apps' roster does not carry them.
    #[test]
    fn box_roster_carries_member_devices_and_the_app_roster_does_not() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 11);
        let (bob, bob_dev) = person(&state, 12);
        let chat = dm(&state, &alice, &bob);
        let old = ulid();
        state
            .db
            .insert_device(&old, "old phone", &ulid(), &bob, "github")
            .expect("device");
        state.db.revoke_device_fully(&old).expect("revoke");
        let cm = load_chat(&state, &chat).expect("chat");

        let boxed = box_chat_json(&state, &cm, &alice, "box");
        let devices_of = |v: &Value, user: &str| -> Vec<String> {
            v["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["member"] == format!("user:{user}"))
                .and_then(|m| m["devices"].as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|d| d.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let Principal::Device { id: bob_id, .. } = &bob_dev else {
            panic!("device")
        };
        let Principal::Device { id: alice_id, .. } = &alice_dev else {
            panic!("device")
        };
        let bobs = devices_of(&boxed, &bob);
        assert!(bobs.contains(bob_id), "{bobs:?}");
        assert!(bobs.contains(&old), "revoked devices still name their rows");
        assert_eq!(devices_of(&boxed, &alice), vec![alice_id.clone()]);
        let agent_rows: Vec<&Value> = boxed["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["devices"].is_array())
            .collect();
        assert_eq!(agent_rows.len(), 2, "humans only");

        let app = chat_json(&state, &cm, &alice, alice_id);
        assert!(app["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m.get("devices").is_none()));
    }
}

#[cfg(test)]
mod agent_tests {
    //! Only your own Project's agent can be added; only an owner removes people; only the
    //! agent's owner removes the agent.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use arbos_core::ids::ulid;

    use super::tests_support::{dm, person, project_of};
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[tokio::test]
    async fn only_own_agent_can_be_added_and_only_its_owner_removes_it() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let alices_project = project_of(&state, &alice, "Research");

        // Bob tries to add Alice's agent: refused.
        let r = agent_add(
            State(state.clone()),
            bob_dev.clone(),
            Path(chat.clone()),
            Json(AddAgentBody {
                project_id: alices_project.clone(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert!(state.db.members_of(&chat).unwrap().len() == 2);

        // Alice adds her own: fine, the box is told.
        let r = agent_add(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            Json(AddAgentBody {
                project_id: alices_project.clone(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(state
            .db
            .membership(&chat, &format!("agent:{alices_project}"))
            .unwrap()
            .is_some());
        let pushes = state.db.take_pushes(&alices_project).unwrap();
        assert!(pushes.iter().any(|(_, k, _)| k == "people_agent_added"));

        // A second agent joining tells agents already in the chat to refresh their roster.
        let bobs_project = project_of(&state, &bob, "Bob");
        let r = agent_add(
            State(state.clone()),
            bob_dev.clone(),
            Path(chat.clone()),
            Json(AddAgentBody {
                project_id: bobs_project.clone(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let alice_pushes = state.db.take_pushes(&alices_project).unwrap();
        assert!(
            alice_pushes
                .iter()
                .any(|(_, k, _)| k == "people_agent_added"),
            "the agent already in the chat re-reads members when another joins"
        );
        let bob_pushes = state.db.take_pushes(&bobs_project).unwrap();
        assert!(bob_pushes.iter().any(|(_, k, _)| k == "people_agent_added"));

        // Bob cannot remove Alice's agent; Alice can.
        let r = agent_remove(
            State(state.clone()),
            bob_dev.clone(),
            Path((chat.clone(), alices_project.clone())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = agent_remove(
            State(state.clone()),
            alice_dev.clone(),
            Path((chat.clone(), alices_project.clone())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(state
            .db
            .membership(&chat, &format!("agent:{alices_project}"))
            .unwrap()
            .is_none());
        let pushes = state.db.take_pushes(&alices_project).unwrap();
        assert!(pushes.iter().any(|(_, k, _)| k == "people_agent_removed"));

        // A non-member cannot add anything at all.
        let (_carol, carol_dev) = person(&state, 3);
        let carols_project = project_of(&state, "gh_3", "Side");
        let r = agent_add(
            State(state.clone()),
            carol_dev,
            Path(chat.clone()),
            Json(AddAgentBody {
                project_id: carols_project,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn deleting_a_project_leaves_its_people_chats() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 21);
        let (bob, _bob_dev) = person(&state, 22);
        let chat = dm(&state, &alice, &bob);
        let project = project_of(&state, &alice, "Lium");
        state
            .db
            .add_member(
                &chat,
                &format!("agent:{project}"),
                "agent",
                &alice,
                &json!({}),
            )
            .expect("agent member");

        let cm = ChatWithMembers {
            chat: state.db.people_chat(&chat).unwrap().unwrap(),
            members: state.db.members_of(&chat).unwrap(),
        };
        let before = crate::people::chat_json(&state, &cm, &alice, "dev");
        assert!(
            before["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["member"] == format!("agent:{project}")),
            "live agent is listed"
        );

        let r = crate::api::delete_project(State(state.clone()), alice_dev, Path(project.clone()))
            .await
            .expect("delete");
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(
            state
                .db
                .membership(&chat, &format!("agent:{project}"))
                .unwrap()
                .is_none(),
            "soft-delete removes the agent from the people chat"
        );
        let pushes = state.db.take_pushes(&project).unwrap();
        assert!(
            pushes.iter().any(|(_, k, _)| k == "people_agent_removed"),
            "box is told to leave the room"
        );

        // A membership left behind by an older delete must not appear in chat_json either.
        state
            .db
            .add_member(
                &chat,
                &format!("agent:{project}"),
                "agent",
                &alice,
                &json!({}),
            )
            .expect("ghost row");
        let cm = ChatWithMembers {
            chat: state.db.people_chat(&chat).unwrap().unwrap(),
            members: state.db.members_of(&chat).unwrap(),
        };
        let after = crate::people::chat_json(&state, &cm, &alice, "dev");
        assert!(
            after["members"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["member"] != format!("agent:{project}")),
            "soft-deleted agent is hidden from the roster"
        );
        assert_eq!(after["lock_state"], "people_only");
    }

    #[tokio::test]
    async fn only_an_owner_removes_people() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 11);
        let (bob, bob_dev) = person(&state, 12);
        let (carol, _carol_dev) = person(&state, 13);
        let chat = dm(&state, &alice, &bob);
        // Carol joins as a plain member (a group-shaped chat).
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
        // Make Bob a plain member too, so only Alice owns the chat.
        state
            .db
            .add_member(&chat, &format!("user:{bob}"), "member", &alice, &json!({}))
            .unwrap();

        let no_ban = Query(std::collections::HashMap::new());
        let r = member_remove(
            State(state.clone()),
            bob_dev.clone(),
            Path((chat.clone(), carol.clone())),
            no_ban.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert!(state
            .db
            .membership(&chat, &format!("user:{carol}"))
            .unwrap()
            .is_some());
        // Removing yourself through the member menu is not a thing: Leave is (the
        // menu omits Remove on your own row).
        let r = member_remove(
            State(state.clone()),
            bob_dev,
            Path((chat.clone(), bob.clone())),
            no_ban.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        // A refused remove writes no membership notice.
        assert!(state
            .db
            .rows_after(&room_of(&chat), 0, None, 10)
            .unwrap()
            .is_empty());

        let r = member_remove(
            State(state.clone()),
            alice_dev,
            Path((chat.clone(), carol.clone())),
            no_ban,
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(state
            .db
            .membership(&chat, &format!("user:{carol}"))
            .unwrap()
            .is_none());
        assert!(!state.allows(
            &Principal::Device {
                id: ulid(),
                token: String::new(),
                user_id: carol.clone()
            },
            &format!("{PEOPLE_ACL_PREFIX}{chat}")
        ));
    }

    #[tokio::test]
    async fn owner_can_mute_and_ban_a_group_member() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 41);
        let (bob, bob_dev) = person(&state, 42);
        let chat = dm(&state, &alice, &bob);
        // Promote to a group so moderation applies the same as production groups.
        let _ = state.db.set_people_chat_kind(&chat, "group");
        state
            .db
            .add_member(&chat, &format!("user:{bob}"), "member", &alice, &json!({}))
            .unwrap();

        // Mute Bob from posting.
        let r = member_patch(
            State(state.clone()),
            alice_dev.clone(),
            Path((chat.clone(), bob.clone())),
            Json(MemberPatchBody {
                silenced: Some(true),
                silenced_until: None,
                role: None,
                can_mute_ban: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let m = state
            .db
            .membership(&chat, &format!("user:{bob}"))
            .unwrap()
            .unwrap();
        assert!(m.silenced);
        assert!(
            on_room_push(&state, &bob_dev, &chat).is_err(),
            "a muted member cannot post"
        );
        assert!(
            on_room_push(&state, &alice_dev, &chat).is_ok(),
            "owner still posts"
        );

        // Ban Bob — removed and cannot rejoin.
        let mut ban_q = std::collections::HashMap::new();
        ban_q.insert("ban".into(), "1".into());
        let r = member_remove(
            State(state.clone()),
            alice_dev.clone(),
            Path((chat.clone(), bob.clone())),
            Query(ban_q),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(state
            .db
            .membership(&chat, &format!("user:{bob}"))
            .unwrap()
            .is_none());
        assert!(state
            .db
            .member_is_banned(&chat, &format!("user:{bob}"))
            .unwrap());
        assert!(state
            .db
            .add_member(&chat, &format!("user:{bob}"), "member", &alice, &json!({}))
            .is_err());

        // Unban Bob — they can be added again.
        let r = member_unban(
            State(state.clone()),
            alice_dev.clone(),
            Path((chat.clone(), bob.clone())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(!state
            .db
            .member_is_banned(&chat, &format!("user:{bob}"))
            .unwrap());
        state
            .db
            .add_member(&chat, &format!("user:{bob}"), "member", &alice, &json!({}))
            .unwrap();
        assert!(state
            .db
            .membership(&chat, &format!("user:{bob}"))
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn create_group_adds_members_and_keeps_dm_separate() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 31);
        let (bob, _bob_dev) = person(&state, 32);
        let (carol, carol_dev) = person(&state, 33);
        let existing_dm = dm(&state, &alice, &bob);

        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec!["user32".into(), "@user33".into(), "user32".into()],
                agent_ids: vec![],
                title: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);

        let groups: Vec<_> = state
            .db
            .chats_of_member(&format!("user:{alice}"))
            .unwrap()
            .into_iter()
            .filter(|cm| cm.chat.kind == "group")
            .collect();
        assert_eq!(groups.len(), 1);
        let cm = &groups[0];
        assert_ne!(cm.chat.id, existing_dm, "group must not reuse the DM");
        assert_eq!(
            state.db.dm_between(&alice, &bob).unwrap().unwrap().id,
            existing_dm
        );
        assert_eq!(cm.chat.created_by, alice);
        let humans: Vec<_> = cm
            .members
            .iter()
            .filter_map(|m| m.user_id().map(str::to_owned))
            .collect();
        assert!(humans.contains(&alice) && humans.contains(&bob) && humans.contains(&carol));
        assert_eq!(humans.len(), 3, "duplicate handles are collapsed");
        assert!(cm
            .members
            .iter()
            .any(|m| m.user_id() == Some(alice.as_str()) && m.role == "owner"));
        assert!(cm
            .members
            .iter()
            .any(|m| m.user_id() == Some(bob.as_str()) && m.role == "member"));
        assert!(state.allows(&carol_dev, &format!("{PEOPLE_ACL_PREFIX}{}", cm.chat.id)));

        let r = chats_create_group(
            State(state.clone()),
            alice_dev,
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![],
                agent_ids: vec![],
                title: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }
}

#[cfg(test)]
mod membership_notice_tests {
    //! #1349: every membership change writes one system line into the group's chat — one per
    //! add batch, one per remove, "<who> left" on leave — as device `relay`, role `system`,
    //! so no client invents it and no agent answers it.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use super::tests_support::{person, project_of};
    use super::*;
    use crate::matrix::backfill::load_doc;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn system_lines(state: &AppState, chat: &str) -> Vec<String> {
        load_doc(state, chat, None)
            .expect("doc")
            .map(|d| {
                d.messages()
                    .into_iter()
                    .filter(|m| m.role == "system")
                    .map(|m| m.text())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn one_line_per_batch_add_remove_and_leave() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, bob_dev) = person(&state, 2);
        let (_carol, _) = person(&state, 3);
        let home = project_of(&state, &alice, "Home");
        let doug = project_of(&state, &alice, "Doug");

        // Alice creates a group with Bob.
        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec!["user2".into()],
                agent_ids: vec![],
                title: None,
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
        assert!(
            system_lines(&state, &chat).is_empty(),
            "creation writes no notice"
        );

        // One batch: Carol plus two agents → one line naming all three.
        let r = member_add(
            State(state.clone()),
            alice_dev.clone(),
            Path(chat.clone()),
            Json(AddMemberBody {
                handle: None,
                handles: vec!["user3".into()],
                agent_ids: vec![home.clone(), doug.clone()],
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let lines = system_lines(&state, &chat);
        assert_eq!(lines.len(), 1, "one notice for the whole batch: {lines:?}");
        assert_eq!(lines[0], "@user1 added @user3, Home and Doug");
        // The line is the relay's, not the adder's message.
        let doc = load_doc(&state, &chat, None).unwrap().unwrap();
        let m = doc
            .messages()
            .into_iter()
            .find(|m| m.role == "system")
            .unwrap();
        assert_eq!(m.rest["sender"], "system");
        assert_eq!(m.rest["deviceId"], crate::notices::RELAY_DEVICE);

        // Remove an agent, remove a person: one line each.
        let r = agent_remove(
            State(state.clone()),
            alice_dev.clone(),
            Path((chat.clone(), doug.clone())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        let carol_id = state.db.user_by_handle("user3").unwrap().unwrap().id;
        let r = member_remove(
            State(state.clone()),
            alice_dev.clone(),
            Path((chat.clone(), carol_id)),
            Query(std::collections::HashMap::new()),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);

        // Bob leaves.
        let r = chat_leave(State(state.clone()), bob_dev.clone(), Path(chat.clone())).await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        let _ = bob;

        let lines = system_lines(&state, &chat);
        assert_eq!(
            lines,
            [
                "@user1 added @user3, Home and Doug",
                "@user1 removed Doug",
                "@user1 removed @user3",
                "@user2 left",
            ]
        );
        // Every notice is its own row, signed by the relay.
        let rows = state.db.rows_after(&room_of(&chat), 0, None, 100).unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .all(|r| r.device == crate::notices::RELAY_DEVICE));
    }
}

#[cfg(test)]
mod read_cursor_tests {
    //! #1438: one read cursor per user. A chat is unread only for announced messages someone
    //! else wrote after the user's read mark — never for the user's own rows (from any of their
    //! devices), never for system lines or rows nobody announced; reading on one device clears
    //! it on every device, and writing in a chat counts as reading it.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use super::tests_support::{dm, person};
    use super::*;
    use crate::notify::{notify_people_chat, NotifyBody};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

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

    fn dto(state: &AppState, chat: &str, me: &str, dev: &Principal) -> PeopleChatDto {
        let Ok(cm) = load_chat(state, chat) else {
            panic!("chat {chat} not found");
        };
        people_chat_dto(state, &cm, me, &device_of(dev))
    }

    async fn say(state: &AppState, dev: &Principal, chat: &str, text: &str) {
        let r = notify_people_chat(
            State(state.clone()),
            dev.clone(),
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
        .await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    async fn read(state: &AppState, dev: &Principal, chat: &str, seq: i64) {
        let r = chat_read(
            State(state.clone()),
            dev.clone(),
            Path(chat.to_owned()),
            Json(ReadBody { seq }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn own_message_is_never_unread_and_reading_syncs_across_devices() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_mac) = person(&state, 1);
        let alice_phone = second_device(&state, &alice);
        let (bob, bob_dev) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);

        // Alice writes from her Mac: nothing is unread for her — on the Mac or the phone.
        say(&state, &alice_mac, &chat, "we dont have one yet").await;
        assert!(!dto(&state, &chat, &alice, &alice_mac).unread);
        assert!(!dto(&state, &chat, &alice, &alice_phone).unread);
        // Bob has genuine unseen content.
        assert!(dto(&state, &chat, &bob, &bob_dev).unread);
        assert_eq!(state.db.unread_in_chat(&bob, &chat).unwrap(), 1);

        // Bob reads on one device: his cursor is per user, so a second device of his is clear
        // too and sees the same cursor.
        let bob_phone = second_device(&state, &bob);
        read(&state, &bob_dev, &chat, 7).await;
        let on_phone = dto(&state, &chat, &bob, &bob_phone);
        assert!(!on_phone.unread);
        assert_eq!(on_phone.read_seq, 7);
        assert_eq!(state.db.unread_total(&bob).unwrap(), 0);

        // A system line nobody announced (a membership notice) moves the room head but is not
        // unread for anyone.
        let (seq, _) = state
            .db
            .append_row(&room_of(&chat), crate::notices::RELAY_DEVICE, &ulid(), b"x")
            .unwrap();
        let after_notice = dto(&state, &chat, &bob, &bob_dev);
        assert!(after_notice.head_seq >= seq);
        assert!(!after_notice.unread);
        assert!(!dto(&state, &chat, &alice, &alice_mac).unread);

        // Alice writes again after Bob read: genuine unread for Bob, on every device.
        say(&state, &alice_mac, &chat, "ok").await;
        assert!(dto(&state, &chat, &bob, &bob_dev).unread);
        assert!(dto(&state, &chat, &bob, &bob_phone).unread);

        // Bob answers without an explicit read: writing is reading. Alice now has Bob's line
        // unread until she reads or writes.
        say(&state, &bob_dev, &chat, "fine").await;
        assert!(!dto(&state, &chat, &bob, &bob_dev).unread);
        assert!(!dto(&state, &chat, &bob, &bob_phone).unread);
        assert!(dto(&state, &chat, &alice, &alice_phone).unread);
        say(&state, &alice_phone, &chat, "👍").await;
        assert!(!dto(&state, &chat, &alice, &alice_mac).unread);
        assert!(!dto(&state, &chat, &alice, &alice_phone).unread);
    }

    #[test]
    fn a_persons_own_row_advances_their_cursor_not_others() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        state.db.advance_user_cursor(&alice, &chat, 12).unwrap();
        assert_eq!(state.db.user_cursor(&alice, &chat).unwrap(), 12);
        assert_eq!(state.db.user_cursor(&bob, &chat).unwrap(), 0);
        // Never backwards.
        state.db.advance_user_cursor(&alice, &chat, 3).unwrap();
        assert_eq!(state.db.user_cursor(&alice, &chat).unwrap(), 12);
    }
}

#[cfg(test)]
mod create_group_tests {
    //! `POST /chats {kind: group}`: a group of my agents alone is a group; a group of nobody
    //! is not; a foreign agent fails before anything is written.

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::Json;

    use super::tests_support::{person, project_of};
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[tokio::test]
    async fn agents_alone_make_a_group_titled_after_them() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let bob_proj = project_of(&state, &alice, "Bob");
        let doug_proj = project_of(&state, &alice, "Doug");
        let before = state
            .db
            .chats_of_member(&format!("user:{alice}"))
            .unwrap()
            .len();

        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![],
                agent_ids: vec![bob_proj.clone(), doug_proj.clone()],
                title: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);

        let chats = state.db.chats_of_member(&format!("user:{alice}")).unwrap();
        assert_eq!(chats.len(), before + 1, "exactly one new chat");
        let cm = chats
            .iter()
            .find(|cm| cm.chat.kind == "group" && cm.members.len() == 3)
            .expect("a group with me and both agents");
        for pid in [&bob_proj, &doug_proj] {
            assert!(cm
                .members
                .iter()
                .any(|m| m.project_id() == Some(pid.as_str())));
            let pushes = state.db.take_pushes(pid).unwrap();
            assert!(pushes.iter().any(|(_, k, _)| k == "people_agent_added"));
        }
        let dto = people_chat_dto(&state, cm, &alice, "dev");
        let mut names: Vec<&str> = dto.title.split(", ").collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["bob", "doug"],
            "untitled agent groups take the agents' handles"
        );
    }

    #[tokio::test]
    async fn nobody_or_a_foreign_agent_creates_nothing() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let bobs_proj = project_of(&state, &bob, "Research");
        let before = state
            .db
            .chats_of_member(&format!("user:{alice}"))
            .unwrap()
            .len();

        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![],
                agent_ids: vec![],
                title: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);

        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![],
                agent_ids: vec![bobs_proj],
                title: None,
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let after = state
            .db
            .chats_of_member(&format!("user:{alice}"))
            .unwrap()
            .len();
        assert_eq!(after, before, "a refused create leaves no half-made chat");
    }
}

#[cfg(test)]
mod group_add_privacy_tests {
    //! A person's `group_add` privacy blocks others from putting them into a new group or
    //! adding them to an existing one (spam-group guard).

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use super::tests_support::person;
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[tokio::test]
    async fn nobody_refuses_create_and_add() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let bob_handle = state.db.user(&bob).unwrap().unwrap().login.unwrap();
        state.db.set_group_add_policy(&bob, "nobody").unwrap();

        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![bob_handle.clone()],
                agent_ids: vec![],
                title: Some("Spam".into()),
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // A group of Alice alone (plus an agent) so she can try Add members.
        let alice_proj = {
            use super::tests_support::project_of;
            project_of(&state, &alice, "Home")
        };
        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![],
                agent_ids: vec![alice_proj],
                title: Some("Alice's".into()),
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        let chat_id = state
            .db
            .chats_of_member(&format!("user:{alice}"))
            .unwrap()
            .into_iter()
            .find(|cm| cm.chat.kind == "group")
            .unwrap()
            .chat
            .id;

        let r = member_add(
            State(state.clone()),
            alice_dev,
            Path(chat_id),
            Json(AddMemberBody {
                handle: Some(bob_handle),
                handles: vec![],
                agent_ids: vec![],
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn people_i_know_allows_only_existing_dm() {
        let state = test_state(AuthMode::Github);
        let (_alice, alice_dev) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let (carol, _) = person(&state, 3);
        let bob_handle = state.db.user(&bob).unwrap().unwrap().login.unwrap();
        let carol_handle = state.db.user(&carol).unwrap().unwrap().login.unwrap();
        state
            .db
            .set_group_add_policy(&bob, "people_i_know")
            .unwrap();
        state
            .db
            .set_group_add_policy(&carol, "people_i_know")
            .unwrap();

        // Open a DM with Bob so Alice "knows" him; Carol stays a stranger.
        let r = dm_create(
            State(state.clone()),
            alice_dev.clone(),
            Json(DmBody {
                handle: bob_handle.clone(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);

        let r = chats_create_group(
            State(state.clone()),
            alice_dev.clone(),
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![bob_handle],
                agent_ids: vec![],
                title: Some("With Bob".into()),
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);

        let r = chats_create_group(
            State(state.clone()),
            alice_dev,
            Json(CreateGroupBody {
                kind: "group".into(),
                members: vec![carol_handle],
                agent_ids: vec![],
                title: Some("With Carol".into()),
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod shared_project_tests {
    //! Projects together: a guest the owner invited drives the project (member access), only the
    //! owner changes settings or members, and removal ends access at once.

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    use super::tests_support::{person, project_of};
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[tokio::test]
    async fn guests_drive_but_do_not_own() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 21);
        let (bob, bob_dev) = person(&state, 22);
        let (_carol, carol_dev) = person(&state, 23);
        let project = project_of(&state, &alice, "Shared");

        assert!(state.allows(&alice_dev, &project));
        assert!(!state.allows(&bob_dev, &project), "not invited yet");

        // Bob cannot invite himself; Alice can invite Bob.
        let r = project_member_add(
            State(state.clone()),
            bob_dev.clone(),
            Path(project.clone()),
            Json(ProjectMemberBody {
                handle: "user22".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = project_member_add(
            State(state.clone()),
            alice_dev.clone(),
            Path(project.clone()),
            Json(ProjectMemberBody {
                handle: "@user22".into(),
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);

        // Member access, owner-level refused.
        assert!(state.allows(&bob_dev, &project));
        assert!(!state.owns(&bob_dev, &project));
        assert!(state.owns(&alice_dev, &project));
        assert!(!state.allows(&carol_dev, &project));
        assert_eq!(
            state.db.shared_project_ids(&bob).unwrap(),
            vec![project.clone()]
        );

        // Only the owner removes a guest (a guest may leave).
        let r = project_member_remove(
            State(state.clone()),
            carol_dev.clone(),
            Path((project.clone(), bob.clone())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = project_member_remove(
            State(state.clone()),
            alice_dev.clone(),
            Path((project.clone(), bob.clone())),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(!state.allows(&bob_dev, &project), "access ends at once");
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use arbos_core::ids::{now_ms, ulid};
    use serde_json::json;

    use crate::auth::Principal;
    use crate::db::{Chat, Project};
    use crate::db_accounts::GithubIdentity;
    use crate::db_people::PeopleChat;
    use crate::state::AppState;

    pub fn person(state: &AppState, github_id: i64) -> (String, Principal) {
        let (user, _) = state
            .db
            .upsert_github_user(
                &GithubIdentity {
                    id: github_id,
                    login: format!("user{github_id}"),
                    avatar_url: None,
                    name: None,
                },
                20.0,
            )
            .expect("user");
        let device_id = ulid();
        state
            .db
            .insert_device(&device_id, "d", &ulid(), &user.id, "github")
            .expect("device");
        (
            user.id.clone(),
            Principal::Device {
                id: device_id,
                token: String::new(),
                user_id: user.id,
            },
        )
    }

    /// A people group ("Arbonites") of these humans; the first is the owner.
    pub fn group(state: &AppState, members: &[&str]) -> String {
        let chat = PeopleChat {
            id: ulid(),
            kind: "group".into(),
            created_by: members[0].to_owned(),
            created_at: now_ms(),
            title: Some("Arbonites".into()),
            image_key: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: None,
        };
        state.db.insert_people_chat(&chat).expect("chat");
        for u in members {
            state
                .db
                .add_member(
                    &chat.id,
                    &format!("user:{u}"),
                    "owner",
                    members[0],
                    &json!({}),
                )
                .expect("member");
        }
        chat.id
    }

    pub fn dm(state: &AppState, a: &str, b: &str) -> String {
        let chat = PeopleChat {
            id: ulid(),
            kind: "dm".into(),
            created_by: a.to_owned(),
            created_at: now_ms(),
            title: None,
            image_key: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: None,
        };
        state.db.insert_people_chat(&chat).expect("chat");
        for u in [a, b] {
            state
                .db
                .add_member(&chat.id, &format!("user:{u}"), "owner", a, &json!({}))
                .expect("member");
        }
        chat.id
    }

    pub fn project_of(state: &AppState, owner: &str, name: &str) -> String {
        let project = Project {
            id: ulid(),
            name: name.into(),
            icon: "R".into(),
            instructions: String::new(),
            models: json!({}),
            state: "hibernated_light".into(),
            created_at: now_ms(),
            last_active_at: now_ms(),
            spend_cap_daily: 25.0,
            color: "#000".into(),
            user_id: owner.to_owned(),
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
        state.db.insert_project(&project).expect("project");
        state
            .db
            .insert_chat(&Chat {
                id: ulid(),
                project_id: project.id.clone(),
                title: name.into(),
                is_pinned: true,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .expect("pinned chat");
        project.id
    }
}

#[cfg(test)]
mod mention_resolution_tests {
    //! What `@here` and `@agents` resolve to on the relay, which must be what the apps resolve
    //! them to on the row they write (`expand_collectives`, `arbos-mac/src/sync/mod.rs`):
    //! everyone in the chat and every agent, never the writer.

    use serde_json::json;

    use super::tests_support::{group, person, project_of};
    use super::*;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[test]
    fn here_and_agents_resolve_to_members_and_nobody_mentions_themselves() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8101);
        let (bob, _) = person(&state, 8102);
        let pid = project_of(&state, &alice, "Lium");
        let g = group(&state, &[&alice, &bob]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &alice, &json!({}))
            .unwrap();
        let Ok(cm) = load_chat(&state, &g) else {
            panic!("chat")
        };
        let me = format!("user:{alice}");

        let here = resolve_mentions(&state, &cm, "@here standup in five", &me);
        assert!(here.contains(&format!("user:{bob}")));
        assert!(here.contains(&format!("agent:{pid}")));
        assert!(!here.contains(&me), "the writer is not mentioned by @here");

        assert_eq!(
            resolve_mentions(&state, &cm, "@agents take this one", &me),
            vec![format!("agent:{pid}")],
            "@agents names the agents and no person"
        );

        // A handle on its own still resolves, and only once when the collective names it too.
        // Sorted, because what is being compared is who was named: a collective walks the
        // roster, and `members_of` orders that by `joined_at` with no tiebreak, so members who
        // joined in the same millisecond come back in whatever order SQLite chooses. Sorting
        // still catches a duplicate, which is what this assertion is for — the lists would
        // differ in length.
        let mut handle_and_collective = resolve_mentions(&state, &cm, "@lium and @here", &me);
        let mut collective_alone = resolve_mentions(&state, &cm, "@here", &me);
        handle_and_collective.sort();
        collective_alone.sort();
        assert_eq!(handle_and_collective, collective_alone);
    }

    /// A collective that names nobody adds nothing — the array is a list of members. The word is
    /// still in the prose, which is what `push::eligibility::mentioned` reads, so the people are
    /// told either way.
    #[test]
    fn a_collective_with_nobody_to_name_resolves_to_nothing() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 8111);
        let (bob, _) = person(&state, 8112);
        let g = group(&state, &[&alice, &bob]);
        let Ok(cm) = load_chat(&state, &g) else {
            panic!("chat")
        };
        let me = format!("user:{alice}");

        assert!(resolve_mentions(&state, &cm, "@agents anyone there?", &me).is_empty());
        assert_eq!(
            resolve_mentions(&state, &cm, "@here", &me),
            vec![format!("user:{bob}")]
        );
    }
}

#[cfg(test)]
mod settings_permission_tests {
    //! Who may change a people chat's name, picture, description and leaf (P0 2026-10-27):
    //! owner and admins yes; plain members, agents' boxes, non-members and the admin token
    //! no — 403 `admin_only`, the value untouched, one audit row per attempt.

    use axum::extract::{Path, State};
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
    use axum::Json;
    use bytes::Bytes;

    use super::tests_support::{dm, person, project_of};
    use super::*;
    use crate::blobs;
    use crate::image_sniff::samples;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// A group of the owner, one admin, one member and one agent; plus a stranger.
    struct Cast {
        chat: String,
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
        let chat = PeopleChat {
            id: ulid(),
            kind: "group".into(),
            created_by: owner.clone(),
            created_at: now_ms(),
            title: Some("Arbonites".into()),
            image_key: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: Some("the original".into()),
        };
        state.db.insert_people_chat(&chat).expect("chat");
        for (uid, role) in [(&owner, "owner"), (&admin, "admin"), (&member, "member")] {
            state
                .db
                .add_member(&chat.id, &format!("user:{uid}"), role, &owner, &json!({}))
                .expect("member");
        }
        let project = project_of(state, &member, "Scout");
        state
            .db
            .add_member(
                &chat.id,
                &format!("agent:{project}"),
                "agent",
                &owner,
                &json!({}),
            )
            .expect("agent member");
        Cast {
            chat: chat.id,
            owner: owner_dev,
            admin: admin_dev,
            member: member_dev,
            agent: Principal::Box { user_id: member },
            stranger: stranger_dev,
        }
    }

    fn body() -> PatchChatBody {
        PatchChatBody {
            title: None,
            image_key: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            description: None,
        }
    }

    async fn patch(state: &AppState, who: &Principal, chat: &str, b: PatchChatBody) -> StatusCode {
        chat_patch(
            State(state.clone()),
            who.clone(),
            Path(chat.to_owned()),
            Json(b),
        )
        .await
        .status()
    }

    async fn upload(state: &AppState, who: &Principal, chat: &str, key: &str) {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        let r = blobs::put(
            State(state.clone()),
            who.clone(),
            Path((format!("{PEOPLE_ACL_PREFIX}{chat}"), key.to_owned())),
            headers,
            Bytes::copy_from_slice(samples::PNG),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
    }

    fn chat_row(state: &AppState, chat: &str) -> PeopleChat {
        state.db.people_chat(chat).unwrap().unwrap()
    }

    fn audit_rows(state: &AppState, chat: &str) -> Vec<Value> {
        state
            .db
            .audit(&format!("{PEOPLE_ACL_PREFIX}{chat}"), 1000)
            .unwrap()
    }

    /// Every field × every principal: only the owner and the admin change anything.
    #[tokio::test]
    async fn group_settings_are_owner_and_admin_only_per_field() {
        let state = test_state(AuthMode::Github);
        let c = cast(&state, 7100);
        // Members may upload into the group's slot (attachments live there too); the
        // picture is the PATCH, and that is what is gated.
        upload(&state, &c.member, &c.chat, "member.png").await;
        upload(&state, &c.owner, &c.chat, "owner.png").await;

        type Make = Box<dyn Fn() -> PatchChatBody>;
        let fields: Vec<(&str, Make)> = vec![
            (
                "title",
                Box::new(|| PatchChatBody {
                    title: Some("Test".into()),
                    ..body()
                }),
            ),
            (
                "image_key",
                Box::new(|| PatchChatBody {
                    image_key: Some("member.png".into()),
                    ..body()
                }),
            ),
            (
                "description",
                Box::new(|| PatchChatBody {
                    description: Some("hijacked".into()),
                    ..body()
                }),
            ),
            (
                "avatar_leaf",
                Box::new(|| PatchChatBody {
                    avatar_leaf: Some(Some(1)),
                    avatar_leaf_color: Some(Some(2)),
                    ..body()
                }),
            ),
        ];
        let denied: [(&str, &Principal); 4] = [
            ("member", &c.member),
            ("agent", &c.agent),
            ("stranger", &c.stranger),
            ("admin-token", &Principal::Admin),
        ];
        let before = chat_row(&state, &c.chat);
        for (field, make) in &fields {
            for (who, principal) in denied {
                assert_eq!(
                    patch(&state, principal, &c.chat, make()).await,
                    StatusCode::FORBIDDEN,
                    "{who} changing {field}"
                );
            }
            let after = chat_row(&state, &c.chat);
            assert_eq!(after.title, before.title, "{field}: title untouched");
            assert_eq!(
                after.image_key, before.image_key,
                "{field}: picture untouched"
            );
            assert_eq!(
                after.description, before.description,
                "{field}: description untouched"
            );
            assert_eq!(
                after.avatar_leaf, before.avatar_leaf,
                "{field}: leaf untouched"
            );
        }
        // 4 fields × 4 denied principals, every one audited as refused.
        let rows = audit_rows(&state, &c.chat);
        assert_eq!(rows.len(), 16);
        assert!(rows.iter().all(|r| r["allowed"] == false));
        assert_eq!(
            rows.iter().filter(|r| r["reason"] == "admin_only").count(),
            4,
            "the plain member is the admin_only case"
        );
        assert!(rows
            .iter()
            .any(|r| r["principal"].as_str().unwrap().starts_with("box:")
                && r["reason"] == "devices_only"));
        assert!(rows
            .iter()
            .any(|r| r["reason"] == "not_member" && r["fields"][0] == "title"));

        // The admin and the owner succeed, field by field.
        assert_eq!(
            patch(
                &state,
                &c.admin,
                &c.chat,
                PatchChatBody {
                    title: Some("Arbonites Admin".into()),
                    ..body()
                }
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            patch(
                &state,
                &c.owner,
                &c.chat,
                PatchChatBody {
                    image_key: Some("owner.png".into()),
                    description: Some("by the owner".into()),
                    avatar_leaf: Some(Some(3)),
                    avatar_leaf_color: Some(Some(4)),
                    ..body()
                }
            )
            .await,
            StatusCode::OK
        );
        let now = chat_row(&state, &c.chat);
        assert_eq!(now.title.as_deref(), Some("Arbonites Admin"));
        assert_eq!(now.image_key.as_deref(), Some("owner.png"));
        assert_eq!(now.description.as_deref(), Some("by the owner"));
        assert_eq!((now.avatar_leaf, now.avatar_leaf_color), (Some(3), Some(4)));
        let rows = audit_rows(&state, &c.chat);
        assert_eq!(rows.len(), 18);
        let ok: Vec<&Value> = rows.iter().filter(|r| r["allowed"] == true).collect();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0]["role"], "admin");
        assert_eq!(ok[0]["fields"], json!(["title"]));
        assert_eq!(ok[1]["role"], "owner");
        assert_eq!(
            ok[1]["fields"],
            json!(["image_key", "avatar_leaf", "description"])
        );
        assert!(ok[1]["device_id"].is_string());
    }

    /// The 403 carries the one sentence the clients show.
    #[tokio::test]
    async fn the_refusal_is_one_plain_sentence() {
        let state = test_state(AuthMode::Github);
        let c = cast(&state, 7200);
        let r = chat_patch(
            State(state.clone()),
            c.member.clone(),
            Path(c.chat.clone()),
            Json(PatchChatBody {
                title: Some("Test".into()),
                ..body()
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["type"], "admin_only");
        assert_eq!(v["error"]["message"], SETTINGS_ADMIN_ONLY);
    }

    /// An empty body is a read: any member gets the chat back, nothing is audited; a
    /// stranger still gets 403.
    #[tokio::test]
    async fn an_empty_patch_is_a_read_for_members() {
        let state = test_state(AuthMode::Github);
        let c = cast(&state, 7300);
        assert_eq!(
            patch(&state, &c.member, &c.chat, body()).await,
            StatusCode::OK
        );
        assert_eq!(
            patch(&state, &c.stranger, &c.chat, body()).await,
            StatusCode::FORBIDDEN
        );
        assert!(audit_rows(&state, &c.chat).is_empty());
    }

    /// A member promoted to admin may edit; demoted back, may not.
    #[tokio::test]
    async fn promotion_opens_and_demotion_closes_the_settings() {
        let state = test_state(AuthMode::Github);
        let c = cast(&state, 7400);
        let member_id = match &c.member {
            Principal::Device { user_id, .. } => user_id.clone(),
            _ => unreachable!(),
        };
        let rename = || PatchChatBody {
            title: Some("Renamed".into()),
            ..body()
        };
        assert_eq!(
            patch(&state, &c.member, &c.chat, rename()).await,
            StatusCode::FORBIDDEN
        );
        state
            .db
            .set_member_role(&c.chat, &format!("user:{member_id}"), "admin")
            .unwrap();
        assert_eq!(
            patch(&state, &c.member, &c.chat, rename()).await,
            StatusCode::OK
        );
        state
            .db
            .set_member_role(&c.chat, &format!("user:{member_id}"), "member")
            .unwrap();
        assert_eq!(
            patch(&state, &c.member, &c.chat, rename()).await,
            StatusCode::FORBIDDEN
        );
    }

    /// A DM has two owners: both may set its fields; a third person and a box may not.
    #[tokio::test]
    async fn dm_fields_follow_the_same_rule() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 7500);
        let (bob, bob_dev) = person(&state, 7501);
        let (_, carol_dev) = person(&state, 7502);
        let chat = dm(&state, &alice, &bob);
        let describe = |t: &str| PatchChatBody {
            description: Some(t.into()),
            ..body()
        };
        assert_eq!(
            patch(&state, &alice_dev, &chat, describe("ours")).await,
            StatusCode::OK
        );
        assert_eq!(
            patch(&state, &bob_dev, &chat, describe("ours too")).await,
            StatusCode::OK
        );
        assert_eq!(
            patch(&state, &carol_dev, &chat, describe("mine")).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            patch(
                &state,
                &Principal::Box {
                    user_id: bob.clone()
                },
                &chat,
                describe("the box")
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            chat_row(&state, &chat).description.as_deref(),
            Some("ours too")
        );
        let rows = audit_rows(&state, &chat);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows.iter().filter(|r| r["allowed"] == true).count(), 2);
    }
}

#[cfg(test)]
mod picture_tests {
    //! A group picture is whatever was uploaded, judged on Content-Type and bytes: a model
    //! file, a renamed model file, or an upload without a type never becomes the picture.

    use axum::extract::{Path, State};
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
    use axum::Json;
    use bytes::Bytes;

    use super::tests_support::{dm, person};
    use super::*;
    use crate::blobs;
    use crate::image_sniff::samples;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn upload(
        state: &AppState,
        who: &Principal,
        chat: &str,
        key: &str,
        mime: Option<&str>,
        bytes: &[u8],
    ) {
        let mut headers = HeaderMap::new();
        if let Some(m) = mime {
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(m).unwrap());
        }
        let r = blobs::put(
            State(state.clone()),
            who.clone(),
            Path((format!("{PEOPLE_ACL_PREFIX}{chat}"), key.to_owned())),
            headers,
            Bytes::copy_from_slice(bytes),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT, "upload of {key}");
    }

    async fn set_picture(state: &AppState, who: &Principal, chat: &str, key: &str) -> StatusCode {
        chat_patch(
            State(state.clone()),
            who.clone(),
            Path(chat.to_owned()),
            Json(PatchChatBody {
                title: None,
                image_key: Some(key.to_owned()),
                avatar_leaf: None,
                avatar_leaf_color: None,
                description: None,
            }),
        )
        .await
        .status()
    }

    fn picture_of(state: &AppState, chat: &str) -> Option<String> {
        state.db.people_chat(chat).unwrap().unwrap().image_key
    }

    #[tokio::test]
    async fn group_picture_must_be_an_image_by_type_and_bytes() {
        let state = test_state(AuthMode::Github);
        let (alice, alice_dev) = person(&state, 31);
        let (bob, _) = person(&state, 32);
        let chat = dm(&state, &alice, &bob);

        // The report: a model file picked in the Mac panel.
        upload(
            &state,
            &alice_dev,
            &chat,
            "code_submission_v1.onnx",
            Some("application/octet-stream"),
            samples::ONNX,
        )
        .await;
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "code_submission_v1.onnx").await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(picture_of(&state, &chat), None);

        // The same bytes renamed and sent as image/png: the bytes decide.
        upload(
            &state,
            &alice_dev,
            &chat,
            "group-fake.png",
            Some("image/png"),
            samples::ONNX,
        )
        .await;
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "group-fake.png").await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );

        // Real image bytes sent without a Content-Type: the type is required too.
        upload(
            &state,
            &alice_dev,
            &chat,
            "group-untyped.jpg",
            None,
            samples::JPEG,
        )
        .await;
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "group-untyped.jpg").await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );

        // A key nothing was uploaded under.
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "group-missing.png").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(picture_of(&state, &chat), None);

        // A real picture, typed and shaped like one: accepted.
        upload(
            &state,
            &alice_dev,
            &chat,
            "group-real.png",
            Some("image/png"),
            samples::PNG,
        )
        .await;
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "group-real.png").await,
            StatusCode::OK
        );
        assert_eq!(picture_of(&state, &chat).as_deref(), Some("group-real.png"));

        // HEIC from an iPhone, and a WebP, pass the same gate.
        upload(
            &state,
            &alice_dev,
            &chat,
            "group-phone.heic",
            Some("image/heic"),
            samples::HEIC,
        )
        .await;
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "group-phone.heic").await,
            StatusCode::OK
        );
        upload(
            &state,
            &alice_dev,
            &chat,
            "group-web.webp",
            Some("image/webp"),
            samples::WEBP,
        )
        .await;
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "group-web.webp").await,
            StatusCode::OK
        );

        // Clearing still works and needs no blob.
        assert_eq!(
            set_picture(&state, &alice_dev, &chat, "").await,
            StatusCode::OK
        );
        assert_eq!(picture_of(&state, &chat), None);
    }
}
