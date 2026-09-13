//! Multitasking audit, second kernel batch: the kernel's own chores stay
//! out of the user's lists (scenario 17), and a coordinator that dispatches
//! work without touching the project page is told so (fix 10).

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn the_gc_chore_is_a_subscription_file_but_not_a_plan_row() {
    let mut k = start_kernel_replay("gc-hidden", "");
    let mut a = Attach::connect(&k.url);
    let plan = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "plan" && f["agent"] == "root"
        })
        .expect("plan frame");
    assert_eq!(plan["nodes"].as_array().map(Vec::len), Some(0), "{plan}");
    let subs = k.place.join(".arbos/agents/root/subscriptions");
    let files: Vec<_> = std::fs::read_dir(&subs)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(files.iter().any(|f| f.contains("gc")), "{files:?}");
    let text = std::fs::read_to_string(subs.join(&files[0])).unwrap();
    assert!(text.contains("internal = true"), "{text}");
    let _ = k.child.kill();
}

#[test]
fn a_coordinator_that_spawns_and_leaves_the_page_alone_is_nudged() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper\",\"task\":\"say one word\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started a helper\"}\n",
        "{\"content\":\"one word\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay("nudge", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get a helper going"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let evs = transcript(&k.place);
    let nudges: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("project page not updated")
        })
        .collect();
    assert_eq!(nudges.len(), 1, "{evs:#?}");
    // The notice sits after the spawn turn's completion line, before
    // whatever comes next: the next turn's model reads it first.
    let done_ix = evs
        .iter()
        .position(|e| e["kind"] == "turn_complete")
        .unwrap();
    let nudge_ix = evs
        .iter()
        .position(|e| {
            e["kind"] == "notice" && e["text"].as_str().unwrap_or("").starts_with("project page")
        })
        .unwrap();
    assert!(nudge_ix > done_ix, "{evs:#?}");
    let _ = k.child.kill();
}

/// Audit §5: a one-word way for a child (or root) to reach what earlier
/// workers did — `grep scope=history` reads every agent's transcript and
/// cites hits by agent and line.
#[test]
fn grep_scope_history_reads_other_agents_transcripts() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper\",\"task\":\"say the codeword\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"content\":\"the codeword is xylophone\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
        "{\"agent\":\"root\",\"content\":\"looking back\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"xylophone\",\"scope\":\"history\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"found it\"}\n",
    );
    let mut k = start_kernel_replay("history-grep", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::thread::sleep(Duration::from_secs(2));
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "what did the helper say?"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(500));
    let evs = transcript(&k.place);
    let grep = evs
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "grep")
        .expect("grep tool line");
    let body = grep["body"].as_str().unwrap_or("");
    assert!(body.contains("helper · line"), "{grep:#?}");
    assert!(body.contains("xylophone"), "{body}");
    let _ = k.child.kill();
}
