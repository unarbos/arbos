//! Backfill (design §4 stage A.4): walk every existing chat, queue its room and members, then
//! replay its history oldest-first as `Message` jobs with the original timestamps. The relay
//! normally never parses Loro bytes; here it rebuilds each chat doc from the room's checkpoint
//! and rows with `arbos_core::doc::ChatDoc` — read-only, in memory, nothing written back.

use serde::Serialize;
use serde_json::Value;

use arbos_core::doc::{message_text, ChatDoc};
use arbos_core::wire;

use super::worker::room_spec;
use super::Job;
use crate::people::PEOPLE_PROJECT;
use crate::state::AppState;

#[derive(Debug, Default, Clone)]
pub struct Scope {
    pub user_id: Option<String>,
    pub chat_id: Option<String>,
    pub project_id: Option<String>,
    pub rooms_only: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub chats: usize,
    pub rooms_queued: usize,
    pub messages_queued: usize,
    pub reads_queued: usize,
    pub rooms_closed: usize,
    pub skipped_empty: usize,
    pub doc_errors: usize,
}

/// One chat to mirror: `(chat_id, project_id or None for people chats)`.
type Target = (String, Option<String>);

fn targets(state: &AppState, scope: &Scope) -> Vec<Target> {
    if let Some(chat_id) = &scope.chat_id {
        return vec![(
            chat_id.clone(),
            scope.project_id.clone().filter(|p| p != PEOPLE_PROJECT),
        )];
    }
    let mut out: Vec<Target> = Vec::new();
    let users: Vec<String> = match &scope.user_id {
        Some(u) => vec![u.clone()],
        None => state
            .db
            .list_users()
            .unwrap_or_default()
            .into_iter()
            .map(|u| u.id)
            .collect(),
    };
    for u in &users {
        for cm in state
            .db
            .chats_of_member(&format!("user:{u}"))
            .unwrap_or_default()
        {
            out.push((cm.chat.id, None));
        }
        for p in state.db.projects_of_user(u).unwrap_or_default() {
            for c in state.db.list_chats(&p.id).unwrap_or_default() {
                out.push((c.id, Some(p.id.clone())));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Rebuild a chat's doc from the relay's checkpoint and rows; `None` when the room has nothing.
pub fn load_doc(
    state: &AppState,
    chat_id: &str,
    project_id: Option<&str>,
) -> anyhow::Result<Option<ChatDoc>> {
    let room = wire::room_name(project_id.unwrap_or(PEOPLE_PROJECT), chat_id);
    let checkpoint = state.db.checkpoint_bytes(&room)?;
    let mut doc = match &checkpoint {
        Some((_, bytes)) => ChatDoc::from_snapshot(None, false, bytes)?,
        None => ChatDoc::new(None, false),
    };
    let mut after = checkpoint.map(|(seq, _)| seq).unwrap_or(0);
    let mut any = checkpoint_present(&doc);
    loop {
        let rows = state.db.rows_after(&room, after, None, 500)?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            after = r.seq;
            if doc.import(&r.bytes).is_ok() {
                any = true;
            }
        }
    }
    Ok(any.then_some(doc))
}

fn checkpoint_present(doc: &ChatDoc) -> bool {
    !doc.messages_list().is_empty()
}

/// The sender string a message row carries, normalised for the room: bare `user` is the
/// project owner, bare `agent` the project's agent; `system`/`project:*`/`routine:*` stay
/// system lines.
fn sender_of(row: &Value, owner: Option<&str>, project_id: Option<&str>) -> String {
    let raw = row["sender"].as_str().unwrap_or("");
    let role = row["role"].as_str().unwrap_or("");
    match (role, raw) {
        (_, s) if s.starts_with("user:") || s.starts_with("agent:") => s.to_owned(),
        ("user", _) => owner
            .map(|o| format!("user:{o}"))
            .unwrap_or_else(|| "system".into()),
        ("assistant", _) => project_id
            .map(|p| format!("agent:{p}"))
            .unwrap_or_else(|| "system".into()),
        _ => "system".to_owned(),
    }
}

/// Parts worth carrying besides the text: attachments and visible status/error lines. Tool
/// calls, reasoning and hidden parts stay in the Loro doc (the agent control plane).
fn carried_parts(row: &Value) -> Option<Value> {
    let parts = row["parts"].as_array()?;
    let keep: Vec<Value> = parts
        .iter()
        .filter(|p| p["hidden"].as_bool() != Some(true))
        .filter(|p| {
            matches!(
                p["kind"].as_str(),
                Some("attachment" | "error" | "question" | "approval")
            )
        })
        .cloned()
        .collect();
    (!keep.is_empty()).then_some(Value::Array(keep))
}

/// Queue a `Message` job for every finished, non-empty row of `doc` whose id is not in `skip`;
/// returns `(message_id, sender, created_at)` of each. Oldest first, so the room's stream
/// order follows the chat.
fn queue_messages(
    state: &AppState,
    chat_id: &str,
    project_id: Option<&str>,
    owner: Option<&str>,
    doc: &ChatDoc,
    skip: &std::collections::HashSet<String>,
) -> Vec<(String, String, Option<i64>)> {
    let mut out = Vec::new();
    for row in doc.projected_rows() {
        let status = row["status"].as_str().unwrap_or("done");
        if matches!(status, "queued" | "streaming") {
            continue;
        }
        let Some(id) = row["id"].as_str() else {
            continue;
        };
        if skip.contains(id) {
            continue;
        }
        let text = message_text(&row);
        let parts = carried_parts(&row);
        if text.trim().is_empty() && parts.is_none() {
            continue;
        }
        let sender = sender_of(&row, owner, project_id);
        let ts = row["createdAt"].as_i64();
        state.matrix.enqueue(
            state,
            Job::Message {
                chat_id: chat_id.to_owned(),
                project_id: project_id.map(str::to_owned),
                message_id: id.to_owned(),
                sender: sender.clone(),
                text,
                parts,
                ts,
            },
        );
        out.push((id.to_owned(), sender, ts));
    }
    out
}

/// What one `Resync` found (#2120: the old "unannounced rows … queued=N" line counted every
/// row the worker had not applied yet, which read as "the announce never fired").
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Resync {
    /// `Message` jobs queued by this pass.
    pub queued: usize,
    /// Rows whose `Message` job from `notify::announce` was still in the outbox: announced,
    /// on their way, not counted.
    pub pending_skipped: usize,
    /// Queued rows that were announced (`chat_events`) but whose mirror job is gone — a hole in
    /// the mirror, not in the announce path.
    pub announced: usize,
    /// Queued rows nobody announced, where that is the design: a solo project chat (the Mac
    /// does not call notify when nobody else is in the project, `crdt-sync.md` §7c).
    pub unannounced_expected: usize,
    /// Queued rows nobody announced in a chat where every row should be: a people chat or a
    /// shared project. These are the rows the resync is the only path for.
    pub unannounced: usize,
}

/// A device pushed rows into a chat (the `Resync` job, debounced per chat): rebuild the doc and
/// queue the messages the relay has not mirrored yet. Covers what nobody announces — a person's
/// own messages in a solo project chat (`crdt-sync.md` §7c: "solo projects have nobody else to
/// tell"), rows from older apps. Rows whose announce job is still queued are left to it.
pub fn resync_chat(state: &AppState, chat_id: &str, project_id: Option<&str>) -> Resync {
    let mut rep = Resync::default();
    let Ok(Some(doc)) = load_doc(state, chat_id, project_id) else {
        return rep;
    };
    if doc.projected_rows().is_empty() {
        return rep;
    }
    let project = project_id.and_then(|p| state.db.project(p).ok().flatten());
    let owner = project.as_ref().map(|p| p.user_id.clone());
    if state
        .db
        .matrix_room(chat_id)
        .ok()
        .flatten()
        .is_none_or(|r| r.closed_at.is_some())
    {
        state.matrix.touch_chat(state, chat_id, project_id);
    }
    let mut skip: std::collections::HashSet<String> = state
        .db
        .matrix_events_of(chat_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(m, _, _)| m)
        .collect();
    let pending = state
        .db
        .matrix_pending_message_ids(chat_id)
        .unwrap_or_default();
    for id in &pending {
        if skip.insert(id.clone()) {
            rep.pending_skipped += 1;
        }
    }
    let queued = queue_messages(state, chat_id, project_id, owner.as_deref(), &doc, &skip);
    rep.queued = queued.len();
    // A solo project chat is the one place rows are unannounced by design.
    let solo_project = project.as_ref().is_some_and(|p| {
        state
            .db
            .project_members(&p.id)
            .map(|m| m.is_empty())
            .unwrap_or(true)
    });
    let now = arbos_core::ids::now_ms();
    for (message_id, sender, ts) in queued {
        if state
            .db
            .chat_event_exists(chat_id, &message_id)
            .unwrap_or(false)
        {
            rep.announced += 1;
        } else if solo_project {
            rep.unannounced_expected += 1;
        } else {
            rep.unannounced += 1;
            tracing::warn!(
                chat = %chat_id,
                project = project_id.unwrap_or(PEOPLE_PROJECT),
                message = %message_id,
                %sender,
                age_ms = ts.map(|t| now - t).unwrap_or(-1),
                "matrix resync: row nobody announced (the resync is its only path)"
            );
        }
    }
    state.matrix.counters.note_resync(&rep);
    rep
}

/// Queue the rooms and history of every chat in `scope`.
pub fn run(state: &AppState, scope: Scope) -> Report {
    let mut rep = Report::default();
    for (chat_id, project_id) in targets(state, &scope) {
        rep.chats += 1;
        let owner: Option<String> = match &project_id {
            Some(p) => state.db.project(p).ok().flatten().map(|p| p.user_id),
            None => None,
        };
        let doc = match load_doc(state, &chat_id, project_id.as_deref()) {
            Ok(Some(d)) => Some(d),
            Ok(None) => None,
            Err(e) => {
                rep.doc_errors += 1;
                tracing::warn!(chat = %chat_id, error = %e, "matrix backfill: doc rebuild failed");
                None
            }
        };
        // A project tab that never had a message gets no room: it would only be noise (a doc
        // with only `meta` rows counts as empty). One made by mistake is closed again.
        let has_messages = doc.as_ref().is_some_and(|d| !d.projected_rows().is_empty());
        if !has_messages && project_id.is_some() {
            rep.skipped_empty += 1;
            if state
                .db
                .matrix_room(&chat_id)
                .ok()
                .flatten()
                .is_some_and(|r| r.closed_at.is_none())
            {
                state.matrix.close_chat(state, &chat_id);
            }
            continue;
        }
        state.matrix.enqueue(
            state,
            Job::SyncRoom {
                chat_id: chat_id.clone(),
                project_id: project_id.clone(),
            },
        );
        rep.rooms_queued += 1;
        if scope.rooms_only {
            continue;
        }
        let Some(doc) = doc else { continue };
        rep.messages_queued += queue_messages(
            state,
            &chat_id,
            project_id.as_deref(),
            owner.as_deref(),
            &doc,
            &std::collections::HashSet::new(),
        )
        .len();
    }
    // Rooms whose chat is gone (a closed project tab, a deleted group) get closed.
    if scope.chat_id.is_none() && scope.user_id.is_none() && scope.project_id.is_none() {
        for (chat_id, project_id) in state.db.matrix_open_chat_rooms().unwrap_or_default() {
            if room_spec(state, &chat_id, project_id.as_deref()).is_none() {
                state.matrix.close_chat(state, &chat_id);
                rep.rooms_closed += 1;
            }
        }
    }
    // Read marks: a member gets a receipt where their mark is (the worker places it on the
    // event at the mark, or on the newest event once caught up); a member with nothing unread
    // but no mark gets one on the newest event.
    if !scope.rooms_only {
        let users: Vec<String> = match &scope.user_id {
            Some(u) => vec![u.clone()],
            None => state
                .db
                .list_users()
                .unwrap_or_default()
                .into_iter()
                .map(|u| u.id)
                .collect(),
        };
        let wanted: std::collections::HashSet<String> =
            targets(state, &scope).into_iter().map(|(c, _)| c).collect();
        for u in users {
            let unread: std::collections::HashSet<String> = state
                .db
                .unread_for_user(&u)
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.unread > 0)
                .map(|c| c.chat_id)
                .collect();
            let marked: std::collections::HashSet<String> = state
                .db
                .matrix_read_marked_chats(&u)
                .unwrap_or_default()
                .into_iter()
                .collect();
            // People chats by membership; project chats as owner or guest.
            let mut mine: Vec<String> = state
                .db
                .chats_of_member(&format!("user:{u}"))
                .unwrap_or_default()
                .into_iter()
                .map(|cm| cm.chat.id)
                .collect();
            let mut projects: Vec<String> = state
                .db
                .projects_of_user(&u)
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.id)
                .collect();
            projects.extend(state.db.shared_project_ids(&u).unwrap_or_default());
            for p in projects {
                mine.extend(
                    state
                        .db
                        .list_chats(&p)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|c| c.id),
                );
            }
            for chat_id in mine {
                if wanted.contains(&chat_id)
                    && (marked.contains(&chat_id) || !unread.contains(&chat_id))
                {
                    state.matrix.read(state, &chat_id, &u);
                    rep.reads_queued += 1;
                }
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn senders_normalise() {
        let r = json!({ "role": "user", "sender": "user" });
        assert_eq!(sender_of(&r, Some("gh_1"), Some("P")), "user:gh_1");
        let r = json!({ "role": "assistant", "sender": "agent" });
        assert_eq!(sender_of(&r, Some("gh_1"), Some("P")), "agent:P");
        let r = json!({ "role": "user", "sender": "user:gh_9" });
        assert_eq!(sender_of(&r, Some("gh_1"), None), "user:gh_9");
        let r = json!({ "role": "system", "sender": "system" });
        assert_eq!(sender_of(&r, Some("gh_1"), Some("P")), "system");
        let r = json!({ "role": "project", "sender": "project:X" });
        assert_eq!(sender_of(&r, None, None), "system");
    }

    #[test]
    fn only_visible_non_tool_parts_are_carried() {
        let row = json!({ "parts": [
            { "kind": "text", "text": "hi" },
            { "kind": "tool", "hidden": true },
            { "kind": "attachment", "attachment": { "name": "a.png" } },
            { "kind": "reasoning", "reasoning": "…" },
        ]});
        let p = carried_parts(&row).unwrap();
        assert_eq!(p.as_array().unwrap().len(), 1);
        assert_eq!(p[0]["kind"], "attachment");
        assert!(carried_parts(&json!({ "parts": [{ "kind": "text", "text": "x" }] })).is_none());
    }
}

#[cfg(test)]
mod doc_tests {
    //! Backfill against a real chat doc: a project chat with a user message and a finished
    //! assistant reply, stored as the relay stores it (one row of Loro bytes), comes out as one
    //! `SyncRoom` job and two `Message` jobs with the original timestamps and senders.

    use arbos_core::doc::{ChatDoc, MessageStatus, UserMessageOpts};
    use arbos_core::ids::ulid;
    use arbos_core::wire;

    use super::*;
    use crate::matrix::{Config, Matrix, Mode};
    use crate::people::tests_support::{dm, person, project_of};
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    fn dual(state: &mut AppState) {
        let mut cfg = Config::off();
        cfg.mode = Mode::Dual;
        cfg.as_token_users = "t".into();
        cfg.hs_token_users = "h".into();
        state.matrix = Matrix::new(cfg);
    }

    #[test]
    fn project_chat_history_becomes_jobs() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (owner, _) = person(&state, 2001);
        let project = project_of(&state, &owner, "Research");
        let chat_id = state.db.list_chats(&project).expect("chats")[0].id.clone();

        let doc = ChatDoc::new(Some(7), false);
        doc.init_meta(&chat_id, &project, 1);
        let user_msg = ulid();
        doc.append_user_message(
            &user_msg,
            "what is the plan?",
            "dev1",
            &UserMessageOpts::default(),
        )
        .expect("user message");
        let reply = ulid();
        doc.start_assistant_message(&reply, &user_msg, "box1");
        let part = doc.add_text_part(&reply, "text").expect("part");
        doc.append_text(&reply, &part, "Three steps.");
        doc.finish_message(&reply, MessageStatus::Done);
        let streaming = ulid();
        doc.start_assistant_message(&streaming, &user_msg, "box1");
        let bytes = doc.export_updates_since(&Default::default());
        let room = wire::room_name(&project, &chat_id);
        state
            .db
            .append_row(&room, "box1", &ulid(), &bytes)
            .expect("row");

        let rep = run(
            &state,
            Scope {
                user_id: Some(owner.clone()),
                ..Default::default()
            },
        );
        assert_eq!(rep.rooms_queued, 1);
        assert_eq!(rep.messages_queued, 2, "the streaming row is not mirrored");
        let due = state.db.matrix_outbox_due(10).expect("due");
        assert!(
            matches!(&due[0].job, Job::SyncRoom { chat_id: c, project_id: Some(p) } if *c == chat_id && *p == project)
        );
        // Later jobs wait behind the sync (one chat, strict order).
        assert_eq!(due.len(), 1);
        state.db.matrix_outbox_done(due[0].id).expect("done");
        let due = state.db.matrix_outbox_due(10).expect("due");
        let Job::Message {
            sender,
            text,
            ts,
            message_id,
            ..
        } = &due[0].job
        else {
            panic!("message job");
        };
        assert_eq!(sender, &format!("user:{owner}"));
        assert_eq!(text, "what is the plan?");
        assert_eq!(message_id, &user_msg);
        assert!(ts.is_some());
        state.db.matrix_outbox_done(due[0].id).expect("done");
        let due = state.db.matrix_outbox_due(10).expect("due");
        let Job::Message { sender, text, .. } = &due[0].job else {
            panic!("message job");
        };
        assert_eq!(sender, &format!("agent:{project}"));
        assert_eq!(text, "Three steps.");
    }

    #[test]
    fn resync_queues_only_unannounced_rows() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (owner, _) = person(&state, 2003);
        let project = project_of(&state, &owner, "Solo");
        let chat_id = state.db.list_chats(&project).expect("chats")[0].id.clone();
        let doc = ChatDoc::new(Some(3), false);
        doc.init_meta(&chat_id, &project, 1);
        let first = ulid();
        doc.append_user_message(&first, "first", "dev1", &UserMessageOpts::default())
            .expect("user message");
        let second = ulid();
        doc.append_user_message(&second, "second", "dev1", &UserMessageOpts::default())
            .expect("user message");
        let bytes = doc.export_updates_since(&Default::default());
        state
            .db
            .append_row(
                &wire::room_name(&project, &chat_id),
                "dev1",
                &ulid(),
                &bytes,
            )
            .expect("row");
        // The room exists and the first message was mirrored (announced earlier).
        state
            .db
            .matrix_insert_room(&chat_id, "!r:arbos.life", Some(&project), "chat")
            .expect("room");
        state
            .db
            .matrix_insert_event(&chat_id, &first, "$e1", "u", 1)
            .expect("event");

        // One pending resync per chat, due later.
        state.matrix.touch_rows(&state, &chat_id, Some(&project));
        state.matrix.touch_rows(&state, &chat_id, Some(&project));
        assert!(
            state.db.matrix_outbox_due(10).expect("due").is_empty(),
            "debounced"
        );
        assert!(state
            .db
            .matrix_pending_kind(&chat_id, "resync")
            .expect("pending"));
        let n = state.db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM matrix_outbox WHERE kind = 'resync'",
                [],
                |r| r.get::<_, i64>(0),
            )
        });
        assert_eq!(n.expect("count"), 1);

        let rep = resync_chat(&state, &chat_id, Some(&project));
        assert_eq!(rep.queued, 1);
        assert_eq!(rep.unannounced_expected, 1, "a solo project row: by design");
        assert_eq!(rep.unannounced, 0);
        let jobs: Vec<Job> = state
            .db
            .with(|c| {
                let mut st =
                    c.prepare("SELECT payload FROM matrix_outbox WHERE kind = 'message'")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<String>>>()
            })
            .expect("payloads")
            .into_iter()
            .map(|p| serde_json::from_str::<Job>(&p).expect("job"))
            .collect();
        assert_eq!(jobs.len(), 1);
        assert!(
            matches!(&jobs[0], Job::Message { message_id, text, .. } if *message_id == second && text == "second")
        );
    }

    /// #2120: the resync tells a row whose announce job is still queued (not counted), a row
    /// that was announced but never mirrored, and a row nobody announced in a chat where every
    /// row should be (a people chat) apart; only the last one counts as `unannounced`.
    #[test]
    fn resync_classifies_pending_announced_and_unannounced_rows() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (alice, _) = person(&state, 2004);
        let (bob, _) = person(&state, 2005);
        let chat_id = dm(&state, &alice, &bob);
        let doc = ChatDoc::new(Some(4), false);
        doc.init_meta(&chat_id, "people", 1);
        let opts = UserMessageOpts {
            sender: Some(format!("user:{alice}")),
            ..Default::default()
        };
        let pending = ulid();
        doc.append_user_message(&pending, "announce job still queued", "dev1", &opts)
            .expect("user message");
        let announced = ulid();
        doc.append_user_message(&announced, "announced, mirror job lost", "dev1", &opts)
            .expect("user message");
        let silent = ulid();
        doc.append_user_message(&silent, "nobody announced me", "dev1", &opts)
            .expect("user message");
        let bytes = doc.export_updates_since(&Default::default());
        state
            .db
            .append_row(
                &wire::room_name("people", &chat_id),
                "dev1",
                &ulid(),
                &bytes,
            )
            .expect("row");
        state.matrix.message(
            &state,
            &chat_id,
            None,
            &pending,
            &format!("user:{alice}"),
            "announce job still queued",
        );
        state
            .db
            .insert_chat_event(&chat_id, None, &announced, Some(&alice))
            .expect("event");

        let rep = resync_chat(&state, &chat_id, None);
        assert_eq!(
            rep,
            Resync {
                queued: 2,
                pending_skipped: 1,
                announced: 1,
                unannounced_expected: 0,
                unannounced: 1,
            }
        );
        let counters = state.matrix.counters.json();
        assert_eq!(counters["passes"], 1);
        assert_eq!(counters["unannounced"], 1);
        assert_eq!(counters["announced"], 1);
        assert_eq!(counters["pending_skipped"], 1);
        // The pending row's job is still the only one for it: no duplicate was queued.
        let pending_ids = state
            .db
            .matrix_pending_message_ids(&chat_id)
            .expect("pending");
        assert_eq!(pending_ids.len(), 3);
        let n: i64 = state
            .db
            .with(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM matrix_outbox WHERE kind = 'message' AND chat_key = ?1",
                    [&chat_id],
                    |r| r.get(0),
                )
            })
            .expect("count");
        assert_eq!(n, 3);
    }

    #[test]
    fn empty_project_tabs_get_no_room() {
        let mut state = test_state(AuthMode::Github);
        dual(&mut state);
        let (owner, _) = person(&state, 2002);
        let project = project_of(&state, &owner, "Empty");
        // A doc with only meta (what a fresh tab has) is still empty.
        let chat_id = state.db.list_chats(&project).expect("chats")[0].id.clone();
        let doc = ChatDoc::new(Some(9), false);
        doc.init_meta(&chat_id, &project, 1);
        let bytes = doc.export_updates_since(&Default::default());
        state
            .db
            .append_row(
                &wire::room_name(&project, &chat_id),
                "box1",
                &ulid(),
                &bytes,
            )
            .expect("row");
        let rep = run(
            &state,
            Scope {
                user_id: Some(owner),
                ..Default::default()
            },
        );
        assert_eq!(rep.rooms_queued, 0);
        assert_eq!(rep.skipped_empty, 1);
    }
}
