//! Desktop symmetry loop, cycle 58: a worker asked and parked; its parent
//! answered with `say`; the worker woke on the say, ran the command and
//! finished — and `waiting/ask-….toml` stayed, so every attach afterwards
//! got the question as a live card over a finished transcript. A turn
//! that starts on anything but the answer now closes the parked question,
//! and a kernel start sweeps one the transcript shows the agent already
//! woke past.

mod common;

use common::{Attach, restart_replay, start_kernel_replay};
use std::time::Duration;

fn asks_in(place: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(place.join(".arbos/agents/root/waiting"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("ask-"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_wake_that_is_not_the_answer_closes_the_parked_ask_and_no_attach_replays_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"asking\",\"calls\":[{\"name\":\"ask\",\"arguments\":{\"question\":\"Which colour?\",\"options\":[\"teal\",\"red\"]}}]}\n",
        "{\"agent\":\"root\",\"content\":\"moving on with what you typed\"}\n",
    );
    let mut k = start_kernel_replay("parked-ask-closed", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "pick a colour"}));
    let ask = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the question reaches the window");
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(20)),
        "parked"
    );
    assert_eq!(asks_in(&k.place).len(), 1, "parked on disk");
    // A fresh attach while parked is offered the card: that is right.
    let mut b = Attach::connect(&k.url);
    assert!(
        b.wait(Duration::from_secs(5), |f| f["type"] == "ask"
            && f["id"] == ask["id"])
            .is_some(),
        "a parked question is replayed while it stands"
    );
    // The person types a line instead of answering the card (the
    // report's shape had the parent's `say`): the wake is not the answer.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "teal, and keep going"}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        common::wait_for(Duration::from_secs(5), || asks_in(&k.place).is_empty()),
        "the parked question is closed: {:?}",
        asks_in(&k.place)
    );
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("ask_closed"), "{log}");
    // A window opening now gets no card over the finished transcript.
    let mut c = Attach::connect(&k.url);
    assert!(
        c.wait(Duration::from_secs(3), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert!(
        c.wait(Duration::from_secs(2), |f| f["type"] == "ask")
            .is_none(),
        "no ask replayed after the agent moved on"
    );
    let _ = k.child.kill();
}

#[test]
fn a_kernel_start_sweeps_an_ask_the_transcript_shows_the_agent_woke_past() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"asking\",\"calls\":[{\"name\":\"ask\",\"arguments\":{\"question\":\"Which colour?\",\"options\":[\"teal\",\"red\"]}}]}\n",
        "{\"agent\":\"root\",\"content\":\"moving on\"}\n",
    );
    let mut k = start_kernel_replay("parked-ask-swept", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "pick a colour"}));
    assert!(
        a.wait(Duration::from_secs(20), |f| f["type"] == "ask")
            .is_some()
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    let file = asks_in(&k.place);
    assert_eq!(file.len(), 1);
    let kept =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/waiting").join(&file[0])).unwrap();
    // The agent moves on (the file is closed by that wake); an older
    // kernel's leftover is staged by putting the file back afterwards.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "teal"}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || asks_in(
        &k.place
    )
    .is_empty()));
    std::fs::write(
        k.place.join(".arbos/agents/root/waiting").join(&file[0]),
        kept,
    )
    .unwrap();
    assert_eq!(asks_in(&k.place).len(), 1, "the stale file staged");
    let mut k2 = restart_replay(&mut k, "");
    let mut b = Attach::connect(&k2.url);
    assert!(
        b.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert!(
        common::wait_for(Duration::from_secs(5), || asks_in(&k2.place).is_empty()),
        "swept at start: {:?}",
        asks_in(&k2.place)
    );
    assert!(
        b.wait(Duration::from_secs(2), |f| f["type"] == "ask")
            .is_none(),
        "not replayed"
    );
    let log = std::fs::read_to_string(k2.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("a kernel before this one"), "{log}");
    let _ = k2.child.kill();
}
