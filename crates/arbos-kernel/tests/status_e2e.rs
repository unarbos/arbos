//! The live line beside an agent's name: `status "…"` from the agent, the
//! kernel's guess from the tool in flight when it has not said, cleared
//! at the turn's end; on the wire as `status` frames and in the tree.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

#[test]
fn the_agents_own_line_wins_the_kernel_guesses_otherwise_and_idle_clears_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"starting\",\"calls\":[{\"name\":\"status\",\"arguments\":{\"step\":\"Reading project context and secrets inventory\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo ok\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done one\"}\n",
        "{\"agent\":\"root\",\"content\":\"second\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo again\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done two\"}\n",
    );
    let mut k = start_kernel_replay_prepared("status", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"s\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));

    // The agent's own line arrives as a status frame and lands on disk.
    let said = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root"
        })
        .expect("a status frame");
    assert_eq!(
        said["step"],
        "Reading project context and secrets inventory"
    );
    assert_eq!(said["source"], "agent");
    let file = k.place.join(".arbos/agents/root/status.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("step = \"Reading project context and secrets inventory\""),
        "{text}"
    );
    // A fresh attach mid-turn sees it in the tree.
    let mut b = Attach::connect(&k.url);
    let snap = b
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let root_node = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "root")
        .unwrap();
    assert_eq!(
        root_node["step"], "Reading project context and secrets inventory",
        "{snap}"
    );
    // The bash step that follows does not overwrite what the agent said.
    let next = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root"
        })
        .expect("the next status frame");
    assert_eq!(
        next["step"], "",
        "the turn's end clears the line, and nothing derived came before it: {next}"
    );
    assert!(!file.exists(), "idle: no status file");

    // Second turn: no `status` call, so the kernel says what runs.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "again"}));
    let derived = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root" && f["step"] != ""
        })
        .expect("a derived status frame");
    assert_eq!(derived["source"], "derived");
    assert!(
        derived["step"]
            .as_str()
            .unwrap_or("")
            .starts_with("Running sleep 2; echo again"),
        "{derived}"
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The transcript shows the tool call like any other, with its result.
    let t = std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        t.contains("\"name\":\"status\"") && t.contains("Status: Reading project context"),
        "{t}"
    );
    let _ = k.child.kill();
}

/// A stale line from a kernel that died mid-turn goes at the next start;
/// a burst of parallel tool starts becomes at most two derived frames
/// (one now, one trailing with the latest), never one per tool.
#[test]
fn stale_status_clears_at_start_and_derived_frames_are_debounced() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"many reads\",\"calls\":[",
        "{\"name\":\"read\",\"arguments\":{\"path\":\"a.txt\"}},",
        "{\"name\":\"read\",\"arguments\":{\"path\":\"b.txt\"}},",
        "{\"name\":\"read\",\"arguments\":{\"path\":\"c.txt\"}},",
        "{\"name\":\"read\",\"arguments\":{\"path\":\"d.txt\"}},",
        "{\"name\":\"read\",\"arguments\":{\"path\":\"e.txt\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    let mut k = start_kernel_replay_prepared("status-burst", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/agents/root")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"s\"\n",
        )
        .unwrap();
        for f in ["a", "b", "c", "d", "e"] {
            std::fs::write(place.join(format!("{f}.txt")), format!("{f}\n")).unwrap();
        }
        // Left by an earlier kernel that died mid-turn.
        std::fs::write(
            place.join(".arbos/agents/root/status.toml"),
            "step = \"Stale from before\"\nsince = \"2026-09-10T00:00:00Z\"\nsource = \"agent\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let snap = a
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .unwrap();
    let root_node = snap["tree"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "root")
        .unwrap();
    assert!(
        root_node.get("step").is_none(),
        "the stale line was cleared at start: {root_node}"
    );
    assert!(!k.place.join(".arbos/agents/root/status.toml").exists());
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("status_cleared"), "{log}");

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "read them all"}));
    // Count the status frames on the way to the turn's end (an empty
    // step), then give a trailing debounce frame time to land.
    let mut derived = 0;
    let mut cleared = false;
    let end = a.wait(Duration::from_secs(30), |f| {
        if f["type"] == "status" && f["agent"] == "root" {
            if f["step"] == "" {
                cleared = true;
                return true;
            } else if f["source"] == "derived" {
                derived += 1;
            }
        }
        false
    });
    assert!(end.is_some());
    while let Some(f) = a.wait(Duration::from_millis(500), |f| {
        f["type"] == "status" && f["agent"] == "root"
    }) {
        if f["step"] != "" && f["source"] == "derived" {
            derived += 1;
        }
    }
    assert!(cleared, "the turn's end cleared the line");
    assert!(
        (1..=2).contains(&derived),
        "five parallel reads gave {derived} derived frames, not one each"
    );
    let _ = k.child.kill();
}

/// A model that writes its step as a one-line reply — `status: Running
/// sleep 45`, or the prompt's own `status "Setting plan"` form Gemini
/// copies out — instead of calling the tool: the live line takes the
/// words (source `agent`), and the line is not written as a reply (it
/// was a code-looking bubble in the chat, qal J1). A status line with
/// no tool call beside it is nudged on, not the end of the turn. A
/// reply with more in it is prose, not a status.
#[test]
fn a_status_written_as_a_one_line_reply_sets_the_live_line() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"status: Running sleep 45\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 1; echo ok\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"status \\\"Setting plan\\\"\"}\n",
        "{\"agent\":\"root\",\"content\":\"The status: all good.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("status-spoken", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"s\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "go"}));
    let said = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root" && f["source"] == "agent"
        })
        .expect("the spoken status as a status frame");
    assert_eq!(said["step"], "Running sleep 45");
    let quoted = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "status" && f["agent"] == "root" && f["step"] == "Setting plan"
        })
        .expect("the quoted form is a status too");
    assert_eq!(quoted["source"], "agent");
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        !transcript.contains("status: Running sleep 45")
            && !transcript.contains("Setting plan\\\"\""),
        "a spoken status is not a reply on the transcript: {transcript}"
    );
    assert!(
        transcript.contains("\"reason\":\"status written as text\""),
        "a status with no tool call is nudged on: {transcript}"
    );
    assert!(
        transcript.contains("\"text\":\"The status: all good.\""),
        "prose that mentions status is a reply: {transcript}"
    );
    // The turn is over: the line is cleared, not left at the last step.
    assert!(!k.place.join(".arbos/agents/root/status.toml").exists());
    let _ = k.child.kill();
}
