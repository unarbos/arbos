//! Read-only debugging endpoints for an account's own devices (Jacob's Grok Bot on a full
//! device token, the apps, the admin) — the gaps left once chat export (`GET /chats/{id}/export`)
//! and agent inspection (`GET /projects/{id}/inspect`) exist:
//!
//! - `GET /me/devices` — every device of the account with kind, last seen, the app version it
//!   last reported, push / VoIP registration, and which one is calling.
//! - `GET /chats/{id}/members` — people + agents + roles of a chat the caller is in.
//! - `GET /chats/{id}/reads` — the read-cursor facts behind the unread dot (#1438).
//! - `GET /machine/debug` — the account's box as the relay sees it (record, state, agentd build,
//!   wake/turn/eviction bookkeeping, timings) plus the box's own `GET /box/state` (image,
//!   uptime, load, memory, disk, workspaces with their turn lock).
//! - `GET /me/feedback` — feedback items of the account (user, app diagnostics, agent-filed).
//! - `GET /me/relay-errors` — recent relay warnings that name the account's ids (`diag.rs`).
//!
//! Everything is scoped to the caller's user (admin: any user via `?user=`); nothing writes.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::redact::scrub_value;

use crate::auth::Principal;
use crate::boxes;
use crate::people;
use crate::signin::api_error;
use crate::state::AppState;

const BOX_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_WINDOW_MS: i64 = 60 * 60 * 1000;

/// The user a read-only debugging call is about: the caller's own; the admin may name one.
fn subject(principal: &Principal, q: &HashMap<String, String>) -> Result<String, Box<Response>> {
    match principal {
        Principal::Device { user_id, .. } | Principal::Box { user_id } => Ok(user_id.clone()),
        Principal::Admin => q
            .get("user")
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Box::new(api_error(
                    StatusCode::BAD_REQUEST,
                    "user",
                    "admin: name the account with ?user=<id>",
                ))
            }),
    }
}

fn since_of(q: &HashMap<String, String>) -> i64 {
    q.get("since")
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(now_ms() - DEFAULT_WINDOW_MS)
}

fn limit_of(q: &HashMap<String, String>, default: usize, max: usize) -> usize {
    q.get("limit")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(1, max)
}

/// `GET /me/devices`.
pub async fn me_devices(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match subject(&principal, &q) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let mut devices = match state.db.devices_of_user(&user_id) {
        Ok(d) => d,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    let versions = state
        .db
        .device_versions_of_user(&user_id)
        .unwrap_or_default();
    let (push, voip) = state
        .db
        .push_registrations_of_user(&user_id)
        .unwrap_or_default();
    let me = match &principal {
        Principal::Device { id, .. } => Some(id.as_str()),
        Principal::Box { .. } | Principal::Admin => None,
    };
    for d in &mut devices {
        let id = d["id"].as_str().unwrap_or("").to_owned();
        d["current"] = json!(me == Some(id.as_str()));
        d["app"] = versions
            .iter()
            .find(|v| v["device_id"].as_str() == Some(id.as_str()))
            .cloned()
            .map(|mut v| {
                if let Some(o) = v.as_object_mut() {
                    o.remove("device_id");
                }
                v
            })
            .unwrap_or(Value::Null);
        d["push_registered"] = json!(push.contains(&id));
        d["voip_registered"] = json!(voip.contains(&id));
    }
    let bx = state.db.user_box(&user_id).ok().flatten();
    Json(json!({
        "user_id": user_id,
        "devices": devices,
        "box_device_id": bx.map(|b| b.device_id),
    }))
    .into_response()
}

/// `GET /chats/{id}/members`: people + agents + roles of a chat the caller is in.
pub async fn chat_members(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    let (me, cm) = match people::member_chat(&state, &principal, &id) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let members: Vec<Value> = cm
        .members
        .iter()
        .map(|m| serde_json::to_value(people::member_dto(&state, m)).unwrap_or(Value::Null))
        .collect();
    let people_n = cm.members.iter().filter(|m| m.user_id().is_some()).count();
    let agents_n = cm
        .members
        .iter()
        .filter(|m| m.project_id().is_some())
        .count();
    Json(json!({
        "chat_id": cm.chat.id,
        "kind": cm.chat.kind,
        "created_by": cm.chat.created_by,
        "me": me,
        "people": people_n,
        "agents": agents_n,
        "members": members,
    }))
    .into_response()
}

/// `GET /chats/{id}/reads?events=<n>`: the read-cursor facts behind the unread dot (#1438) for
/// the caller in one chat — people chats by membership, project chats by project access.
pub async fn chat_reads(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    let room = if state.db.people_chat(&id).ok().flatten().is_some() {
        if !people::room_access(&state, &principal, &id) {
            return api_error(
                StatusCode::FORBIDDEN,
                "not_member",
                "you are not in this chat",
            );
        }
        people::room_of(&id)
    } else {
        match state.db.chat_project_id(&id).ok().flatten() {
            Some(project) if state.allows(&principal, &project) => {
                arbos_core::wire::room_name(&project, &id)
            }
            Some(_) => return api_error(StatusCode::FORBIDDEN, "forbidden", "not your chat"),
            None => return api_error(StatusCode::NOT_FOUND, "not_found", "no such chat"),
        }
    };
    let (head_seq, last_row_at) = state.db.room_activity(&room).unwrap_or((0, 0));
    let device_ids: Vec<String> = state
        .db
        .devices_of_user(user_id)
        .unwrap_or_default()
        .iter()
        .filter_map(|d| d["id"].as_str().map(str::to_owned))
        .collect();
    let reads = state
        .db
        .read_state(user_id, &id, &device_ids)
        .unwrap_or(Value::Null);
    let unread = state.db.unread_in_chat(user_id, &id).unwrap_or(0);
    let n = limit_of(&q, 20, 200) as i64;
    let events = state.db.chat_events_tail(&id, n).unwrap_or_default();
    Json(json!({
        "chat_id": id,
        "room": room,
        "user_id": user_id,
        "head_seq": head_seq,
        "last_row_at": last_row_at,
        "unread": unread,
        "reads": reads,
        "events": events,
        "rule": "unread = chat_events with id > reads.read_mark.event_id whose sender_user is not you; the apps' dot follows `unread`.",
    }))
    .into_response()
}

/// `GET /machine/debug`: the account's box, relay view + the box's own `GET /box/state`.
pub async fn machine_debug(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match subject(&principal, &q) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let (bs, since) = state.box_state(&user_id).await;
    let provider = boxes::provider_status(&state, &user_id).await;
    let agentd = state.db.box_agentd(&user_id).ok().flatten();
    let sb = state.db.user_box(&user_id).ok().flatten();
    let now = now_ms();
    let relay_view = json!({
        "state": format!("{bs:?}").to_lowercase(),
        "state_since": since,
        "awake_for_s": if since > 0 { Some((now - since).max(0) / 1000) } else { None },
        "provider": provider,
        "snapshot": sb.as_ref().and_then(|s| s.snapshot.clone()),
        "daytona_state": sb.as_ref().and_then(|s| s.daytona_state.clone()),
        "has_url": sb.as_ref().is_some_and(|s| s.box_url.is_some()),
        "agentd": agentd.map(|a| json!({ "sha": a.sha, "version": a.version, "image": a.image, "reported_at": a.reported_at })),
        "default_snapshot": state.default_snapshot(),
        "turn_running_since": state.turn_running.lock().await.get(&user_id).copied(),
        "evicted_at": state.evicted.lock().await.get(&user_id).copied(),
        "last_box_http_at": state.box_http_seen.lock().await.get(&user_id).copied(),
        "sockets": state.box_conns.lock().await.get(&user_id).map(|(n, last)| json!({ "open": n, "last_disconnect_at": last })),
        "wake_started_at": state.wake_started.lock().await.get(&user_id).copied(),
        "stale_bounces": state.stale_bounces.lock().await.get(&user_id).copied().unwrap_or(0),
        "upgrade": state.upgrading.lock().await.get(&user_id).map(|u| json!({
            "phase": u.phase, "since": u.since, "from_snapshot": u.from_snapshot, "to_snapshot": u.to_snapshot,
        })),
    });
    let mut timings = state
        .db
        .timings(&boxes::timing_key(&user_id))
        .unwrap_or_default();
    let keep = timings.len().saturating_sub(20);
    timings.drain(..keep);
    let on_box = match &sb {
        Some(sb) => match &sb.box_url {
            Some(url) => {
                let sent = boxes::send_to_box(&state, sb, || {
                    state
                        .http
                        .get(format!("{url}/box/state"))
                        .bearer_auth(&sb.mcp_token)
                        .timeout(BOX_TIMEOUT)
                })
                .await;
                match sent {
                    Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                        Ok(mut v) => {
                            scrub_value(&mut v);
                            json!({ "reachable": true, "state": v })
                        }
                        Err(e) => json!({ "reachable": false, "error": format!("bad json: {e}") }),
                    },
                    Ok(r) => {
                        json!({ "reachable": false, "error": format!("box answered {}", r.status()) })
                    }
                    Err(e) => {
                        json!({ "reachable": false, "error": format!("box unreachable: {e}") })
                    }
                }
            }
            None => {
                json!({ "reachable": false, "error": "box has no url yet (asleep or starting)" })
            }
        },
        None => json!({ "reachable": false, "error": "no box for this user" }),
    };
    // Room health (#1470): every people chat the account is in, divergent ones first.
    let mut rooms: Vec<Value> = Vec::new();
    for cm in state
        .db
        .chats_of_member(&format!("user:{user_id}"))
        .unwrap_or_default()
    {
        let room = people::room_of(&cm.chat.id);
        let mut h = crate::room_health::health_of(&state, &cm.chat.id, &room).await;
        if let Some(o) = h.as_object_mut() {
            o.remove("rule");
            o.remove("replicas");
            o.insert("title".into(), json!(cm.chat.title));
        }
        rooms.push(h);
    }
    rooms.sort_by_key(|r| !r["divergent"].as_bool().unwrap_or(false));
    Json(json!({
        "user_id": user_id,
        "relay": relay_view,
        "timings": timings,
        "box": on_box,
        "rooms": rooms,
        "rooms_divergent": rooms.iter().filter(|r| r["divergent"].as_bool().unwrap_or(false)).count(),
    }))
    .into_response()
}

/// `GET /me/feedback?since=&limit=&include=rollout`: the account's feedback items.
pub async fn me_feedback(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let user_id = match subject(&principal, &q) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let since = q
        .get("since")
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let limit = limit_of(&q, 50, 500);
    let with_rollout = q
        .get("include")
        .is_some_and(|i| i.split(',').any(|p| p.trim() == "rollout"));
    let items: Vec<Value> = crate::feedback::items_of_user(&state, &user_id, since, limit)
        .await
        .into_iter()
        .map(|mut item| {
            if !with_rollout {
                item.rollout = None;
            }
            let mut v = serde_json::to_value(&item).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("has_image".into(), json!(!item.file.is_empty()));
                o.remove("file");
            }
            scrub_value(&mut v);
            v
        })
        .collect();
    Json(json!({ "user_id": user_id, "count": items.len(), "items": items })).into_response()
}

/// `GET /me/relay-errors?since=&limit=`: recent relay warnings that name the account's ids.
pub async fn me_relay_errors(
    State(state): State<AppState>,
    principal: Principal,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let since = since_of(&q);
    let limit = limit_of(&q, 200, 1000);
    if matches!(principal, Principal::Admin) && !q.contains_key("user") {
        let lines = crate::diag::recent(since, &[], true, limit);
        return Json(
            json!({ "scope": "all", "since": since, "count": lines.len(), "lines": lines }),
        )
        .into_response();
    }
    let user_id = match subject(&principal, &q) {
        Ok(u) => u,
        Err(r) => return *r,
    };
    let mut ids: Vec<String> = vec![user_id.clone()];
    ids.extend(
        state
            .db
            .devices_of_user(&user_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|d| d["id"].as_str().map(str::to_owned)),
    );
    ids.extend(
        state
            .db
            .projects_of_user(&user_id)
            .unwrap_or_default()
            .iter()
            .map(|p| p.id.clone()),
    );
    if let Ok(Some(bx)) = state.db.user_box(&user_id) {
        ids.push(bx.device_id.clone());
        if let Some(s) = bx.sandbox_id {
            ids.push(s);
        }
    }
    ids.extend(
        state
            .db
            .chats_of_member(&format!("user:{user_id}"))
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.chat.id),
    );
    let lines = crate::diag::recent(since, &ids, false, limit);
    Json(json!({ "scope": "user", "user_id": user_id, "since": since, "count": lines.len(), "lines": lines }))
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    async fn body(r: Response) -> (StatusCode, Value) {
        let status = r.status();
        let bytes = to_bytes(r.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn devices_list_their_kind_last_seen_and_the_caller() {
        let state = test_state(AuthMode::Github);
        let (user, dev) = person(&state, 1);
        let (status, v) =
            body(me_devices(State(state.clone()), dev.clone(), Query(HashMap::new())).await).await;
        assert_eq!(status, StatusCode::OK);
        let devices = v["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["kind"], "github");
        assert_eq!(devices[0]["current"], true);
        assert!(devices[0].get("last_seen_at").is_some());
        assert_eq!(devices[0]["push_registered"], false);
        assert_eq!(v["user_id"], user);
        // Another user sees only their own.
        let (_, other) = person(&state, 2);
        let (_, v) =
            body(me_devices(State(state.clone()), other, Query(HashMap::new())).await).await;
        assert_ne!(v["user_id"], user);
    }

    #[tokio::test]
    async fn members_and_reads_need_membership() {
        let state = test_state(AuthMode::Github);
        let (a, dev_a) = person(&state, 1);
        let (b, _dev_b) = person(&state, 2);
        let (_, stranger) = person(&state, 3);
        let chat = dm(&state, &a, &b);
        let (status, v) =
            body(chat_members(State(state.clone()), dev_a.clone(), Path(chat.clone())).await).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["people"], 2);
        assert_eq!(v["agents"], 0);
        assert_eq!(v["members"].as_array().unwrap().len(), 2);
        let (status, _) =
            body(chat_members(State(state.clone()), stranger.clone(), Path(chat.clone())).await)
                .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, v) = body(
            chat_reads(
                State(state.clone()),
                dev_a.clone(),
                Path(chat.clone()),
                Query(HashMap::new()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["unread"], 0);
        assert!(v["reads"]["user_cursor"].is_null());
        let (status, _) = body(
            chat_reads(
                State(state.clone()),
                stranger,
                Path(chat),
                Query(HashMap::new()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn machine_debug_without_a_box_says_so() {
        let state = test_state(AuthMode::Github);
        let (user, dev) = person(&state, 1);
        let _p = project_of(&state, &user, "Agent");
        let (status, v) =
            body(machine_debug(State(state.clone()), dev, Query(HashMap::new())).await).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["relay"]["state"], "asleep");
        assert_eq!(v["box"]["reachable"], false);
        // Admin must name a user.
        let (status, _) = body(
            machine_debug(
                State(state.clone()),
                Principal::Admin,
                Query(HashMap::new()),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn feedback_and_relay_errors_are_scoped() {
        let state = test_state(AuthMode::Github);
        let (_, dev) = person(&state, 1);
        let (status, v) =
            body(me_feedback(State(state.clone()), dev.clone(), Query(HashMap::new())).await).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["count"], 0);
        let (status, v) =
            body(me_relay_errors(State(state.clone()), dev, Query(HashMap::new())).await).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["scope"], "user");
    }
}
