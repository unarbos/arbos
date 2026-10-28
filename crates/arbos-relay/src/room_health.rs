//! Room health (#1470 slice 3, from #1518): is a chat's history on the relay whole, and do the
//! replicas agree with it? Arbonites showed a room can be *silently* short — a stale checkpoint
//! deleted rows, later rows were stored but parked by Loro as pending, `export` answered a short
//! list with no warning and the agents never saw their mentions. One call tells the truth:
//!
//! `GET /chats/{id}/health` (members of the chat, or the admin) →
//! head/floor, the checkpoint, stored vs pending rows, projected vs announced vs mirrored message
//! counts, what each replica last told the relay (HELLO cursor, catch-up, history heals), and a
//! `divergent` flag with short reasons. The rebuilt doc is cached per room head.
//!
//! Respond-gate decisions (2026-10-27): every agent's box posts its verdict on every human
//! message of a people chat (`POST /box/people/{chat}/gate`: reply or silent, the judge's
//! one-line reason, confidence, who it was for, model, latency, cost). `GET /chats/{id}/health`
//! carries the newest ones in a `gate` block; `GET /admin/room-health/{chat}?n=` is the
//! operator's view of the same with a longer window — "why did Affine answer that" is read
//! here, not from four boxes' logs.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use arbos_core::doc::ChatDoc;
use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::db_people::GateDecision;
use crate::people;
use crate::signin::api_error;
use crate::state::AppState;

/// Gate decisions shown in `/chats/{id}/health` (members) and by default in the admin view.
const GATE_DEFAULT: i64 = 40;
const GATE_MAX: i64 = 500;

/// What one replica (device or box) last told the relay about a room.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReplicaInfo {
    /// Cursor the replica announced in HELLO (what it had before catch-up).
    pub hello_cursor: i64,
    /// Row it was walked to by its last ROWS_DONE.
    pub caught_up_to: i64,
    pub last_seen_at: i64,
    /// History heals pushed (#1540): count and bytes — ops the replica held and the relay lacked.
    pub heals: u32,
    pub heal_bytes: u64,
    pub last_heal_at: i64,
}

/// Replica facts per room, in memory (reset by a relay restart; the next HELLO refills them).
pub type Replicas = std::sync::Mutex<HashMap<String, ReplicaInfo>>;

pub fn note_hello(room: &crate::state::Room, device: &str, cursor: i64) {
    let mut r = room.replicas.lock().unwrap();
    let e = r.entry(device.to_owned()).or_default();
    e.hello_cursor = cursor;
    e.last_seen_at = now_ms();
}

pub fn note_caught_up(room: &crate::state::Room, device: &str, seq: i64) {
    let mut r = room.replicas.lock().unwrap();
    let e = r.entry(device.to_owned()).or_default();
    e.caught_up_to = e.caught_up_to.max(seq);
    e.last_seen_at = now_ms();
}

pub fn note_heal(room: &crate::state::Room, device: &str, bytes: usize) {
    let mut r = room.replicas.lock().unwrap();
    let e = r.entry(device.to_owned()).or_default();
    e.heals += 1;
    e.heal_bytes += bytes as u64;
    e.last_heal_at = now_ms();
    e.last_seen_at = e.last_heal_at;
}

/// The rebuilt doc's verdict on the stored rows.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DocHealth {
    pub head: i64,
    pub floor: i64,
    pub checkpoint_seq: i64,
    pub checkpoint_at: i64,
    pub checkpoint_size: i64,
    pub rows_stored: i64,
    /// Rows Loro parked (their deps are not in the checkpoint or the rows before them).
    pub rows_pending: i64,
    pub first_pending_seq: Option<i64>,
    /// Rows that failed to decode at all.
    pub rows_bad: i64,
    /// Messages in the rebuilt doc (what `export` and a fresh device see).
    pub projected: i64,
}

/// Rebuild checkpoint + rows and count what applies. Cached on the room by head.
pub fn doc_health(
    state: &AppState,
    room: &crate::state::Room,
    room_name: &str,
) -> anyhow::Result<DocHealth> {
    let st = state.db.room_state(room_name)?;
    // Keyed on (head, floor): a checkpoint changes the stored doc without moving the head.
    if let Some((key, cached)) = room.health_cache.lock().unwrap().as_ref() {
        if *key == (st.head, st.floor) {
            return Ok(cached.clone());
        }
    }
    let (checkpoint_at, mut doc) = match state.db.checkpoint_full(room_name)? {
        Some((_, at, bytes)) => (at, ChatDoc::from_snapshot(None, false, &bytes)?),
        None => (0, ChatDoc::new(None, false)),
    };
    let mut after = 0;
    let mut pending = 0;
    let mut bad = 0;
    let mut first_pending = None;
    let mut stored = 0;
    loop {
        let rows = state.db.rows_after(room_name, after, None, 256)?;
        let n = rows.len();
        for r in rows {
            after = r.seq;
            stored += 1;
            match doc.import(&r.bytes) {
                Ok(status) if status.pending.is_some() => {
                    pending += 1;
                    first_pending.get_or_insert(r.seq);
                }
                Ok(_) => {}
                Err(_) => bad += 1,
            }
        }
        if n < 256 {
            break;
        }
    }
    let h = DocHealth {
        head: st.head,
        floor: st.floor,
        checkpoint_seq: st.checkpoint_seq,
        checkpoint_at,
        checkpoint_size: st.checkpoint_size,
        rows_stored: stored,
        rows_pending: pending,
        first_pending_seq: first_pending,
        rows_bad: bad,
        projected: doc.messages_list().len() as i64,
    };
    *room.health_cache.lock().unwrap() = Some(((st.head, st.floor), h.clone()));
    Ok(h)
}

/// Lines the relay knows about that are not user/agent messages (join notices, system lines):
/// `chat_events` counts only announced messages, the mirror carries system lines too, so the
/// three counters agree only within this margin.
const SYSTEM_LINE_MARGIN: i64 = 25;

/// The full health block for one chat.
pub async fn health_of(state: &AppState, chat_id: &str, room_name: &str) -> Value {
    let room = state.room(room_name).await;
    let doc = match doc_health(state, &room, room_name) {
        Ok(d) => d,
        Err(e) => return json!({ "chat_id": chat_id, "error": e.to_string() }),
    };
    let announced = state.db.chat_events_count(chat_id).unwrap_or(0);
    let mirror = state.db.matrix_events_count(chat_id).unwrap_or(0);
    let replicas: Vec<Value> = {
        let r = room.replicas.lock().unwrap();
        let mut v: Vec<(String, ReplicaInfo)> =
            r.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        v.sort_by_key(|(_, i)| std::cmp::Reverse(i.last_seen_at));
        v.into_iter()
            .map(|(device, i)| {
                json!({
                    "device": device,
                    "hello_cursor": i.hello_cursor,
                    "caught_up_to": i.caught_up_to,
                    "rows_behind": (doc.head - i.caught_up_to.max(i.hello_cursor)).max(0),
                    "last_seen_at": i.last_seen_at,
                    "heals": i.heals,
                    "ops_beyond_relay_bytes": i.heal_bytes,
                    "last_heal_at": if i.last_heal_at > 0 { Some(i.last_heal_at) } else { None },
                })
            })
            .collect()
    };
    let mut reasons: Vec<String> = Vec::new();
    if doc.rows_pending > 0 {
        reasons.push(format!(
            "{} of {} stored rows do not apply to the rebuilt doc (first seq {}): their ops' dependencies are not on the relay",
            doc.rows_pending,
            doc.rows_stored,
            doc.first_pending_seq.unwrap_or_default()
        ));
    }
    if doc.rows_bad > 0 {
        reasons.push(format!("{} stored rows do not decode", doc.rows_bad));
    }
    if doc.projected + SYSTEM_LINE_MARGIN < announced {
        reasons.push(format!(
            "announced messages ({announced}) exceed the rebuilt doc ({}): messages were lost from the relay's history",
            doc.projected
        ));
    }
    if doc.projected + SYSTEM_LINE_MARGIN < mirror {
        reasons.push(format!(
            "the Matrix mirror ({mirror}) has more messages than the rebuilt doc ({})",
            doc.projected
        ));
    }
    let healed_recently = replicas
        .iter()
        .any(|r| r["heals"].as_u64().unwrap_or(0) > 0);
    if healed_recently && doc.rows_pending == 0 {
        reasons.push("a replica pushed history the relay lacked; the doc is whole now".into());
    }
    let divergent = doc.rows_pending > 0
        || doc.rows_bad > 0
        || doc.projected + SYSTEM_LINE_MARGIN < announced
        || doc.projected + SYSTEM_LINE_MARGIN < mirror;
    json!({
        "chat_id": chat_id,
        "room": room_name,
        "gate": gate_block(state, chat_id, GATE_DEFAULT),
        "head_seq": doc.head,
        "floor": doc.floor,
        "checkpoint": if doc.checkpoint_seq > 0 {
            json!({ "seq": doc.checkpoint_seq, "at": doc.checkpoint_at, "size": doc.checkpoint_size })
        } else {
            Value::Null
        },
        "rows": { "stored": doc.rows_stored, "pending": doc.rows_pending, "first_pending_seq": doc.first_pending_seq, "bad": doc.rows_bad },
        "messages": { "projected": doc.projected, "announced": announced, "mirror": mirror, "margin": SYSTEM_LINE_MARGIN },
        "replicas": replicas,
        "divergent": divergent,
        "reasons": reasons,
        "checked_at": now_ms(),
        "rule": "divergent = stored rows that do not apply, or fewer projected messages than announced/mirrored (beyond the system-line margin). `export` is the projected doc; pending rows exist on the relay but no fresh device can show them until a replica that holds their dependencies reconnects (history heal, #1540).",
    })
}

/// The newest gate decisions of a chat, grouped per message (newest message first): one
/// entry per message with every agent's verdict and reason.
pub fn gate_block(state: &AppState, chat_id: &str, limit: i64) -> Value {
    let rows = state
        .db
        .gate_decisions(chat_id, limit.clamp(1, GATE_MAX))
        .unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    let mut by_msg: HashMap<String, Vec<&GateDecision>> = HashMap::new();
    for d in &rows {
        if !by_msg.contains_key(&d.message_id) {
            order.push(d.message_id.clone());
        }
        by_msg.entry(d.message_id.clone()).or_default().push(d);
    }
    let messages: Vec<Value> = order
        .iter()
        .map(|mid| {
            let ds = &by_msg[mid];
            let at = ds.iter().map(|d| d.at).max().unwrap_or(0);
            let repliers: Vec<&str> = ds
                .iter()
                .filter(|d| d.decision == "reply")
                .map(|d| d.handle.as_str())
                .collect();
            json!({
                "message_id": mid,
                "at": at,
                "repliers": repliers,
                "agents": ds.iter().map(|d| json!({
                    "agent_id": d.agent_id,
                    "handle": d.handle,
                    "decision": d.decision,
                    "reason": d.reason,
                    "confidence": d.confidence,
                    "addressee": d.addressee,
                    "chosen": d.chosen,
                    "model": d.model,
                    "latency_ms": d.latency_ms,
                    "cost_usd": d.cost_usd,
                    "at": d.at,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "decisions": rows.len(),
        "messages": messages,
        "rule": "one judge call per human message per agent box (agentd responders.rs); `decision` is that agent's verdict, `reason` the judge's line, `chosen` every agent the judge picked; `repliers` are the agents that answered the message.",
    })
}

/// The body a box posts for one verdict.
#[derive(Debug, serde::Deserialize)]
pub struct GateReport {
    pub message_id: String,
    pub agent_id: String,
    #[serde(default)]
    pub handle: String,
    pub decision: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub chosen: Vec<String>,
    pub confidence: Option<f64>,
    pub addressee: Option<String>,
    pub model: Option<String>,
    pub latency_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub at: Option<i64>,
}

/// `POST /box/people/{chat}/gate` — a box records its agent's verdict on one message. The
/// agent must be one of the box owner's projects and a member of the chat.
pub async fn post_gate_decision(
    State(state): State<AppState>,
    principal: Principal,
    Path(chat_id): Path<String>,
    Json(body): Json<GateReport>,
) -> Response {
    let Principal::Box { user_id } = &principal else {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "box only");
    };
    if !people::room_access(&state, &principal, &chat_id) {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_member",
            "no agent of yours is in this chat",
        );
    }
    let owns = state
        .db
        .project(&body.agent_id)
        .ok()
        .flatten()
        .is_some_and(|p| p.user_id == *user_id);
    let member = state
        .db
        .membership(&chat_id, &format!("agent:{}", body.agent_id))
        .ok()
        .flatten()
        .is_some();
    if !owns || !member {
        return api_error(
            StatusCode::FORBIDDEN,
            "not_your_agent",
            "that agent is not yours or not in this chat",
        );
    }
    if !matches!(body.decision.as_str(), "reply" | "silent") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_decision",
            "decision must be reply or silent",
        );
    }
    let clip = |s: &str, n: usize| -> String { s.chars().take(n).collect() };
    let d = GateDecision {
        chat_id: chat_id.clone(),
        message_id: clip(&body.message_id, 64),
        agent_id: body.agent_id.clone(),
        handle: clip(&body.handle, 64),
        decision: body.decision.clone(),
        reason: clip(&body.reason, 600),
        chosen: body.chosen.iter().take(32).map(|c| clip(c, 64)).collect(),
        confidence: body.confidence,
        addressee: body.addressee.as_deref().map(|a| clip(a, 80)),
        model: body.model.as_deref().map(|m| clip(m, 80)),
        latency_ms: body.latency_ms,
        cost_usd: body.cost_usd,
        at: body.at.unwrap_or_else(now_ms),
    };
    match state.db.put_gate_decision(&d) {
        Ok(()) => (StatusCode::NO_CONTENT, ()).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "store", e.to_string()),
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct RoomHealthQuery {
    /// How many gate decisions to include (default 40, max 500).
    pub n: Option<i64>,
}

/// `GET /admin/room-health/{chat}` — the health block plus up to `n` gate decisions.
pub async fn admin_room_health(
    State(state): State<AppState>,
    principal: Principal,
    Path(chat_id): Path<String>,
    Query(q): Query<RoomHealthQuery>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    if state.db.people_chat(&chat_id).ok().flatten().is_none() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no such people chat");
    }
    let room = people::room_of(&chat_id);
    let mut v = health_of(&state, &chat_id, &room).await;
    v["gate"] = gate_block(&state, &chat_id, q.n.unwrap_or(GATE_DEFAULT));
    Json(v).into_response()
}

/// `GET /chats/{id}/health` — people chats by membership, project chats by project access.
pub async fn chat_health(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
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
    Json(health_of(&state, &id, &room).await).into_response()
}

#[cfg(test)]
mod tests {
    //! Health says the truth about a damaged room and about a whole one.

    use arbos_core::doc::{ChatDoc, UserMessageOpts};
    use arbos_core::ids::{now_ms, ulid};

    use crate::db::Chat;
    use crate::people::tests_support::{person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    /// A box posts its agent's verdict; the admin view and the members' health both show it,
    /// grouped per message with the reason; a box that does not own the agent is refused.
    #[tokio::test]
    async fn gate_decisions_are_stored_and_shown_per_message() {
        use crate::people::tests_support::group;
        use axum::extract::{Path, Query, State};
        let state = test_state(AuthMode::Github);
        let (jacob, _) = person(&state, 1);
        let (ken, _) = person(&state, 2);
        let chat = group(&state, &[&jacob, &ken]);
        let affine = project_of(&state, &jacob, "Affine");
        let jo = project_of(&state, &ken, "Jo");
        for (p, owner) in [(&affine, &jacob), (&jo, &ken)] {
            state
                .db
                .add_member(
                    &chat,
                    &format!("agent:{p}"),
                    "agent",
                    owner,
                    &serde_json::json!({}),
                )
                .unwrap();
        }
        let report = |agent: &str, handle: &str, decision: &str, reason: &str, msg: &str| {
            super::GateReport {
                message_id: msg.into(),
                agent_id: agent.into(),
                handle: handle.into(),
                decision: decision.into(),
                reason: reason.into(),
                chosen: vec![jo.clone()],
                confidence: Some(0.93),
                addressee: Some("@jo".into()),
                model: Some("typesafe/jev-1.14-test".into()),
                latency_ms: Some(120),
                cost_usd: Some(0.00014),
                at: None,
            }
        };
        let jacobs_box = crate::auth::Principal::Box {
            user_id: jacob.clone(),
        };
        let kens_box = crate::auth::Principal::Box {
            user_id: ken.clone(),
        };
        let r = super::post_gate_decision(
            State(state.clone()),
            jacobs_box.clone(),
            Path(chat.clone()),
            axum::Json(report(
                &affine,
                "affine",
                "silent",
                "judge: for=@jo (0.93); not me → silent",
                "m1",
            )),
        )
        .await;
        assert_eq!(r.status(), axum::http::StatusCode::NO_CONTENT);
        let r = super::post_gate_decision(
            State(state.clone()),
            kens_box,
            Path(chat.clone()),
            axum::Json(report(
                &jo,
                "jo",
                "reply",
                "judge: for=@jo (0.93) ≥ 0.60 → reply",
                "m1",
            )),
        )
        .await;
        assert_eq!(r.status(), axum::http::StatusCode::NO_CONTENT);
        // Jacob's box may not speak for Ken's agent.
        let r = super::post_gate_decision(
            State(state.clone()),
            jacobs_box,
            Path(chat.clone()),
            axum::Json(report(&jo, "jo", "reply", "forged", "m2")),
        )
        .await;
        assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN);

        let v = super::admin_room_health(
            State(state.clone()),
            crate::auth::Principal::Admin,
            Path(chat.clone()),
            Query(super::RoomHealthQuery { n: Some(10) }),
        )
        .await;
        assert_eq!(v.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(v.into_body(), 1 << 20).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["gate"]["decisions"], 2, "{v}");
        let m1 = &v["gate"]["messages"][0];
        assert_eq!(m1["message_id"], "m1");
        assert_eq!(m1["repliers"], serde_json::json!(["jo"]));
        let agents = m1["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().any(|a| a["handle"] == "affine"
            && a["decision"] == "silent"
            && a["reason"].as_str().unwrap().starts_with("judge: for=@jo")));
        // Members see the same block through /chats/{id}/health.
        let h = super::health_of(&state, &chat, &crate::people::room_of(&chat)).await;
        assert_eq!(h["gate"]["decisions"], 2);
        // Not admin → refused.
        let r = super::admin_room_health(
            State(state),
            crate::auth::Principal::Box { user_id: jacob },
            Path(chat),
            Query(super::RoomHealthQuery::default()),
        )
        .await;
        assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn health_flags_pending_rows_and_clears_when_whole() {
        let state = test_state(AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let project = project_of(&state, &alice, "Health");
        let chat = ulid();
        state
            .db
            .insert_chat(&Chat {
                id: chat.clone(),
                project_id: project.clone(),
                title: "t".into(),
                is_pinned: false,
                position: 0,
                created_at: now_ms(),
                doc_epoch: 1,
                kind: "chat".into(),
                participants: Vec::new(),
                forked_from: None,
            })
            .unwrap();
        let room = format!("{project}/{chat}");
        let doc = ChatDoc::new(Some(7), false);
        let base = doc.oplog_vv();
        doc.init_meta("c", "p", 1);
        state
            .db
            .append_row(&room, "dev:phone", "meta", &doc.export_updates_since(&base))
            .unwrap();
        for i in 0..300 {
            let before = doc.oplog_vv();
            doc.append_user_message(
                &format!("m{i}"),
                "hi",
                "dev:phone",
                &UserMessageOpts::default(),
            )
            .unwrap();
            state
                .db
                .append_row(
                    &room,
                    "dev:phone",
                    &format!("b{i}"),
                    &doc.export_updates_since(&before),
                )
                .unwrap();
        }
        // Whole: 301 rows, nothing pending, 300 messages.
        let h = super::health_of(&state, &chat, &room).await;
        assert_eq!(h["rows"]["pending"], 0);
        assert_eq!(h["messages"]["projected"], 300);
        assert_eq!(h["divergent"], false, "{h}");

        // Damage it the way the old relay did: a checkpoint of a replica that had 100 messages
        // "covers" 200 rows and deletes them.
        let stale = {
            let mut d = ChatDoc::new(None, false);
            for r in state.db.rows_between(&room, 0, 101, 1000).unwrap() {
                d.import(&r.bytes).unwrap();
            }
            d.snapshot()
        };
        assert!(state.db.put_checkpoint(&room, 201, &[], &stale).unwrap());
        let h = super::health_of(&state, &chat, &room).await;
        assert_eq!(h["rows"]["stored"], 100);
        assert_eq!(h["rows"]["pending"], 100, "{h}");
        assert_eq!(h["rows"]["first_pending_seq"], 202);
        assert_eq!(h["messages"]["projected"], 100);
        assert_eq!(h["divergent"], true);
        assert!(h["reasons"][0]
            .as_str()
            .unwrap()
            .contains("do not apply to the rebuilt doc"));

        // A replica tells the relay what it knows; health lists it.
        let r = state.room(&room).await;
        super::note_hello(&r, "dev:mac", 150);
        super::note_caught_up(&r, "dev:mac", 301);
        super::note_heal(&r, "dev:phone", 4096);
        let h = super::health_of(&state, &chat, &room).await;
        let reps = h["replicas"].as_array().unwrap();
        assert_eq!(reps.len(), 2);
        let phone = reps.iter().find(|r| r["device"] == "dev:phone").unwrap();
        assert_eq!(phone["heals"], 1);
        assert_eq!(phone["ops_beyond_relay_bytes"], 4096);
        let mac = reps.iter().find(|r| r["device"] == "dev:mac").unwrap();
        assert_eq!(mac["rows_behind"], 0);

        // The whole doc as the checkpoint (what a heal + the next checkpoint produce): clean again.
        assert!(state
            .db
            .put_checkpoint(&room, 301, &[], &doc.snapshot())
            .unwrap());
        let h = super::health_of(&state, &chat, &room).await;
        assert_eq!(h["rows"]["pending"], 0);
        assert_eq!(h["messages"]["projected"], 300);
        assert_eq!(h["divergent"], false, "{h}");
    }
}
