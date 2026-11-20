//! Membership notices (2026-10-22, #1349): when people or agents are added to a group,
//! removed from it, or leave, the relay itself writes one centred system line into the
//! group's chat — "Jacob added Home, hjkb and Doug", "Jacob removed Doug", "Alice left" — so
//! every device and every agent's context sees the same event, once, and nobody's client has
//! to invent it. A people group's chat is a Loro doc the relay stores as opaque rows; here it
//! rebuilds the doc (checkpoint + rows), appends the line as device `relay`, stores the update
//! as a new row and broadcasts it like any other row. Server channels live in Matrix rooms
//! instead: the same words go out as an `m.notice` from the server owner with
//! `life.arbos.sender: system` (`Job::ChannelNotice`), which the clients already read as a
//! system row. Agents never answer a system line (agentd skips `role: system`).

use arbos_core::doc::ChatDoc;
use arbos_core::ids::ulid;
use arbos_core::wire::{self, Frame, FrameType};
use bytes::Bytes;
use serde_json::{json, Value};

use crate::matrix::backfill::load_doc;
use crate::people::PEOPLE_PROJECT;
use crate::state::{AppState, RoomBroadcast};

/// Device id the relay signs its own rows with.
pub const RELAY_DEVICE: &str = "relay";

/// "A", "A and B", "A, B and C" — the names as a sentence.
pub fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// "<who> added <names>"; `None` when there is nobody to name.
pub fn added_line(who: &str, names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| format!("{who} added {}", join_names(names)))
}

/// "<who> removed <names>".
pub fn removed_line(who: &str, names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| format!("{who} removed {}", join_names(names)))
}

/// "<who> left".
pub fn left_line(who: &str) -> String {
    format!("{who} left")
}

/// A person as the notice names them: their name, else `@handle`, else the id.
pub fn person_name(state: &AppState, user_id: &str) -> String {
    match state.db.user_card(user_id).ok().flatten() {
        Some(card) => match card.name.as_deref().map(str::trim) {
            Some(n) if !n.is_empty() => n.to_owned(),
            _ if !card.handle.trim().is_empty() => format!("@{}", card.handle),
            _ => "Someone".to_owned(),
        },
        None => "Someone".to_owned(),
    }
}

/// An agent as the notice names it: its project's name, else the id.
pub fn agent_name(state: &AppState, project_id: &str) -> String {
    state
        .db
        .project(project_id)
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_else(|| project_id.to_owned())
}

/// One relay-authored edit of a chat's doc: rebuild it from the checkpoint and rows, apply
/// `edit`, store the update as a row signed `relay`, broadcast it to the room like any device
/// row, and let the Matrix mirror pick it up. `project_id` is `None` for a people chat.
/// Returns the row's seq. Every write the relay makes on its own authority (membership
/// notices, moderation) goes through here, so no client has to be trusted with it.
pub async fn write_doc(
    state: &AppState,
    project_id: Option<&str>,
    chat_id: &str,
    edit: impl FnOnce(&ChatDoc) -> anyhow::Result<()>,
) -> anyhow::Result<i64> {
    write_doc_as(state, project_id, chat_id, RELAY_DEVICE, edit).await
}

/// [`write_doc`] signed as `device` — the relay writing **for** a member it hosts (an outside
/// agent, `outside::<project>`), so the row says whose words these are.
pub async fn write_doc_as(
    state: &AppState,
    project_id: Option<&str>,
    chat_id: &str,
    device: &str,
    edit: impl FnOnce(&ChatDoc) -> anyhow::Result<()>,
) -> anyhow::Result<i64> {
    write_doc_deleting(state, project_id, chat_id, device, |doc| {
        edit(doc).map(|()| Vec::new())
    })
    .await
}

/// How long a relay write waits for a room's admission lock before it gives up. A push holds
/// the lock while it is judged and stored, which is milliseconds on a warm room; the first
/// push after a restart also holds it across the rebuild of the room's doc, which reads the
/// room's whole history and has no bound of its own. A relay write that cannot get in by
/// then fails, as it does on a database error, and its caller retries or reports.
const ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The room's admission lock for a relay write, or an error after [`ADMISSION_WAIT`].
pub(crate) async fn admission(
    room: &crate::state::Room,
) -> anyhow::Result<tokio::sync::MutexGuard<'_, ()>> {
    admission_within(room, ADMISSION_WAIT).await
}

async fn admission_within(
    room: &crate::state::Room,
    wait: std::time::Duration,
) -> anyhow::Result<tokio::sync::MutexGuard<'_, ()>> {
    tokio::time::timeout(wait, room.push_admission.lock())
        .await
        .map_err(|_| anyhow::anyhow!("the room is busy storing a push"))
}

/// [`write_doc_as`] for an edit that may take messages out of the chat: `edit` returns the
/// ids it deleted, and they are recorded as deleted in the same transaction as the row, so a
/// deletion the relay made is never stored without its record (`push_guard::corrections`
/// reads it to keep a deleted message from being pushed back).
///
/// The row is stored and broadcast under the room's admission lock, the one a device's push
/// holds while it is stored with its correction and broadcast. So this row gets its seq and
/// leaves in that order: never ahead of a push stored before it, never between a push and the
/// relay's correction to it (a receiver drops a row whose seq is not above the last one it
/// sent on). The lock is held for the store and the broadcast only. Reading the room's doc
/// and making the edit happen before it, so a relay write that is slow to build — an outside
/// agent's stream sends one per event — does not hold up the room's pushes while it does.
/// The edit is made on the doc as it was read; a push stored in between is merged by the
/// document, as it is for any two writers.
pub async fn write_doc_deleting(
    state: &AppState,
    project_id: Option<&str>,
    chat_id: &str,
    device: &str,
    edit: impl FnOnce(&ChatDoc) -> anyhow::Result<Vec<String>>,
) -> anyhow::Result<i64> {
    let room_name = wire::room_name(project_id.unwrap_or(PEOPLE_PROJECT), chat_id);
    let doc = load_doc(state, chat_id, project_id)?.unwrap_or_else(|| ChatDoc::new(None, false));
    let base = doc.oplog_vv();
    let deleted = edit(&doc)?;
    let bytes = doc.export_updates_since(&base);
    anyhow::ensure!(!bytes.is_empty(), "the edit produced no update");
    let batch_id = ulid();
    let room = state.room(&room_name).await;
    let _admission = admission(&room).await?;
    let (seq, dup, _) = state.db.append_row_with(
        &room_name,
        device,
        &batch_id,
        &bytes,
        &crate::db::RowEffects {
            chat_id,
            deleted: &deleted,
            ..Default::default()
        },
    )?;
    if !dup {
        let frame = Frame::new(
            FrameType::Row,
            json!({ "seq": seq, "device": device, "batchId": batch_id }),
            Bytes::from(bytes),
        )
        .encode();
        let _ = room.tx.send(RoomBroadcast {
            from_conn: 0,
            bytes: frame,
            seq,
        });
        state.matrix.touch_rows(state, chat_id, project_id);
    }
    Ok(seq)
}

/// Replace the room's stored history with a snapshot that starts **now**: the live state plus
/// nothing older. After a secret redaction the rows and the old checkpoint still hold the
/// removed text as past ops; this writes a shallow checkpoint at the current head and deletes
/// every row it covers, so the relay keeps no copy. Devices that were offline across the
/// redaction and push an update depending on history before this point are the one residual
/// (documented) risk: Loro cannot merge an op whose dependencies were trimmed.
pub fn compact_shallow(
    state: &AppState,
    project_id: Option<&str>,
    chat_id: &str,
) -> anyhow::Result<()> {
    let room_name = wire::room_name(project_id.unwrap_or(PEOPLE_PROJECT), chat_id);
    let Some(doc) = load_doc(state, chat_id, project_id)? else {
        return Ok(());
    };
    let st = state.db.room_state(&room_name)?;
    let bytes = doc.shallow_snapshot();
    anyhow::ensure!(!bytes.is_empty(), "empty shallow snapshot");
    let frontier = arbos_core::doc::encode_vv(&doc.oplog_vv());
    anyhow::ensure!(
        state
            .db
            .put_checkpoint(&room_name, st.head, &frontier, &bytes)?,
        "head below floor"
    );
    let _ = state.db.flush_after_scrub();
    Ok(())
}

/// The facts behind a membership line, so a client renders it from the live roster: the actor
/// by user id (never the name of the moment — "Const added Aureum", 2026-10-26, when the
/// profile had been renamed Jacob), the subjects by id. `text` stays on the row as the
/// fallback for clients that predate the field.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Notice {
    /// `added` | `removed` | `left` | `joined`.
    pub kind: &'static str,
    /// User id of the person who acted (or left / joined).
    pub actor: String,
    /// `{"kind": "user" | "agent", "id": …}` for each person or agent the line names.
    pub subjects: Vec<Value>,
}

pub fn subject_user(id: &str) -> Value {
    json!({ "kind": "user", "id": id })
}
pub fn subject_agent(project_id: &str) -> Value {
    json!({ "kind": "agent", "id": project_id })
}

/// Write `text` as a system line into a people chat's doc, with `notice` (its facts) on the
/// row, and broadcast it. Returns the row's seq. Errors are the caller's to log: a notice
/// must never fail the membership change it describes.
pub async fn post_people_notice(
    state: &AppState,
    chat_id: &str,
    text: &str,
    notice: Notice,
) -> anyhow::Result<i64> {
    let seq = write_doc(state, None, chat_id, |doc| {
        let id = ulid();
        doc.append_system_line(&id, text, RELAY_DEVICE, None)?;
        doc.set_message_fields(
            &id,
            &[(
                "notice",
                serde_json::to_value(&notice).unwrap_or(Value::Null),
            )],
        );
        Ok(())
    })
    .await?;
    tracing::info!(chat = %chat_id, seq, %text, "membership notice");
    Ok(seq)
}

/// Same words, into a server channel's Matrix room (through the worker, after the channel's
/// own sync so the room exists).
pub fn post_channel_notice(state: &AppState, channel_id: &str, text: &str) {
    state.matrix.channel_notice(state, channel_id, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_as_a_sentence() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(join_names(&s(&["Home"])), "Home");
        assert_eq!(join_names(&s(&["Home", "Doug"])), "Home and Doug");
        assert_eq!(
            join_names(&s(&["Home", "hjkb", "Doug"])),
            "Home, hjkb and Doug"
        );
        assert_eq!(
            added_line("Jacob", &s(&["Home", "hjkb", "Doug"])).as_deref(),
            Some("Jacob added Home, hjkb and Doug")
        );
        assert_eq!(
            removed_line("Jacob", &s(&["Doug"])).as_deref(),
            Some("Jacob removed Doug")
        );
        assert_eq!(added_line("Jacob", &[]), None);
        assert_eq!(left_line("Alice"), "Alice left");
    }

    /// A relay write does not wait for ever behind a push that holds the room: after
    /// its wait it fails, and gets in at once when the room is free.
    #[tokio::test]
    async fn a_relay_write_gives_up_on_a_room_held_too_long() {
        let state = crate::state::test_support::test_state(crate::state::AuthMode::Github);
        let room = state.room("people/c1").await;
        let held = room.push_admission.lock().await;
        let wait = std::time::Duration::from_millis(20);
        assert!(admission_within(&room, wait).await.is_err());
        drop(held);
        assert!(admission(&room).await.is_ok());
    }

    /// A relay write reads the room and makes its edit without the room's admission lock,
    /// and takes the lock only to store and broadcast its row. While a push holds the lock
    /// the edit still runs — a stream of relay writes does not queue pushes behind its
    /// reading and editing — and nothing is stored until the push lets go.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_write_edits_outside_the_room_lock_and_stores_under_it() {
        use crate::people::tests_support::{dm, person};
        use crate::state::test_support::test_state;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let state = test_state(crate::state::AuthMode::Github);
        let (alice, _) = person(&state, 1);
        let (bob, _) = person(&state, 2);
        let chat = dm(&state, &alice, &bob);
        let room_name = wire::room_name(PEOPLE_PROJECT, &chat);
        let room = state.room(&room_name).await;
        let head = || state.db.room_state(&room_name).map(|s| s.head).unwrap_or(0);
        let before = head();

        let push_being_stored = room.push_admission.lock().await;
        let edited = Arc::new(AtomicBool::new(false));
        let (state2, chat2, edited2) = (state.clone(), chat.clone(), edited.clone());
        let write = tokio::spawn(async move {
            write_doc(&state2, None, &chat2, move |doc| {
                doc.append_system_line("n1", "a notice", RELAY_DEVICE, None)?;
                edited2.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
        });
        for _ in 0..500 {
            if edited.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            edited.load(Ordering::SeqCst),
            "the edit does not wait for the lock"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            head(),
            before,
            "nothing is stored while a push holds the lock"
        );
        assert!(!write.is_finished());

        drop(push_being_stored);
        let seq = write.await.unwrap().unwrap();
        assert_eq!(seq, before + 1);
        assert_eq!(head(), before + 1);
    }
}
