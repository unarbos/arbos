//! Contract tests for what other processes depend on: the wire frames the
//! desktop and arbench speak, the plan node status graph, and the place
//! lock that keeps two kernels off one folder.

use arbos_core::{
    Do, Node, NodeStatus, Place, PlaceLock, Usage, When,
    node::can_transition,
    wire::{Frame, TreeNode},
};
use std::path::PathBuf;

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
                }],
                focus: ".arbos/agents/root".into(),
                budget: Some(Usage { used: 1, size: 2 }),
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
    assert!(serde_json::from_str::<Frame>(r#"{"type":"no_such_frame"}"#).is_err());
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
