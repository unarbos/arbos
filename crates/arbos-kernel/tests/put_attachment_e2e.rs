//! iPhone loop M-52: `user.attachments` are paths on the kernel's machine,
//! and no frame carried bytes, so a photo attached on the phone never
//! reached the agent. `put` now takes `data` (base64) and writes the file
//! under `.arbos/attachments/…` (20 MB cap, confined, never over a
//! protected file), answering `written`; the next `user` frame names the
//! same relative path and the kernel makes it absolute on the transcript,
//! so the engine reads the file the client sent.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

/// A 1×1 PNG.
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[test]
fn a_put_with_bytes_lands_under_attachments_and_the_user_line_points_at_it() {
    let mut k = start_kernel_replay(
        "put-attachment",
        "{\"agent\":\"root\",\"content\":\"a tiny image\"}\n{\"agent\":\"root\",\"content\":\"no picture came\"}\n",
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "put", "path": "attachments/photo-1.png", "data": PNG_B64}));
    let written = a
        .wait(Duration::from_secs(5), |f| f["type"] == "written")
        .expect("a written frame");
    assert!(written.get("error").is_none(), "{written}");
    assert_eq!(written["path"], "attachments/photo-1.png");
    assert_eq!(written["size"], 70);
    assert!(written["hash"].as_str().is_some_and(|h| h.len() >= 16));
    let on_disk = k.place.join(".arbos/attachments/photo-1.png");
    assert!(on_disk.is_file());
    assert_eq!(std::fs::read(&on_disk).unwrap().len(), 70);
    assert_eq!(&std::fs::read(&on_disk).unwrap()[..4], b"\x89PNG");

    // The user frame names the relative path; the transcript has it
    // absolute, where the engine's image reader looks.
    a.send(serde_json::json!({
        "type": "user", "agent": "root", "text": "what is in this picture?",
        "attachments": ["attachments/photo-1.png"], "channel": "text", "device": "phone"
    }));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    let user: serde_json::Value = transcript
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .find(|e: &serde_json::Value| e["kind"] == "user")
        .unwrap();
    let att = user["attachments"][0].as_str().unwrap();
    assert_eq!(att, on_disk.display().to_string(), "{user}");
    assert_eq!(user["device"], "phone");

    // Refusals: out of the store, not base64, a protected file, too big.
    for (path, data, why) in [
        ("../outside.png", PNG_B64, "leaves .arbos/"),
        ("/etc/passwd", PNG_B64, "relative to .arbos/"),
        ("attachments/bad.png", "not base64 at all!!", "not base64"),
        (
            "agents/root/agent.md",
            PNG_B64,
            "a file goes under attachments/",
        ),
        ("secrets.png", PNG_B64, "attachments/"),
    ] {
        a.send(serde_json::json!({"type": "put", "path": path, "data": data}));
        let w = a
            .wait(Duration::from_secs(5), |f| {
                f["type"] == "written" && f["path"] == path
            })
            .unwrap_or_else(|| panic!("a written frame for {path}"));
        let err = w["error"].as_str().unwrap_or("");
        assert!(err.contains(why), "{path}: {w}");
    }
    let big = "A".repeat(21 * 1024 * 1024 / 3 * 4);
    a.send(serde_json::json!({"type": "put", "path": "attachments/big.bin", "data": big}));
    let w = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "written" && f["path"] == "attachments/big.bin"
        })
        .expect("a written frame for the big one");
    assert!(
        w["error"]
            .as_str()
            .unwrap_or("")
            .contains("over the 20 MB cap"),
        "{w}"
    );
    assert!(!k.place.join(".arbos/attachments/big.bin").exists());
    assert!(!k.place.join(".arbos/secrets.png").exists());

    // A path that is no file here (a desktop attaching by path to a remote
    // place): dropped from the line, said loudly, the words still go.
    a.send(serde_json::json!({
        "type": "user", "agent": "root", "text": "and this one?",
        "attachments": ["/Users/jacob/Desktop/holiday.jpg"]
    }));
    let err = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "error" && f["agent"] == "root"
        })
        .expect("an error frame for the missing file");
    let detail = err["detail"].as_str().unwrap();
    assert!(
        detail.contains("holiday.jpg") && detail.contains("names no file on this machine"),
        "{detail}"
    );
    assert!(detail.contains("`put` frame"), "{detail}");
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = transcript
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let user = events
        .iter()
        .filter(|e| e["kind"] == "user")
        .last()
        .unwrap();
    assert_eq!(user["text"], "and this one?");
    assert!(
        user.get("attachments").is_none() || user["attachments"].as_array().unwrap().is_empty(),
        "the dead path is not on the line: {user}"
    );
    assert!(
        events.iter().any(|e| e["kind"] == "notice"
            && e["failed"] == true
            && e["text"].as_str().unwrap_or("").contains("holiday.jpg")),
        "{events:#?}"
    );
    let _ = k.child.kill();
}
