//! One-shot writer for `crates/arbos-core/fixtures/` golden files.
//!
//! ```text
//! cargo run -p arbos-core --bin write_fixtures
//! ```

use std::fs;
use std::path::PathBuf;

use arbos_core::doc::{message_text, ChatDoc, MessageStatus, UserMessageOpts};
use arbos_core::wire::{Ack, BoxState, Frame, FrameType, Hello, PushHeader, RowHeader, State};
use arbos_core::PROTO_VERSION;
use loro::{LoroList, LoroMap, LoroText, LoroValue};
use serde_json::json;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn write_frames(dir: &std::path::Path) {
    let hello = Frame::new(
        FrameType::Hello,
        serde_json::to_value(Hello {
            cursor: 0,
            device: "dev:fixture".into(),
            room: "proj/chat".into(),
            proto: PROTO_VERSION,
        })
        .unwrap(),
        Vec::new(),
    );
    let state = Frame::new(
        FrameType::State,
        serde_json::to_value(State {
            head_seq: 3,
            seq_floor: 0,
            checkpoint_seq: 0,
            checkpoint_size: 0,
            row_count: 0,
            row_bytes: 0,
            box_state: BoxState::Awake,
        })
        .unwrap(),
        Vec::new(),
    );
    let row = Frame::new(
        FrameType::Row,
        serde_json::to_value(RowHeader {
            seq: 1,
            device: "dev:fixture".into(),
            batch_id: "batch-row-1".into(),
        })
        .unwrap(),
        b"row-payload".to_vec(),
    );
    let rows_done = Frame::new(FrameType::RowsDone, json!({ "headSeq": 3 }), Vec::new());
    let push = Frame::new(
        FrameType::Push,
        serde_json::to_value(PushHeader {
            heal: false,
            batch_id: "batch-push-1".into(),
        })
        .unwrap(),
        b"push-payload".to_vec(),
    );
    let ack = Frame::new(
        FrameType::Ack,
        serde_json::to_value(Ack {
            batch_id: "batch-push-1".into(),
            seq: 2,
            dup: false,
        })
        .unwrap(),
        Vec::new(),
    );

    for (name, frame) in [
        ("frame_hello.bin", hello),
        ("frame_state.bin", state),
        ("frame_row.bin", row),
        ("frame_rows_done.bin", rows_done),
        ("frame_push.bin", push),
        ("frame_ack.bin", ack),
    ] {
        fs::write(dir.join(name), frame.encode()).expect(name);
    }
}

fn write_chat_doc(dir: &std::path::Path) {
    // Fixed peer so regenerations of the same logical content stay closer.
    let doc = ChatDoc::new(Some(42), false);
    doc.init_meta("chat-fixture", "proj-fixture", 1);

    doc.transact(|d| {
        d.append_user_message(
            "msg-user",
            "hello from user",
            "dev:fixture",
            &UserMessageOpts {
                sender: Some("user:ken".into()),
                ..UserMessageOpts::default()
            },
        )
        .unwrap();
        // Flat reactions on the user row (`emoji|sender` → true).
        d.set_reaction("msg-user", "👍", "user:ken", true).unwrap();
    });

    let part_id = doc.start_assistant_message("msg-assistant", "msg-user", "dev:fixture");
    doc.append_text("msg-assistant", &part_id, "assistant says hi");
    doc.finish_message("msg-assistant", MessageStatus::Done);
    // One hidden tool part so Swift shape tests see `hiddenParts` / hiddenCount.
    doc.add_tool_part(
        "msg-assistant",
        "call-fixture-1",
        "search",
        &json!({ "q": "hi" }),
        "no hits",
        None,
        None,
        false,
    );

    doc.transact(|d| {
        d.append_system_line(
            "msg-system",
            "Sam joined the call",
            "dev:fixture",
            Some("call-1"),
        )
        .unwrap();
        d.append_greeting(
            "msg-greeting",
            "Hi, I'm Arbos.",
            "dev:fixture",
            "proj-fixture",
            "Arbos",
        )
        .unwrap();
        d.append_user_answer(
            "msg-answer",
            "42",
            "q-1",
            &json!("forty-two"),
            "dev:fixture",
        );
    });

    // Legacy nested emoji-bucket reactions on a dedicated row (no flat keys).
    {
        let msgs = doc.messages_list();
        let m = msgs.push_container(LoroMap::new()).expect("legacy row");
        m.insert("id", "msg-legacy-react").ok();
        m.insert("role", "user").ok();
        m.insert("sender", "user:legacy").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:fixture").ok();
        m.insert("createdAt", 1_700_000_000_000i64).ok();
        let reactions = m
            .insert_container("reactions", LoroMap::new())
            .expect("reactions");
        let bucket = reactions
            .insert_container("🎉", LoroMap::new())
            .expect("bucket");
        bucket.insert("user:legacy", true).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", "part-legacy").ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "legacy reactions row").ok();
        doc.doc.commit();
    }

    let snap = doc.snapshot();
    fs::write(dir.join("chat_doc.bin"), &snap).expect("chat_doc.bin");
    // Side-car expected shape (kinds / text / projected reactions / hiddenCount)
    // for the Rust test and the Swift check. Built from the same projected_rows
    // path the FFI uses.
    write_expected_rows(dir, "chat_doc_expected.json", &doc);
}

/// Side-car expected shape for a chat-doc snapshot (same fields as `chat_doc_expected.json`).
fn write_expected_rows(dir: &std::path::Path, name: &str, doc: &ChatDoc) {
    let mut expected = Vec::new();
    for json in doc.projected_rows() {
        let hidden_count = json
            .get("hiddenParts")
            .and_then(|h| h.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        expected.push(json!({
            "id": json["id"],
            "role": json["role"],
            "text": message_text(&json),
            "greeting": json.get("greeting").cloned().unwrap_or(json!(null)),
            "answersQuestionId": json.get("answersQuestionId").cloned().unwrap_or(json!(null)),
            "hiddenCount": hidden_count,
            "reactions": json.get("reactions").cloned().unwrap_or(json!({})),
        }));
    }
    fs::write(
        dir.join(name),
        serde_json::to_vec_pretty(&expected).unwrap(),
    )
    .expect(name);
}

/// Older chat-doc shapes core readers still accept (step 9b-1). One row per shape;
/// committed once so a later reader change cannot silently drop docs from older builds.
fn write_chat_doc_legacy(dir: &std::path::Path) {
    let doc = ChatDoc::new(Some(44), false);
    doc.init_meta("chat-legacy", "proj-legacy", 1);
    let msgs = doc.messages_list();
    let created = 1_700_000_000_000i64;

    // 1. Old Swift-client user row (ios_mirror shape): chatId, pending, waitingUser false,
    // empty hiddenParts, replyTo, mentions, nested attachment wrapper, LoroText body.
    {
        let m = msgs.push_container(LoroMap::new()).expect("ios user");
        m.insert("id", "msg-ios-user").ok();
        m.insert("chatId", "chat1").ok();
        m.insert("role", "user").ok();
        m.insert("sender", "user").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:ios").ok();
        m.insert("createdAt", created).ok();
        m.insert("waitingUser", false).ok();
        m.insert("pending", false).ok();
        m.insert("replyTo", "m0").ok();
        m.insert("senderHandle", "ken").ok();
        m.insert("senderName", "Ken").ok();
        m.insert("senderAvatar", "avatar-1").ok();
        let ml = m
            .insert_container("mentions", LoroList::new())
            .expect("mentions");
        ml.push("agent:p1").ok();
        ml.push("user:sam").ok();
        for k in [
            "turn",
            "decision",
            "subagentId",
            "subagentName",
            "inReplyTo",
            "usage",
            "returnContract",
            "model",
            "effort",
            "completedAt",
        ] {
            m.insert(k, LoroValue::Null).ok();
        }
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        for (name, mime, kind, blob_ref) in [
            ("pic.png", "image/png", Some("image"), Some("blob-1")),
            ("notes.txt", "text/plain", None, None),
        ] {
            let part = parts.push_container(LoroMap::new()).expect("part");
            part.insert("id", format!("part-att-{name}")).ok();
            part.insert("kind", "attachment").ok();
            let a = part
                .insert_container("attachment", LoroMap::new())
                .expect("attachment");
            a.insert("name", name).ok();
            a.insert("mime", mime).ok();
            if let Some(k) = kind {
                a.insert("kind", k).ok();
            }
            if let Some(r) = blob_ref {
                a.insert("ref", r).ok();
            }
        }
        let part = parts.push_container(LoroMap::new()).expect("text part");
        part.insert("id", "part-ios-text").ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "hello from ios").ok();
        m.insert_container("hiddenParts", LoroList::new())
            .expect("hiddenParts");
    }

    // 2. Pre-0.6.64: no reactions map at all.
    {
        let m = msgs.push_container(LoroMap::new()).expect("no reactions");
        m.insert("id", "msg-no-reactions").ok();
        m.insert("role", "user").ok();
        m.insert("sender", "user:legacy").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:legacy").ok();
        m.insert("createdAt", created + 1).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", "part-no-react").ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "no reactions map").ok();
    }

    // 3. turn as a plain string.
    {
        let m = msgs.push_container(LoroMap::new()).expect("turn string");
        m.insert("id", "msg-turn-string").ok();
        m.insert("role", "assistant").ok();
        m.insert("sender", "agent").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:legacy").ok();
        m.insert("createdAt", created + 2).ok();
        m.insert("turn", "reply").ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", "part-turn-str").ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "turn was a string").ok();
    }

    // 4. turn as a {kind} object.
    {
        let m = msgs.push_container(LoroMap::new()).expect("turn object");
        m.insert("id", "msg-turn-object").ok();
        m.insert("role", "assistant").ok();
        m.insert("sender", "agent").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:legacy").ok();
        m.insert("createdAt", created + 3).ok();
        let turn = m
            .insert_container("turn", LoroMap::new())
            .expect("turn map");
        turn.insert("kind", "reply").ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", "part-turn-obj").ok();
        part.insert("kind", "text").ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "turn was an object").ok();
    }

    // 5. Flat attachment part (no nested `attachment` wrapper).
    {
        let m = msgs.push_container(LoroMap::new()).expect("flat attach");
        m.insert("id", "msg-flat-attach").ok();
        m.insert("role", "user").ok();
        m.insert("sender", "user").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:legacy").ok();
        m.insert("createdAt", created + 4).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", "part-flat-att").ok();
        part.insert("kind", "attachment").ok();
        part.insert("name", "flat.png").ok();
        part.insert("mime", "image/png").ok();
        part.insert("ref", "blob-flat").ok();
        let text = parts.push_container(LoroMap::new()).expect("text");
        text.insert("id", "part-flat-text").ok();
        text.insert("kind", "text").ok();
        let t = text
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, "with flat attachment").ok();
    }

    // 6. Text part whose `text` is a plain string (not LoroText).
    {
        let m = msgs.push_container(LoroMap::new()).expect("plain text");
        m.insert("id", "msg-plain-text").ok();
        m.insert("role", "assistant").ok();
        m.insert("sender", "agent").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:legacy").ok();
        m.insert("createdAt", created + 5).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("id", "part-plain").ok();
        part.insert("kind", "text").ok();
        part.insert("text", "plain string text body").ok();
    }

    // 7. Agent status part next to an open question part.
    {
        let m = msgs
            .push_container(LoroMap::new())
            .expect("status+question");
        m.insert("id", "msg-status-question").ok();
        m.insert("role", "assistant").ok();
        m.insert("sender", "agent:proj-legacy").ok();
        m.insert("status", "done").ok();
        m.insert("deviceId", "dev:legacy").ok();
        m.insert("createdAt", created + 6).ok();
        m.insert("waitingUser", true).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let q = parts.push_container(LoroMap::new()).expect("question");
        q.insert("id", "part-question").ok();
        q.insert("kind", "question").ok();
        q.insert("questionId", "q-legacy").ok();
        q.insert("question", "Pick one").ok();
        q.insert("state", "open").ok();
        let st = parts.push_container(LoroMap::new()).expect("status");
        st.insert("id", "part-status").ok();
        st.insert("kind", "status").ok();
        st.insert("code", "waiting_user").ok();
        st.insert("message", "Waiting for your answer").ok();
    }

    doc.doc.commit();
    fs::write(dir.join("chat_doc_legacy.bin"), doc.snapshot()).expect("chat_doc_legacy.bin");
    write_expected_rows(dir, "chat_doc_legacy_expected.json", &doc);
}

/// Rows as the Mac fixture's old `insert_message` / `insert_json` wrote them: every
/// serde field of the Mac `Message`, including the client-only placeholders core never
/// writes (`chatId`, `pending`, `hiddenParts`, `false` flags, null options, empty
/// `mentions`). Docs synced from older Macs still carry this shape.
fn write_mac_placeholder_rows(dir: &std::path::Path) {
    let doc = ChatDoc::new(Some(43), false);
    doc.init_meta("chat-mac-legacy", "proj-mac-legacy", 1);

    let rows = [
        (
            "msg-mac-user",
            "user",
            "user",
            None,
            "hello from an old mac",
        ),
        (
            "msg-mac-assistant",
            "assistant",
            "agent",
            Some("reply"),
            "legacy assistant reply",
        ),
    ];
    let msgs = doc.messages_list();
    for (i, (id, role, sender, turn, text)) in rows.iter().enumerate() {
        let created = 1_700_000_000_000i64 + i as i64 * 1_000;
        let m = msgs.push_container(LoroMap::new()).expect("row");
        m.insert("id", *id).ok();
        m.insert("chatId", "chat-mac-legacy").ok();
        m.insert("role", *role).ok();
        m.insert("sender", *sender).ok();
        m.insert("status", "done").ok();
        match turn {
            Some(t) => m.insert("turn", *t).ok(),
            None => m.insert("turn", LoroValue::Null).ok(),
        };
        for key in [
            "decision",
            "subagentId",
            "subagentName",
            "handoffMessages",
            "inReplyTo",
            "routineName",
            "senderName",
            "senderIcon",
            "senderHandle",
            "senderAvatar",
            "senderAvatarColor",
            "replyTo",
            "askedByHandle",
            "ownerHandle",
            "usage",
            "returnContract",
            "model",
            "effort",
        ] {
            m.insert(key, LoroValue::Null).ok();
        }
        m.insert_container("mentions", LoroList::new())
            .expect("mentions");
        m.insert("waitingUser", false).ok();
        m.insert("waitingApproval", false).ok();
        m.insert("createdAt", created).ok();
        m.insert("completedAt", created).ok();
        m.insert("pending", false).ok();
        m.insert("greeting", false).ok();
        let parts = m.insert_container("parts", LoroList::new()).expect("parts");
        let part = parts.push_container(LoroMap::new()).expect("part");
        part.insert("kind", "text").ok();
        part.insert("id", format!("part-{id}")).ok();
        let t = part
            .insert_container("text", LoroText::new())
            .expect("text");
        t.insert(0, text).ok();
        m.insert_container("hiddenParts", LoroList::new())
            .expect("hiddenParts");
    }
    doc.doc.commit();

    fs::write(dir.join("mac_placeholder_rows.bin"), doc.snapshot()).expect("mac rows bin");
    let expected: Vec<_> = rows
        .iter()
        .map(|(id, role, _, _, text)| json!({ "id": id, "role": role, "text": text }))
        .collect();
    fs::write(
        dir.join("mac_placeholder_rows_expected.json"),
        serde_json::to_vec_pretty(&expected).unwrap(),
    )
    .expect("mac rows expected json");
}

fn main() {
    let dir = fixtures_dir();
    fs::create_dir_all(&dir).expect("fixtures dir");
    write_frames(&dir);
    write_chat_doc(&dir);
    write_chat_doc_legacy(&dir);
    write_mac_placeholder_rows(&dir);
    eprintln!("wrote fixtures to {}", dir.display());
}
