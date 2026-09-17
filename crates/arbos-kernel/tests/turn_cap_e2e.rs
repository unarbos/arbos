//! `[spend] turn_cap_usd`: the most any one turn may spend. #347 gave the
//! kernel a per-turn dollar cap as a harness knob; this makes it a
//! place's setting, says plainly what the user sees when a turn stops on
//! cost, and applies it to a worker's turn as much as the coordinator's
//! — the place's `cap_usd` guards the total.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    let live = place
        .join(".arbos/agents")
        .join(agent)
        .join("transcript.jsonl");
    let archived = place
        .join(".arbos/archive/agents")
        .join(agent)
        .join("transcript.jsonl");
    std::fs::read_to_string(if live.exists() { live } else { archived })
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_turn_over_the_places_per_turn_cap_ends_with_a_plain_notice_and_workers_have_the_same_cap() {
    let replies = concat!(
        // Turn 1, root: three steps at $0.60 each; the cap is $1.00, so the
        // turn ends after the second step's cost is known.
        "{\"agent\":\"root\",\"content\":\"step one\",\"cost\":0.6,\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo one\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"step two\",\"cost\":0.6,\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo two\"}}]}\n",
        // Turn 2, the user's next message: a fresh budget; a cheap turn
        // spawns a worker whose own turn runs over the same cap.
        "{\"agent\":\"root\",\"content\":\"one worker\",\"cost\":0.1,\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Pricey worker\",\"task\":\"a\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\",\"cost\":0.1}\n",
        "{\"agent\":\"pricey-worker\",\"content\":\"working\",\"cost\":0.7,\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo w1\"}}]}\n",
        "{\"agent\":\"pricey-worker\",\"content\":\"more\",\"cost\":0.7,\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo w2\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"the worker stopped on cost\",\"cost\":0.1}\n",
    );
    let mut k = start_kernel_replay_prepared("turn-cap", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n[spend]\nturn_cap_usd = 1.0\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "do three steps"}));
    // What the user sees: one plain notice — the rule working, the numbers
    // as they are, the fix in the same breath — as a `notice`
    // notification, not an error; no `interrupted` line beside it.
    let n = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "notify" && f["agent"] == "root" && f["kind"] != "reply"
        })
        .expect("the user hears the turn stopped at the cap");
    assert_eq!(n["kind"], "notice", "{n}");
    assert_eq!(n["title"], "root stopped at the per-turn cap");
    let body = n["body"].as_str().unwrap();
    assert!(
        body.starts_with("Stopped at the per-turn cap: this turn spent $1.20 on model calls, over the $1.00 you allow for one turn."),
        "{body}"
    );
    assert!(
        body.contains("raise turn_cap_usd under [spend] in .arbos/project.toml"),
        "{body}"
    );
    assert!(
        body.contains("What is in the working tree stays")
            && body.contains("A new message starts a fresh budget"),
        "{body}"
    );
    let lower = body.to_ascii_lowercase();
    assert!(
        !lower.contains("error") && !lower.contains("fail"),
        "{body}"
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    assert_eq!(
        root.iter()
            .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
            .count(),
        2,
        "the third step never ran: {root:?}"
    );
    assert!(
        root.iter().any(|e| e["text"] == "step two"),
        "the paid-for step is on record, then the notice"
    );
    let notice = root
        .iter()
        .find(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Stopped at the per-turn cap"))
        })
        .unwrap();
    assert_eq!(
        notice["failed"], false,
        "the rule working is not a failure: {notice}"
    );
    assert_eq!(notice["text"], body);
    assert!(
        root.iter().all(|e| e["kind"] != "interrupted"),
        "one closing line, not two: {root:?}"
    );
    assert!(
        root.iter()
            .all(|e| !(e["kind"] == "notice" && e["failed"] == true)),
        "{root:?}"
    );
    // The place's total still counts the turn.
    let spend = std::fs::read_to_string(k.place.join(".arbos/spend.toml")).unwrap();
    assert!(spend.contains("spent_usd = 1.2"), "{spend}");

    // A new message is a fresh budget; the worker's turn has the same cap.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "try a worker"}));
    assert!(
        common::wait_for(Duration::from_secs(40), || {
            transcript(&k.place, "root")
                .iter()
                .any(|e| e["text"] == "the worker stopped on cost")
        }),
        "root hears back: {:?}",
        transcript(&k.place, "root")
    );
    let worker = transcript(&k.place, "pricey-worker");
    assert_eq!(
        worker
            .iter()
            .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
            .count(),
        2,
        "{worker:?}"
    );
    let wn = worker
        .iter()
        .find(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Stopped at the per-turn cap"))
        })
        .expect("the worker's own notice");
    assert!(wn["text"].as_str().unwrap().contains("spent $1.40"), "{wn}");
    // The coordinator's own second turn stayed cheap: its budget is its own.
    let root = transcript(&k.place, "root");
    assert_eq!(
        root.iter()
            .filter(|e| e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Stopped at the per-turn cap")))
            .count(),
        1,
        "root was capped once, in turn 1: {root:?}"
    );
    // The parent reads the numbers and the fix in the report itself.
    let report = root
        .iter()
        .find(|e| e["kind"] == "say" && e["from"] == "pricey-worker")
        .expect("the worker's report");
    let text = report["text"].as_str().unwrap();
    assert!(
        text.contains("Stopped at the per-turn cap: this turn spent $1.40")
            && text.contains("$1.00 you allow"),
        "{report}"
    );
    assert!(
        !text.contains("ended badly"),
        "a cap is not a failure: {report}"
    );
    let _ = k.child.kill();
}
