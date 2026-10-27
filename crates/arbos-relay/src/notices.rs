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
    let room_name = wire::room_name(project_id.unwrap_or(PEOPLE_PROJECT), chat_id);
    let doc = load_doc(state, chat_id, project_id)?.unwrap_or_else(|| ChatDoc::new(None, false));
    let base = doc.oplog_vv();
    edit(&doc)?;
    let bytes = doc.export_updates_since(&base);
    anyhow::ensure!(!bytes.is_empty(), "the edit produced no update");
    let batch_id = ulid();
    let (seq, dup) = state
        .db
        .append_row(&room_name, RELAY_DEVICE, &batch_id, &bytes)?;
    if !dup {
        let frame = Frame::new(
            FrameType::Row,
            json!({ "seq": seq, "device": RELAY_DEVICE, "batchId": batch_id }),
            Bytes::from(bytes),
        )
        .encode();
        let room = state.room(&room_name).await;
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
}
