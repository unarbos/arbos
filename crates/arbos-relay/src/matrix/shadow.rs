//! Shadow-compare (design §4 stage B): per chat, does the homeserver say the same as the relay?
//! Message ids and count, member set, and per-user unread counts. Pure read; the report is the
//! gate for the read flip and for every ring (zero divergence for 7 days on ring 0).

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use arbos_core::doc::message_text;

use super::backfill::load_doc;
use super::client::Client;
use super::worker::room_spec;
use crate::state::AppState;

#[derive(Debug, Default, Clone, Serialize)]
pub struct ChatDiff {
    pub chat_id: String,
    pub project_id: Option<String>,
    pub room_id: Option<String>,
    /// Finished messages with text in the Loro doc.
    pub relay_messages: usize,
    /// Events the relay has recorded as mirrored.
    pub mirrored_messages: usize,
    /// Of the newest 100 relay messages, which are not in the room; and, for a chat read on
    /// the homeserver, the earlier rows no device has carried in yet.
    pub missing: Vec<String>,
    pub members_relay: Vec<String>,
    pub members_room: Vec<String>,
    pub members_equal: bool,
    pub ok: bool,
    pub note: Option<String>,
    /// Newest event differs from the relay's newest row (stream order vs timestamp); not a
    /// divergence.
    pub order_note: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct UnreadDiff {
    pub user_id: String,
    pub chat_id: String,
    pub relay: i64,
    pub homeserver: i64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub chats: usize,
    pub diverged: usize,
    pub no_room: usize,
    pub diffs: Vec<ChatDiff>,
    pub unread_diffs: Vec<UnreadDiff>,
    pub unread_checked: usize,
    /// Pairs where the user never read the chat on the relay: the relay counts only announced
    /// events, the homeserver the whole backfilled history — both mean "never opened".
    pub unread_never_read: usize,
}

fn relay_message_ids(state: &AppState, chat_id: &str, project_id: Option<&str>) -> Vec<String> {
    let Ok(Some(doc)) = load_doc(state, chat_id, project_id) else {
        return Vec::new();
    };
    doc.projected_rows()
        .iter()
        .filter(|r| !matches!(r["status"].as_str(), Some("queued" | "streaming")))
        .filter(|r| !message_text(r).trim().is_empty())
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect()
}

/// Compare one chat. `only_chat` limits the run; `None` compares every mirrored room.
pub async fn run(state: &AppState, only_chat: Option<&str>, with_unread: bool) -> Report {
    let client = Client::new(state.http.clone(), state.matrix.cfg.clone());
    let cfg = &state.matrix.cfg;
    let rooms = state.db.matrix_rooms_all().unwrap_or_default();
    let mut rep = Report::default();
    let mut room_of_chat: HashMap<String, String> = HashMap::new();
    for room in rooms.iter().filter(|r| r.closed_at.is_none()) {
        if only_chat.is_some_and(|c| c != room.chat_id) {
            continue;
        }
        // Servers and channels live only on the homeserver: nothing on the relay to compare.
        if matches!(room.kind.as_str(), "space" | "channel") {
            continue;
        }
        rep.chats += 1;
        room_of_chat.insert(room.chat_id.clone(), room.room_id.clone());
        let mut d = ChatDiff {
            chat_id: room.chat_id.clone(),
            project_id: room.project_id.clone(),
            room_id: Some(room.room_id.clone()),
            ..Default::default()
        };
        let ids = relay_message_ids(state, &room.chat_id, room.project_id.as_deref());
        d.relay_messages = ids.len();
        let mirrored: BTreeSet<String> = state
            .db
            .matrix_events_of(&room.chat_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
        d.mirrored_messages = mirrored.len();
        // An encrypted room is not mirrored into, so its messages are not "missing".
        if !room.encrypted {
            d.missing = ids
                .iter()
                .rev()
                .take(100)
                .filter(|id| !mirrored.contains(*id))
                .cloned()
                .collect();
        }
        // A chat its members read on the homeserver is whole there once its earlier rows have
        // been carried in (`history.rs`); the ones still waiting are missing.
        let native = room.project_id.is_none()
            && crate::people::load_chat(state, &room.chat_id)
                .ok()
                .and_then(|cm| super::people_native_room(state, &cm))
                .is_some();
        if native {
            for id in super::history::missing(state, &room.chat_id, room.encrypted) {
                if d.missing.len() < 100 && !d.missing.contains(&id) {
                    d.missing.push(id);
                }
            }
        }
        match room_spec(state, &room.chat_id, room.project_id.as_deref()) {
            Some(spec) => {
                d.members_relay = spec.members.iter().map(|m| m.mxid.clone()).collect();
                d.members_relay.sort();
                match client.joined_members(&room.room_id, &spec.creator).await {
                    Ok(mut joined) => {
                        joined.retain(|m| cfg.member_of_mxid(m).is_some());
                        joined.sort();
                        d.members_room = joined;
                    }
                    Err(e) => d.note = Some(format!("joined_members: {e}")),
                }
                // The newest event in the room should be the newest the relay knows.
                if d.note.is_none() && !ids.is_empty() {
                    match client.messages_back(&room.room_id, &spec.creator, 20).await {
                        Ok(chunk) => {
                            let newest_room = chunk
                                .iter()
                                .filter(|e| e["type"] == "m.room.message")
                                .find_map(|e| {
                                    e["content"]["life.arbos.id"].as_str().map(str::to_owned)
                                });
                            // Room order is stream order: a live message that landed while
                            // history was still being replayed sits after older rows. Content
                            // parity is what `missing` measures; this stays informational.
                            if newest_room.as_deref() != ids.last().map(String::as_str)
                                && d.missing.is_empty()
                            {
                                d.order_note = Some(format!(
                                    "newest in room {:?} vs relay {:?}",
                                    newest_room,
                                    ids.last()
                                ));
                            }
                        }
                        Err(e) => d.note = Some(format!("messages: {e}")),
                    }
                }
            }
            None => d.note = Some("chat gone on the relay, room still open".into()),
        }
        d.members_equal = d.members_relay == d.members_room;
        d.ok = d.missing.is_empty() && d.members_equal && d.note.is_none();
        if !d.ok {
            rep.diverged += 1;
        }
        rep.diffs.push(d);
    }
    if only_chat.is_none() {
        // Chats with history but no room yet.
        for u in state.db.list_users().unwrap_or_default() {
            for cm in state
                .db
                .chats_of_member(&format!("user:{}", u.id))
                .unwrap_or_default()
            {
                if !room_of_chat.contains_key(&cm.chat.id)
                    && !relay_message_ids(state, &cm.chat.id, None).is_empty()
                {
                    room_of_chat.insert(cm.chat.id.clone(), String::new());
                    rep.no_room += 1;
                }
            }
        }
    }
    if with_unread {
        for u in state.db.list_users().unwrap_or_default() {
            let relay: HashMap<String, i64> = state
                .db
                .unread_for_user(&u.id)
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.chat_id, c.unread))
                .collect();
            let marked: HashSet<String> = state
                .db
                .matrix_read_marked_chats(&u.id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let Ok(hs) = client.unread_counts(&cfg.user_mxid(&u.id)).await else {
                continue;
            };
            for (room_id, notif, _hl) in hs {
                let Some(chat) = state.db.matrix_room_by_room_id(&room_id).ok().flatten() else {
                    continue;
                };
                if only_chat.is_some_and(|c| c != chat.chat_id) {
                    continue;
                }
                rep.unread_checked += 1;
                let r = relay.get(&chat.chat_id).copied().unwrap_or(0);
                if r > 0 && notif >= r && !marked.contains(&chat.chat_id) {
                    rep.unread_never_read += 1;
                    continue;
                }
                // Synapse may lag by one during a write (design §4 stage B table).
                if !(r == notif || notif + 1 == r && notif > 0) {
                    rep.unread_diffs.push(UnreadDiff {
                        user_id: u.id.clone(),
                        chat_id: chat.chat_id,
                        relay: r,
                        homeserver: notif,
                    });
                }
            }
        }
    }
    rep
}

pub fn to_json(rep: &Report) -> Value {
    serde_json::to_value(rep).unwrap_or(Value::Null)
}

/// Where nightly reports go: `<data>/matrix/shadow-<YYYY-MM-DD>.json`.
fn report_dir(state: &AppState) -> std::path::PathBuf {
    state.cfg.data_dir.join("matrix")
}

/// The newest stored nightly report, if any.
pub fn last_report(state: &AppState) -> Option<Value> {
    let mut files: Vec<_> = std::fs::read_dir(report_dir(state))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("shadow-") && n.ends_with(".json"))
        })
        .collect();
    files.sort();
    let path = files.pop()?;
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Run the full compare (with unread counts) and keep the report on disk.
pub async fn run_and_store(state: &AppState) -> Report {
    let rep = run(state, None, true).await;
    let dir = report_dir(state);
    let _ = std::fs::create_dir_all(&dir);
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut v = to_json(&rep);
    v["at"] = Value::String(chrono::Utc::now().to_rfc3339());
    if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
        let _ = std::fs::write(dir.join(format!("shadow-{day}.json")), bytes);
    }
    tracing::info!(
        chats = rep.chats,
        diverged = rep.diverged,
        no_room = rep.no_room,
        unread_diffs = rep.unread_diffs.len(),
        "matrix shadow-compare"
    );
    rep
}

/// Nightly at 03:10 UTC while `ARBOS_MATRIX` is not off (the stage B gate reads these).
pub fn spawn_nightly(state: AppState) {
    if !state.matrix.cfg.mode.writes() {
        return;
    }
    tokio::spawn(async move {
        loop {
            let now = chrono::Utc::now();
            let next = {
                let today = now.date_naive().and_hms_opt(3, 10, 0).expect("time");
                let candidate = today.and_utc();
                if candidate > now {
                    candidate
                } else {
                    candidate + chrono::Duration::days(1)
                }
            };
            let wait = (next - now).to_std().unwrap_or_default();
            tokio::time::sleep(wait).await;
            if state.matrix.cfg.disabled() {
                continue;
            }
            run_and_store(&state).await;
        }
    });
}
