//! A person's unsaved buffer is not overwritten (Jacob's co-editing
//! ruling; side-panels handover 6, the claim half).
//!
//! A window says `claim {path, held: true}` while its editor holds
//! unsaved edits. Every window hears `claimed`; the agent's file tools
//! refuse to write there and the refusal is the tool's result, so the
//! agent says in its own turn what it wanted. `held: false` releases;
//! a connection that closes releases everything it held; a window that
//! attaches later hears what is held; a path outside the place is an
//! error.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn plain(place: &std::path::Path) {
    std::fs::create_dir_all(place.join(".arbos")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"worker\"\n",
    )
    .unwrap();
}

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_held_path_is_not_written_and_the_agent_hears_who_holds_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"writing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"notes.txt\",\"content\":\"agent's line\\n\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"first attempt over\"}\n",
        "{\"agent\":\"root\",\"content\":\"writing again\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"notes.txt\",\"content\":\"agent's line\\n\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"second attempt over\"}\n",
    );
    let k = start_kernel_replay_prepared("path-claims", replies, "", plain);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let mut b = Attach::connect(&k.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "hello");

    // Outside the place: refused, nothing held.
    a.send(serde_json::json!({"type":"claim","path":"../elsewhere.txt","held":true}));
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("an error");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("outside this place"),
        "{err}"
    );

    // A holds notes.txt; both windows hear it.
    a.send(serde_json::json!({"type":"claim","path":"notes.txt","held":true}));
    let held = a
        .wait(Duration::from_secs(5), |f| f["type"] == "claimed")
        .expect("claimed on the asker");
    assert_eq!(held["held"], true, "{held}");
    assert!(
        held["path"].as_str().unwrap().ends_with("/notes.txt"),
        "absolute, under the place: {held}"
    );
    assert!(!held["by"].as_str().unwrap_or("").is_empty(), "{held}");
    assert!(held["since_ms"].as_i64().unwrap() > 0, "{held}");
    let on_b = b
        .wait(Duration::from_secs(5), |f| f["type"] == "claimed")
        .expect("claimed on the other window");
    assert_eq!(on_b["held"], true);

    // The agent's write is refused, and the refusal is its tool result.
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"write notes.txt","attachments":[]}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let events = transcript(&k.place);
    let write = events
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "write")
        .expect("the write is on record");
    let error = write["error"].as_str().unwrap_or("");
    assert!(
        error.starts_with("refused:") && error.contains("editor with unsaved changes"),
        "{write}"
    );
    assert!(
        error.contains("Nothing was written") && error.contains("Say in your reply"),
        "{write}"
    );
    assert!(!k.place.join("notes.txt").exists(), "the file was not made");

    // Released: the next write lands.
    a.send(serde_json::json!({"type":"claim","path":"notes.txt","held":false}));
    let freed = b
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "claimed" && f["held"] == false
        })
        .expect("released on the other window");
    assert!(freed.get("since_ms").is_none(), "{freed}");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"write notes.txt again","attachments":[]}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert_eq!(
        std::fs::read_to_string(k.place.join("notes.txt")).unwrap(),
        "agent's line\n"
    );

    // B cannot release what A holds; A's connection closing releases it.
    a.send(serde_json::json!({"type":"claim","path":"notes.txt","held":true}));
    let _ = b.wait(Duration::from_secs(5), |f| {
        f["type"] == "claimed" && f["held"] == true
    });
    b.send(serde_json::json!({"type":"claim","path":"notes.txt","held":false}));
    assert!(
        b.wait(Duration::from_secs(2), |f| f["type"] == "claimed"
            && f["held"] == false)
            .is_none(),
        "another window's release changes nothing"
    );
    // A late window hears what is held.
    let mut c = Attach::connect(&k.url);
    let on_attach = c
        .wait(Duration::from_secs(5), |f| f["type"] == "claimed")
        .expect("held paths on attach");
    assert_eq!(on_attach["held"], true, "{on_attach}");
    assert!(on_attach["path"].as_str().unwrap().ends_with("/notes.txt"));
    drop(a);
    let released = b
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "claimed" && f["held"] == false
        })
        .expect("released when the holder's connection closed");
    assert!(released["path"].as_str().unwrap().ends_with("/notes.txt"));
    let _ = c;
}
