//! qa-002: the desktop deletes a chat with `rm -rf` on its folder and, for
//! `root`, recreates it at once; a rewind restores a whole folder from git.
//! The kernel's per-agent read cursor used to outlive the folder, so the
//! new file's lines never reached a client until it outgrew the old one.

mod common;

use common::{Attach, start_kernel};
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

fn root_events(a: &mut Attach, kind: &str, timeout: Duration) -> bool {
    a.wait(timeout, |f| {
        f["type"] == "event" && f["agent"] == "root" && f["event"]["kind"] == kind
    })
    .is_some()
}

#[test]
fn a_restored_chat_folder_streams_its_new_lines() {
    let mut k = start_kernel("recreate");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // Two prompts so the old transcript is longer than one new prompt's worth.
    for text in ["first", "second"] {
        a.send(serde_json::json!({"type": "user", "agent": "root", "text": text}));
        assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    }
    assert!(root_events(&mut a, "user", Duration::from_secs(5)));
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
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        a.wait(Duration::from_secs(5), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "user"
                && f["event"]["text"] == "after restore"
        })
        .is_some(),
        "the restored chat's user event must reach the client"
    );
    let _ = k.child.kill();
}
