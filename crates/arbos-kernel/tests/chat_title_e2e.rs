//! F-156: a chat nobody named gets its label from the model after its
//! first turn — Cursor's *Container image formats notebook* in place of
//! the prompt's opening words *This project is a*. One call per chat,
//! ever; the tree carries the title so every window draws it at once.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

/// The harness turns titles off for every kernel it starts; this file is
/// where they are on. Set before any kernel is spawned in this process.
fn titles_on() {
    // SAFETY: tests in this binary run kernels as child processes and read
    // this variable only when spawning them.
    unsafe { std::env::set_var("ARBOS_CHAT_TITLES", "on") };
}

const PROMPT: &str = "This project is a research notebook about container image formats (OCI, Docker v2, singularity). Keep notes in notes.md.";

#[test]
fn a_chat_nobody_named_is_titled_by_the_model_once_and_the_tree_carries_it() {
    // Every line pinned: the title call answers only to `root:title`.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"Understood: a notebook on OCI, Docker v2 and Singularity image formats. I will keep notes.md current.\"}\n",
        "{\"agent\":\"root:title\",\"content\":\"Title: \\\"Container image formats notebook.\\\"\"}\n",
        "{\"agent\":\"root\",\"content\":\"Second turn, nothing new.\"}\n",
    );
    titles_on();
    let mut k = start_kernel_replay("chat-title", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // The desktop's fallback label is already on disk, as it is in life.
    let dir = k.place.join(".arbos/agents/root");
    let mut agent = arbos_core::Agent::load(&dir).unwrap();
    agent.title = "This project is a".into();
    agent.save(&dir).unwrap();

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": PROMPT}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    // The tree frame carries the model's title, normalised: no label, no
    // quotes, no trailing full stop, four words at most.
    let tree = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "tree"
                && f["tree"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|n| n["id"] == "root" && n["title"] == "Container image formats notebook")
        })
        .expect("a tree frame with the model's title on root");
    let root = tree["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "root")
        .unwrap();
    assert_eq!(root["name"], "root", "the name is untouched: {root}");
    let agent = arbos_core::Agent::load(&dir).unwrap();
    assert_eq!(agent.title, "Container image formats notebook");
    assert_eq!(agent.name, "root");
    assert_eq!(
        std::fs::read_to_string(dir.join("title-asked"))
            .unwrap()
            .trim(),
        "asked",
        "the one call is marked on disk"
    );

    // A second turn does not ask again: the title stands and the script's
    // remaining root line is the turn's, not a title's.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "anything else?"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let agent = arbos_core::Agent::load(&dir).unwrap();
    assert_eq!(agent.title, "Container image formats notebook");
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert_eq!(
        log.matches("\"event\":\"chat_titled\"").count(),
        1,
        "titled once: {log}"
    );
    let _ = k.child.kill();
}

/// A name the person or the parent gave is theirs: no call, no change.
/// And a script with no title line makes no title (the fallback stands),
/// so no existing test's turn line is ever taken for one.
#[test]
fn a_named_chat_is_left_alone_and_an_unscripted_title_is_not_made() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"hello\"}\n",
        "{\"agent\":\"root\",\"content\":\"again\"}\n",
    );
    titles_on();
    let mut k = start_kernel_replay("chat-title-named", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let dir = k.place.join(".arbos/agents/root");
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": PROMPT}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "and again"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // Both turns ran as scripted: the title path took no line.
    let events: Vec<serde_json::Value> = std::fs::read_to_string(dir.join("transcript.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let said: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(said, vec!["hello", "again"], "{events:#?}");
    let agent = arbos_core::Agent::load(&dir).unwrap();
    assert_eq!(agent.title, "", "no title was made without a script line");
    // The attempt is still marked: one, ever.
    assert!(dir.join("title-asked").exists());

    // Named by the person: never asked at all.
    let mut agent = arbos_core::Agent::load(&dir).unwrap();
    agent.name = "Formats notebook".into();
    agent.save(&dir).unwrap();
    let _ = std::fs::remove_file(dir.join("title-asked"));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "one more"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        !dir.join("title-asked").exists(),
        "a named chat is not even asked"
    );
    let _ = k.child.kill();
}
