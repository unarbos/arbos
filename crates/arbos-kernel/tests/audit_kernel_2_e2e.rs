//! Multitasking audit, second kernel batch: the kernel's own chores stay
//! out of the user's lists (scenario 17), and a coordinator that dispatches
//! work without touching the project page is told so (fix 10).

mod common;

use common::{start_kernel_replay, Attach};
use std::time::Duration;

/// Poll the transcript until `ok` holds or `timeout` passes. The attach
/// stream's first `idle` frame can be the agent's state at attach, before
/// the turn; the file is the truth.
fn wait_transcript(
    place: &std::path::Path,
    timeout: Duration,
    ok: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let evs = transcript(place);
        if ok(&evs) || std::time::Instant::now() >= deadline {
            return evs;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

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
    // The first helper sleeps four seconds, so its report lands after
    // root's spawn turn has ended and opens the done turn the test reads
    // (a report landing mid-turn folds in and opens none — the red on a
    // loaded runner). The second is waited on: its report is in the
    // spawn result, one turn, no done wake — the shape does not matter for
    // what the second round asserts (the user's words re-arm the nudge).
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper\",\"task\":\"Run: bash `sleep 4`. Then say one word.\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started a helper\"}\n",
        "{\"agent\":\"helper\",\"content\":\"pausing\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 4\",\"description\":\"A pause\"}}]}\n",
        "{\"agent\":\"helper\",\"content\":\"one word\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
        "{\"agent\":\"root\",\"content\":\"delegating again\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper-two\",\"task\":\"say one word\",\"wait\":true}}]}\n",
        "{\"agent\":\"helper-two\",\"content\":\"one word\"}\n",
        "{\"agent\":\"root\",\"content\":\"started another\"}\n",
    );
    let mut k = start_kernel_replay("nudge", replies);
    let mut a = Attach::connect(&k.url);
    assert!(a
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .is_some());
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get a helper going"}));
    let evs = wait_transcript(&k.place, Duration::from_secs(40), |evs| {
        evs.iter().any(|e| {
            e["kind"] == "nudge"
                && e["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("project page not updated")
        })
    });
    let nudges: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|e| {
            e["kind"] == "nudge"
                && e["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("project page not updated")
        })
        .collect();
    // Once per idle period: the spawn turn nudges; the done turn that may
    // follow, also leaving the page alone, does not repeat it.
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
            e["kind"] == "nudge" && e["text"].as_str().unwrap_or("").starts_with("project page")
        })
        .unwrap();
    assert!(nudge_ix > done_ix, "{evs:#?}");
    // The done turn runs and leaves the page alone too: still one nudge.
    let evs = wait_transcript(&k.place, Duration::from_secs(40), |evs| {
        evs.iter().filter(|e| e["kind"] == "turn_complete").count() >= 2
    });
    assert_eq!(
        evs.iter().filter(|e| e["kind"] == "nudge").count(),
        1,
        "{evs:#?}"
    );
    // The user's next words re-arm it: another spawn without a page
    // change earns one more, and only one more.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "another helper"}));
    let evs = wait_transcript(&k.place, Duration::from_secs(40), |evs| {
        evs.iter().filter(|e| e["kind"] == "turn_complete").count() >= 3
    });
    assert_eq!(
        evs.iter().filter(|e| e["kind"] == "turn_complete").count(),
        3,
        "the spawn turn, its done turn, and the waited spawn: {evs:#?}"
    );
    assert_eq!(
        evs.iter().filter(|e| e["kind"] == "nudge").count(),
        2,
        "{evs:#?}"
    );
    let _ = k.child.kill();
}

/// Audit §5: a one-word way for a child (or root) to reach what earlier
/// workers did — `grep scope=history` reads every agent's transcript and
/// cites hits by agent and line.
#[test]
fn grep_scope_history_reads_other_agents_transcripts() {
    // Root waits on the helper so the report is in the spawn result and
    // one turn ends. A free-running helper that finishes after the spawn
    // turn opens a done wake; sending the next prompt in that window
    // ate "noted" and never reached grep (kernel CI red on 4770c7cf —
    // the same race as archive_children, 2026-09-17).
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"helper\",\"task\":\"say the codeword\",\"wait\":true}}]}\n",
        "{\"agent\":\"helper\",\"content\":\"the codeword is xylophone\"}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"root\",\"content\":\"looking back\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"xylophone\",\"scope\":\"history\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"found it\"}\n",
    );
    let mut k = start_kernel_replay("history-grep", replies);
    let mut a = Attach::connect(&k.url);
    assert!(a
        .wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
        .is_some());
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "get the codeword"}));
    wait_transcript(&k.place, Duration::from_secs(40), |evs| {
        evs.iter().filter(|e| e["kind"] == "turn_complete").count() >= 1
    });
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "what did the helper say?"}),
    );
    let evs = wait_transcript(&k.place, Duration::from_secs(40), |evs| {
        evs.iter()
            .any(|e| e["kind"] == "tool" && e["name"] == "grep")
    });
    let grep = evs
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "grep")
        .expect("grep tool line");
    let body = grep["body"].as_str().unwrap_or("");
    assert!(body.contains("helper · line"), "{grep:#?}");
    assert!(body.contains("xylophone"), "{body}");
    let _ = k.child.kill();
}
