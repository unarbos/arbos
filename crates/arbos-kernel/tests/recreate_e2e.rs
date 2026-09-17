//! qa-002: the desktop deletes a chat with `rm -rf` on its folder and, for
//! `root`, recreates it at once; a rewind restores a whole folder from git.
//! The kernel's per-agent read cursor used to outlive the folder, so the
//! new file's lines never reached a client until it outgrew the old one.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn a_restored_chat_folder_streams_its_new_lines() {
    // Three scripted replies, one per prompt: the turns run for real,
    // deterministically, with no key (the replay provider, #69/#77).
    let mut k = start_kernel_replay(
        "recreate",
        concat!(
            "{\"content\": \"noted first\"}\n",
            "{\"content\": \"noted second\"}\n",
            "{\"content\": \"noted the restore\"}\n",
        ),
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // Two prompts so the old transcript is longer than one new prompt's worth.
    // The tailed `user` line and the turn's `idle` arrive in either order:
    // the turn frame is immediate, the transcript tail is polled.
    for text in ["first", "second"] {
        a.send(serde_json::json!({"type": "user", "agent": "root", "text": text}));
        let (mut idle, mut user) = (false, false);
        let seen = a.wait(Duration::from_secs(30), |f| {
            if f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle" {
                idle = true;
            }
            if f["type"] == "event" && f["agent"] == "root" && f["event"]["kind"] == "user" {
                user = true;
            }
            idle && user
        });
        assert!(seen.is_some(), "turn idle and the user line for {text:?}");
    }
    std::thread::sleep(Duration::from_millis(500));

    // Replace the folder in one step with a *longer* transcript, as a
    // rewind from git or a restored backup does: the transcript is a new
    // file, longer than the one the kernel was tailing. A size-based reset
    // never fires; only the file's identity tells the kernel to start over.
    let root = k.place.join(".arbos").join("agents").join("root");
    let staged = k.place.join(".arbos").join("root-staged");
    copy_dir(&root, &staged);
    // The restored history has one more line at its *start*: everything the
    // kernel already read has moved, and the new line sits before its
    // remembered offset.
    let transcript = staged.join("transcript.jsonl");
    let old = std::fs::read_to_string(&transcript).unwrap();
    let notice = serde_json::to_string(&arbos_core::Event::new(arbos_core::EventKind::Notice {
        text: "restored from backup".into(),
        failed: false,
    }))
    .unwrap();
    std::fs::write(&transcript, format!("{notice}\n{old}")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::rename(&staged, &root).unwrap();

    assert!(
        a.wait(Duration::from_secs(5), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "notice"
                && f["event"]["text"] == "restored from backup"
        })
        .is_some(),
        "the restored folder's new line must reach the client"
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "after restore"}));
    // Same either-order rule as above: `wait` consumes every frame it reads,
    // so waiting for `idle` first swallowed the tailed `user` line whenever
    // it came first, and the second wait then ran out its 5 s for nothing
    // (the CI flake on #141/#147). One wait, both conditions.
    let (mut idle, mut user) = (false, false);
    let seen = a.wait(Duration::from_secs(30), |f| {
        if f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle" {
            idle = true;
        }
        if f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "user"
            && f["event"]["text"] == "after restore"
        {
            user = true;
        }
        idle && user
    });
    assert!(
        seen.is_some(),
        "the restored chat's turn must end and its user event reach the client (idle: {idle}, user line: {user})"
    );
    let _ = k.child.kill();
}
