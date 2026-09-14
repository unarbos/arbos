//! T3-10: a write into a file that shapes how agents behave asks the user
//! first, in every mode — auto included — and a denial is a tool error the
//! model reads. An ordinary write in auto mode still just happens.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn tools(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool")
        .collect()
}

#[test]
fn writing_hooks_toml_in_auto_mode_asks_and_a_denial_stops_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"editing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"src/note.txt\",\"content\":\"fine\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"now the hooks\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/hooks.toml\",\"content\":\"[[hook]]\\nmatch = \\\"bash\\\"\\ncommand = \\\"true\\\"\\n\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"and by shell\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo obey >> .arbos/PROTOCOL.md\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    // An old-style place: root has write and bash, mode auto.
    let mut k = start_kernel_replay_prepared("protected", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::create_dir_all(place.join("src")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"p\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));

    // The ordinary write went through without a question; the protected
    // one asks. Deny it.
    let ask = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("an approval question for the protected write");
    let q = ask["question"].as_str().unwrap_or("");
    assert!(
        q.contains(".arbos/hooks.toml") && q.contains("shapes how agents"),
        "{q}"
    );
    assert_eq!(ask["options"], serde_json::json!(["allow", "deny"]));
    let id = ask["id"].as_str().unwrap_or("").to_string();
    a.send(serde_json::json!({"type": "approve", "agent": "root", "call_id": id, "allow": false}));

    // The bash write into PROTOCOL.md asks too; allow that one.
    let ask2 = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("an approval question for the shell write");
    assert!(
        ask2["question"]
            .as_str()
            .unwrap_or("")
            .contains(".arbos/PROTOCOL.md"),
        "{ask2}"
    );
    let id2 = ask2["id"].as_str().unwrap_or("").to_string();
    a.send(serde_json::json!({"type": "approve", "agent": "root", "call_id": id2, "allow": true}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));

    let t = tools(&k.place);
    assert_eq!(t.len(), 3, "{t:#?}");
    assert!(
        t[0].get("error").is_none(),
        "the ordinary write ran: {:#?}",
        t[0]
    );
    assert!(k.place.join("src/note.txt").exists());
    let err = t[1]["error"].as_str().unwrap_or("");
    assert!(err.contains("did not allow write"), "{:#?}", t[1]);
    assert!(
        !k.place.join(".arbos/hooks.toml").exists(),
        "the denied write left no file"
    );
    assert!(
        t[2].get("error").is_none(),
        "the allowed shell write ran: {:#?}",
        t[2]
    );
    let protocol = std::fs::read_to_string(k.place.join(".arbos/PROTOCOL.md")).unwrap_or_default();
    assert!(protocol.ends_with("obey\n"), "the allowed write landed");
    let _ = k.child.kill();
}
