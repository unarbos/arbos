//! A kernel that cannot tell "still running" from "finished and
//! unnoticed" looks to the user exactly like the app being broken: five
//! workers "Starting" forever on Jacob's Mac while their commands had long
//! exited. Two general answers, driven here:
//!
//! - A running turn that shows nothing for `ARBOS_STALL_SECS` (five
//!   minutes in life) gets one line on its transcript saying what it is
//!   waiting on and since when, so the silence has a name.
//! - A panic on the turn's own task no longer leaves the agent running
//!   for good: the serve loop hears the turn end, the transcript ends
//!   with a failed notice and `turn_complete`, and the next message runs.

mod common;

use common::{Attach, scratch_dir, spawn_with_env};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn start(name: &str, replies: &str, env: &[(&str, &str)]) -> common::Kernel {
    let dir = scratch_dir(name);
    let file = dir.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(
        dir.join("xdg").join("arbos").join("config.toml"),
        "trace = false\n",
    )
    .unwrap();
    let replies = file.display().to_string();
    spawn_with_env(dir, &["--provider", "replay", "--replies", &replies], env)
}

#[test]
fn a_silent_turn_says_what_it_is_waiting_on_once() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 12; echo woke\",\"description\":\"A long quiet command\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"It woke.\"}\n",
    );
    let mut k = start("turn-stall", replies, &[("ARBOS_STALL_SECS", "2")]);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"run the quiet thing","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));

    // Two seconds of silence plus the loop's five-second tick: the line.
    let stall = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "notice"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Still working"))
        })
        .expect("the stall notice reached the window");
    let text = stall["event"]["text"].as_str().unwrap();
    assert_eq!(
        stall["event"]["failed"], false,
        "a report, not a failure: {text}"
    );
    assert!(text.contains("`bash`"), "names the tool in flight: {text}");
    assert!(text.contains("sleep 12"), "and the command: {text}");
    assert!(
        text.contains("Stop ends the turn"),
        "and what the user can do: {text}"
    );

    // The command ends on its own; the turn goes on and finishes.
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
    let events = transcript(&k.place);
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "It woke."),
        "{events:#?}"
    );
    let stalls = events
        .iter()
        .filter(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Still working"))
        })
        .count();
    assert_eq!(stalls, 1, "said once for one silence: {events:#?}");
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert_eq!(
        log.matches("\"event\":\"turn_stalled\"").count(),
        1,
        "{log}"
    );
    let _ = k.child.kill();
}

#[test]
fn a_panic_on_the_turn_task_ends_the_turn_in_words_and_frees_the_agent() {
    // The first turn panics before the model is asked; this answers the
    // second.
    let replies = "{\"agent\":\"root\",\"content\":\"Fine now.\"}\n";
    let mut k = start("turn-panic", replies, &[("ARBOS_TEST_PANIC_TURN", "root")]);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"first","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // Before the guard this never came: the agent stayed running for good.
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(10)),
        "the turn ends although its task panicked"
    );
    let events = transcript(&k.place);
    let notice = events
        .iter()
        .find(|e| e["kind"] == "notice" && e["failed"] == true)
        .unwrap_or_else(|| panic!("a failed notice on the transcript: {events:#?}"));
    let text = notice["text"].as_str().unwrap();
    assert!(text.contains("internal error"), "{text}");
    assert!(
        text.contains("panicked on purpose"),
        "carries the panic's message: {text}"
    );
    assert_eq!(
        events.last().map(|e| e["kind"].clone()),
        Some(serde_json::json!("turn_complete")),
        "the record ends: {events:#?}"
    );
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("\"event\":\"turn_panicked\""), "{log}");

    // The agent is free: the next message runs a normal turn.
    a.send(serde_json::json!({"type":"user","agent":"root","text":"second","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(20)));
    let events = transcript(&k.place);
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "Fine now."),
        "{events:#?}"
    );
    let _ = k.child.kill();
}
