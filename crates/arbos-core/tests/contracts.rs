//! Contract tests for what other processes depend on: the wire frames the
//! desktop and arbench speak, the plan node status graph, and the place
//! lock that keeps two kernels off one folder.

use arbos_core::{
    Do, Event, EventKind, Node, NodeStatus, Place, PlaceLock, TranscriptTail, Usage, When,
    agent_exists, append_event, bootstrap, create_chat, list_agents, load_transcript,
    node::{can_transition, load_nodes, ready, save_node},
    read_focus, validate_focus,
    wire::{Frame, TreeNode},
    write_focus,
};
use std::{io::Write, path::PathBuf};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-core-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn roundtrip(frame: &Frame) -> serde_json::Value {
    let text = serde_json::to_string(frame).unwrap();
    let back: Frame = serde_json::from_str(&text).expect("frame round-trips");
    assert_eq!(
        serde_json::to_string(&back).unwrap(),
        text,
        "re-serialising changed the frame"
    );
    serde_json::from_str(&text).unwrap()
}

#[test]
fn every_frame_variant_round_trips_with_a_snake_case_tag() {
    let frames: Vec<(Frame, &str)> = vec![
        (
            Frame::Snapshot {
                tree: vec![TreeNode {
                    id: "root".into(),
                    name: "root".into(),
                    parent: None,
                    paused: false,
                    model: "inherit".into(),
                    kind: "agent".into(),
                    mode: String::new(),
                    prs: 0,
                }],
                focus: ".arbos/agents/root".into(),
                budget: Some(Usage {
                    used: 1,
                    size: 2,
                    cost: None,
                }),
            },
            "snapshot",
        ),
        (Frame::Tree { tree: vec![] }, "tree"),
        (
            Frame::Turn {
                agent: "root".into(),
                state: "running".into(),
                budget: None,
            },
            "turn",
        ),
        (
            Frame::Ask {
                agent: "root".into(),
                question: "q".into(),
                options: vec!["a".into(), "b".into()],
                id: Some("call_1".into()),
            },
            "ask",
        ),
        (
            Frame::Pty {
                agent: "root".into(),
                page: "t1".into(),
                data: "aGk=".into(),
            },
            "pty",
        ),
        (
            Frame::PtyIn {
                agent: "root".into(),
                page: "t1".into(),
                data: "aGk=".into(),
            },
            "pty_in",
        ),
        (
            Frame::Browser {
                agent: "root".into(),
                page: "b1".into(),
                url: "https://example.com".into(),
                screenshot: None,
            },
            "browser",
        ),
        (
            Frame::User {
                agent: "root".into(),
                text: "hi".into(),
                steer: true,
                attachments: vec!["/tmp/a.png".into()],
                channel: String::new(),
                device: String::new(),
            },
            "user",
        ),
        (
            Frame::Pause {
                agent: "root".into(),
                paused: true,
            },
            "pause",
        ),
        (
            Frame::Focus {
                path: "root".into(),
            },
            "focus",
        ),
        (
            Frame::Stop {
                agent: "root".into(),
            },
            "stop",
        ),
        (
            Frame::Compact {
                agent: "root".into(),
            },
            "compact",
        ),
        (
            Frame::Answer {
                agent: "root".into(),
                text: "teal".into(),
                id: Some("call_1".into()),
            },
            "answer",
        ),
        (
            Frame::Approve {
                agent: "root".into(),
                call_id: "c1".into(),
                allow: false,
            },
            "approve",
        ),
        (
            Frame::Undo {
                agent: "root".into(),
            },
            "undo",
        ),
        (
            Frame::SetModel {
                agent: "root".into(),
                model: "openai/gpt-5-nano".into(),
            },
            "set_model",
        ),
        (
            Frame::Error {
                agent: Some("root".into()),
                detail: "no agent nobody".into(),
            },
            "error",
        ),
        (Frame::VoiceStart, "voice_start"),
        (Frame::VoiceStop, "voice_stop"),
        (Frame::Refresh, "refresh"),
        (
            Frame::Board {
                owner: "root".into(),
                action: "open".into(),
                panel: "terminal".into(),
                terminal_ids: vec!["t1".into()],
                cwd: Some("/tmp".into()),
                title: None,
                url: None,
            },
            "board",
        ),
    ];
    for (frame, tag) in &frames {
        let value = roundtrip(frame);
        assert_eq!(value["type"], *tag, "tag for {frame:?}");
    }
}

#[test]
fn optional_frame_fields_default_and_stay_hidden() {
    // Older clients send `user` without steer/attachments.
    let frame: Frame =
        serde_json::from_str(r#"{"type":"user","agent":"root","text":"hi"}"#).unwrap();
    match frame {
        Frame::User {
            steer, attachments, ..
        } => {
            assert!(!steer);
            assert!(attachments.is_empty());
        }
        other => panic!("unexpected {other:?}"),
    }
    let text = serde_json::to_string(&Frame::Browser {
        agent: "root".into(),
        page: "b1".into(),
        url: "u".into(),
        screenshot: None,
    })
    .unwrap();
    assert!(
        !text.contains("screenshot"),
        "None must not be serialised: {text}"
    );
}

#[test]
fn unknown_frame_type_and_missing_fields_are_rejected() {
    // A frame from a newer build reads as `unknown` and is skipped, so a
    // client keeps its connection when the kernel grows a frame type.
    assert!(matches!(
        serde_json::from_str::<Frame>(r#"{"type":"no_such_frame","x":1}"#),
        Ok(Frame::Unknown)
    ));
    assert!(serde_json::from_str::<Frame>(r#"{"type":"user"}"#).is_err());
    assert!(serde_json::from_str::<Frame>(r#"{"agent":"root","text":"hi"}"#).is_err());
}

#[test]
fn node_status_graph_matches_the_documented_rules() {
    use NodeStatus::*;
    let mut node = Node::new("goal");
    node.status = Pending;
    for to in [Active, Blocked, Done, Failed, Cancelled] {
        assert!(can_transition(&node, to).is_ok(), "pending -> {to:?}");
    }
    assert!(
        can_transition(&node, Pending).is_err(),
        "no self transition"
    );
    node.status = Cancelled;
    for to in [Pending, Active, Blocked, Done, Failed] {
        assert!(
            can_transition(&node, to).is_err(),
            "cancelled is final ({to:?})"
        );
    }
    node.status = Done;
    assert!(
        can_transition(&node, Pending).is_ok(),
        "done reopens to pending"
    );
    assert!(can_transition(&node, Active).is_err());
    // A recurring node has no terminal success or failure.
    let mut recurring = Node::new("tick");
    recurring.when = When {
        every_ms: Some(60_000),
        ..When::default()
    };
    recurring.status = Active;
    assert!(can_transition(&recurring, Done).is_err());
    assert!(can_transition(&recurring, Failed).is_err());
    assert!(can_transition(&recurring, Cancelled).is_ok());
}

#[test]
fn node_serialises_with_snake_case_do_kinds() {
    for (d, kind) in [
        (Do::Agent, "agent"),
        (
            Do::Shell {
                cmd: "true".into(),
                report: None,
            },
            "shell",
        ),
        (Do::Notify { text: "hi".into() }, "notify"),
        (Do::Ask, "ask"),
    ] {
        let value = serde_json::to_value(&d).unwrap();
        assert_eq!(value["kind"], kind);
        let back: Do = serde_json::from_value(value).unwrap();
        assert_eq!(back, d);
    }
    assert_eq!(NodeStatus::parse("Canceled"), Some(NodeStatus::Cancelled));
    assert_eq!(NodeStatus::parse("nope"), None);
}

/// qa-012: a write that fails part-way (disk full, `ulimit -f`, quota)
/// must not leave the head of a line behind, or the next good append is
/// glued to it and both are lost to every reader.
#[test]
fn a_failed_append_leaves_no_partial_line() {
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
    let dir = tmp("partial");
    let transcript = dir.join("transcript.jsonl");
    let plan = dir.join("plan.jsonl");
    append_event(
        &transcript,
        &Event::new(EventKind::User {
            text: "one".into(),
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
        }),
    )
    .unwrap();
    let mut node = Node::new("first");
    node.id = 1;
    save_node(&plan, &node).unwrap();
    let t_len = std::fs::metadata(&transcript).unwrap().len();
    let p_len = std::fs::metadata(&plan).unwrap().len();

    // Cap files at 4 KB for this process, then try to append 16 KB.
    let limit = libc::rlimit {
        rlim_cur: 4096,
        rlim_max: libc::RLIM_INFINITY,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limit) }, 0);
    let big = "x".repeat(16 * 1024);
    let r1 = append_event(
        &transcript,
        &Event::new(EventKind::User {
            text: big.clone(),
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
        }),
    );
    let mut huge = Node::new(big);
    huge.id = 2;
    let r2 = save_node(&plan, &huge);
    let restore = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &restore) }, 0);

    assert!(
        r1.is_err() && r2.is_err(),
        "the oversized appends must fail, not kill the process"
    );
    assert_eq!(
        std::fs::metadata(&transcript).unwrap().len(),
        t_len,
        "transcript cut back to its last complete line"
    );
    assert_eq!(
        std::fs::metadata(&plan).unwrap().len(),
        p_len,
        "plan cut back to its last complete line"
    );

    // The next good append lands on its own line and every reader sees it.
    append_event(
        &transcript,
        &Event::new(EventKind::TurnComplete { usage: None }),
    )
    .unwrap();
    let events = load_transcript(&transcript).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].seq, 2);
    let mut second = Node::new("second");
    second.id = 2;
    save_node(&plan, &second).unwrap();
    assert_eq!(load_nodes(&plan).unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transcript_tail_reads_only_new_lines_and_numbers_them_like_load_transcript() {
    let dir = tmp("tail");
    let path = dir.join("transcript.jsonl");
    let mut tail = TranscriptTail::default();
    assert!(
        tail.read_new(&path).unwrap().is_empty(),
        "missing file is empty"
    );

    append_event(
        &path,
        &Event::new(EventKind::User {
            text: "one".into(),
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
        }),
    )
    .unwrap();
    // A damaged line and a blank line still occupy their line numbers.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"not json\n\n")
        .unwrap();
    append_event(&path, &Event::new(EventKind::TurnComplete { usage: None })).unwrap();

    let first = tail.read_new(&path).unwrap();
    let full = load_transcript(&path).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first.iter().map(|e| e.seq).collect::<Vec<_>>(),
        full.iter().map(|e| e.seq).collect::<Vec<_>>(),
        "seq must match the physical line, as load_transcript numbers it"
    );
    assert_eq!(first[1].seq, 4);
    assert!(
        tail.read_new(&path).unwrap().is_empty(),
        "nothing new, nothing returned"
    );

    // A writer mid-append: the partial line waits for its newline.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(br#"{"ts":1,"kind":"user","text":"two"}"#)
        .unwrap();
    assert!(tail.read_new(&path).unwrap().is_empty());
    f.write_all(b"\n").unwrap();
    let next = tail.read_new(&path).unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].seq, 5);

    // The file was replaced by a shorter one: the tail starts over.
    std::fs::write(&path, "").unwrap();
    append_event(
        &path,
        &Event::new(EventKind::User {
            text: "fresh".into(),
            attachments: vec![],
            channel: String::new(),
            device: String::new(),
        }),
    )
    .unwrap();
    let again = tail.read_new(&path).unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(
        again[0].seq, 1,
        "a replaced file is numbered from its first line"
    );

    // qa-002: a different file of the same length (a chat deleted and
    // recreated, a fork's transcript copied in) is also a replacement.
    let swap = dir.join("swap.jsonl");
    std::fs::copy(&path, &swap).unwrap();
    std::fs::rename(&swap, &path).unwrap();
    let swapped = tail.read_new(&path).unwrap();
    assert_eq!(
        swapped.len(),
        1,
        "same length, new inode: read from the start again"
    );
    assert_eq!(swapped[0].seq, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn focus_only_ever_names_an_existing_agent_folder() {
    // qa-004: the focus file is written from the attach socket and read by
    // every client and every prompt. It must not carry arbitrary paths.
    let dir = tmp("focus");
    let place = Place::new(&dir);
    bootstrap(&place).unwrap();
    let chat = create_chat(&place).unwrap();
    let id = chat.id.as_str();

    assert_eq!(
        validate_focus(&place, id).unwrap(),
        format!(".arbos/agents/{id}")
    );
    assert_eq!(
        validate_focus(&place, &format!(".arbos/agents/{id}/")).unwrap(),
        format!(".arbos/agents/{id}")
    );
    for bad in [
        "../../../../etc/passwd",
        ".arbos/agents/../../etc",
        "/etc/passwd",
        ".arbos/agents/does-not-exist",
        "",
        ".arbos/agents/a b",
    ] {
        assert!(
            validate_focus(&place, bad).is_err(),
            "{bad:?} must be refused"
        );
        assert!(
            write_focus(&place, bad).is_err(),
            "{bad:?} must not be written"
        );
    }
    assert_eq!(
        std::fs::read_to_string(place.focus_path()).unwrap().trim(),
        ".arbos/agents/root",
        "refused writes leave the file as it was"
    );

    // A dangling focus on disk reads as root and is repaired.
    std::fs::write(place.focus_path(), ".arbos/agents/gone\n").unwrap();
    assert_eq!(read_focus(&place), ".arbos/agents/root");
    assert_eq!(
        std::fs::read_to_string(place.focus_path()).unwrap().trim(),
        ".arbos/agents/root"
    );
    write_focus(&place, id).unwrap();
    assert_eq!(read_focus(&place), format!(".arbos/agents/{id}"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agent_exists_agrees_with_list_agents() {
    // qa-005/qa-006: one rule for "this id is an agent here".
    let dir = tmp("exists");
    let place = Place::new(&dir);
    bootstrap(&place).unwrap();
    let chat = create_chat(&place).unwrap();
    std::fs::create_dir_all(place.agent_dir("garbage")).unwrap();
    std::fs::write(place.agent_dir("garbage").join("agent.md"), b"\xff\xfe").unwrap();
    std::fs::create_dir_all(place.agent_dir("nomd")).unwrap();
    let listed: Vec<String> = list_agents(&place)
        .unwrap()
        .into_iter()
        .map(|a| a.id.to_string())
        .collect();
    for id in [
        "root",
        chat.id.as_str(),
        "garbage",
        "nomd",
        "nobody",
        "../etc",
        "",
    ] {
        assert_eq!(
            agent_exists(&place, id),
            listed.iter().any(|l| l == id),
            "{id:?}: agent_exists and list_agents must agree"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// qa-013: after a clock rewind a recurring node's next_due can sit days
/// ahead. It must not go silent for that long: anything further out than
/// one period is unreachable on the current clock and counts as due.
#[test]
fn a_recurring_node_scheduled_past_one_period_is_due_now() {
    let now = 1_000_000_000_000i64;
    let every = 30_000u64;
    let mut n = Node::new("tick");
    n.when = When {
        every_ms: Some(every),
        next_due_ms: Some(now + 10 * 86_400_000),
        ..When::default()
    };
    assert!(
        ready(&n, false, now),
        "ten days ahead on a 30s period is a rewound clock"
    );
    n.when.next_due_ms = Some(now + every as i64);
    assert!(
        !ready(&n, false, now),
        "exactly one period ahead is a normal schedule"
    );
    n.when.next_due_ms = Some(now + 5_000);
    assert!(!ready(&n, false, now), "a few seconds ahead waits");
    n.when.next_due_ms = Some(now - 10 * 86_400_000);
    assert!(
        ready(&n, false, now),
        "ten days overdue fires (once; claim re-arms from now)"
    );
    // A one-shot deferral far ahead is what the user asked for.
    let mut once = Node::new("remind");
    once.when = When {
        after_ms: Some(now + 10 * 86_400_000),
        ..When::default()
    };
    assert!(!ready(&once, false, now));
}

#[test]
fn place_lock_refuses_a_second_holder() {
    // F-015: fs4 0.13 returns Ok(false) for a contended try-lock; lock.rs
    // checks is_err(), so today this test fails. It documents the contract
    // the kernel relies on: one serve per place.
    let dir = tmp("lock");
    let place = Place::new(&dir);
    let first = PlaceLock::acquire(&place).expect("first lock");
    let second = PlaceLock::acquire(&place);
    assert!(
        second.is_err(),
        "a second PlaceLock on the same place must fail while the first is held"
    );
    drop(first);
    assert!(
        PlaceLock::acquire(&place).is_ok(),
        "the lock must be free again once the holder is dropped"
    );
}

#[test]
fn place_lock_writes_the_pid_and_removes_the_file_on_drop() {
    let dir = tmp("lockpid");
    let place = Place::new(&dir);
    let path = place.lock_path();
    {
        let _lock = PlaceLock::acquire(&place).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());
    }
    assert!(!path.exists(), "dropping the lock removes the file");
}
