//! A person's save from a window's editor lands whole and only over what
//! the editor read (Jacob's co-editing ruling, the compare-and-swap half;
//! side-panels handover 6).
//!
//! `save {path, text, base_hash}`: a new file with `""`; a stale hash is
//! a conflict the asker alone hears and nothing is written; the right
//! hash lands and every window hears `saved` with the new hash; an
//! agent's edit in between makes the editor's next save a conflict; the
//! store, `.git/` and paths outside the place are refused.

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
    std::fs::create_dir_all(place.join(".git")).unwrap();
    std::fs::write(place.join(".git/config"), "[core]\n").unwrap();
}

#[test]
fn a_save_lands_only_over_what_the_editor_read() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"editing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"draft.md\",\"content\":\"the agent's version\\n\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"edited\"}\n",
    );
    let k = start_kernel_replay_prepared("save-frame", replies, "", plain);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let mut b = Attach::connect(&k.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "hello");

    // Refusals, each with its reason, none writing anything.
    for (path, word) in [
        ("../outside.md", "outside this place"),
        (".arbos/notes.md", "written with put"),
        (".git/config", "never written by hand"),
    ] {
        a.send(serde_json::json!({"type":"save","path":path,"text":"x","base_hash":""}));
        let r = a
            .wait(Duration::from_secs(5), |f| f["type"] == "saved")
            .expect("a saved reply");
        assert!(
            r["error"].as_str().unwrap_or("").contains(word),
            "{path}: {r}"
        );
    }
    assert!(!k.place.join("outside.md").exists());
    assert_eq!(
        std::fs::read_to_string(k.place.join(".git/config")).unwrap(),
        "[core]\n"
    );

    // A new file: `""` means it must not exist yet. Both windows hear it.
    a.send(
        serde_json::json!({"type":"save","path":"draft.md","text":"first draft\n","base_hash":""}),
    );
    let saved = a
        .wait(Duration::from_secs(5), |f| f["type"] == "saved")
        .expect("saved");
    assert!(saved.get("error").is_none(), "{saved}");
    assert_eq!(saved["path"], "draft.md");
    assert_eq!(saved["size"], 12);
    let hash = saved["hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64);
    assert!(!saved["by"].as_str().unwrap_or("").is_empty(), "{saved}");
    let on_b = b
        .wait(Duration::from_secs(5), |f| f["type"] == "saved")
        .expect("the other window hears the save");
    assert_eq!(on_b["hash"], hash);
    assert_eq!(
        std::fs::read_to_string(k.place.join("draft.md")).unwrap(),
        "first draft\n"
    );

    // A stale base: conflict to the asker alone, nothing written.
    a.send(serde_json::json!({"type":"save","path":"draft.md","text":"stale\n","base_hash":""}));
    let conflict = a
        .wait(Duration::from_secs(5), |f| f["type"] == "saved")
        .expect("a reply");
    assert!(
        conflict["error"]
            .as_str()
            .unwrap()
            .contains("conflict — the file exists now"),
        "{conflict}"
    );
    assert_eq!(
        conflict["hash"], hash,
        "the reply carries the file's hash now"
    );
    assert!(
        b.wait(Duration::from_secs(1), |f| f["type"] == "saved")
            .is_none(),
        "a refusal is the asker's alone"
    );
    assert_eq!(
        std::fs::read_to_string(k.place.join("draft.md")).unwrap(),
        "first draft\n"
    );

    // The right base lands.
    a.send(serde_json::json!({"type":"save","path":"draft.md","text":"second draft\n","base_hash":hash}));
    let saved2 = a
        .wait(Duration::from_secs(5), |f| f["type"] == "saved")
        .expect("saved");
    assert!(saved2.get("error").is_none(), "{saved2}");
    let hash2 = saved2["hash"].as_str().unwrap().to_string();
    assert_ne!(hash2, hash);
    assert_eq!(
        std::fs::read_to_string(k.place.join("draft.md")).unwrap(),
        "second draft\n"
    );

    // The agent edits the file; the editor's save over its old read is a
    // conflict that says so.
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"edit draft.md","attachments":[]}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert_eq!(
        std::fs::read_to_string(k.place.join("draft.md")).unwrap(),
        "the agent's version\n"
    );
    a.send(serde_json::json!({"type":"save","path":"draft.md","text":"third draft\n","base_hash":hash2}));
    let conflict = a
        .wait(Duration::from_secs(5), |f| f["type"] == "saved")
        .expect("a reply");
    assert!(
        conflict["error"]
            .as_str()
            .unwrap()
            .contains("changed since you read it"),
        "{conflict}"
    );
    assert_eq!(
        std::fs::read_to_string(k.place.join("draft.md")).unwrap(),
        "the agent's version\n",
        "nothing written over the agent's edit"
    );
    // Re-read (the conflict reply carries the hash now), then it lands.
    let now = conflict["hash"].as_str().unwrap().to_string();
    a.send(
        serde_json::json!({"type":"save","path":"draft.md","text":"third draft\n","base_hash":now}),
    );
    let saved3 = a
        .wait(Duration::from_secs(5), |f| f["type"] == "saved")
        .expect("saved");
    assert!(saved3.get("error").is_none(), "{saved3}");
    assert_eq!(
        std::fs::read_to_string(k.place.join("draft.md")).unwrap(),
        "third draft\n"
    );
}
