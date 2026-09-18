//! qa-033: `say` to a worker that has finished and moved to the archive
//! said "no agent is named X. Agents here: (none)" — as if it had never
//! run. It now says the worker finished, when, where its report is, and
//! what to do; and an unknown name still hears which workers ran.

mod common;

use common::{Attach, start_kernel_replay};
use std::path::Path;
use std::time::{Duration, Instant};

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str(l).ok())
    .collect()
}

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

#[test]
fn a_say_to_an_archived_worker_says_it_finished_and_where_its_report_is() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"design-draft\",\"task\":\"draft the design\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        // The worker's report lands after root's dispatch turn has closed,
        // so it opens the done turn the script assumes. An instant report
        // folds into the running turn as a say and there is no second
        // turn_complete to wait for (red on main at 8196ed6e; the fold-race
        // family — #585, #606).
        "{\"content\":\"the design is drafted\",\"delay_ms\":2500}\n",
        // Root's done turn: the user's constraint arrives after the worker
        // is gone; root steers it anyway, then tries a name nobody has.
        "{\"agent\":\"root\",\"content\":\"steering\",\"calls\":[{\"name\":\"say\",\"arguments\":{\"to\":\"design-draft\",\"mode\":\"steer\",\"text\":\"also cover remote attach\"}},{\"name\":\"say\",\"arguments\":{\"to\":\"nobody-here\",\"text\":\"hello\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"understood\"}\n",
    );
    let mut k = start_kernel_replay("say-archived", replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "draft the design"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The worker finishes and is archived; root's done turn runs its says.
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count()
            >= 2),
        "root's done turn ends"
    );
    assert!(k.place.join(".arbos/archive/agents/design-draft").is_dir());
    let root = transcript(&k.place, "root");
    let says: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "say")
        .collect();
    assert_eq!(says.len(), 2, "{root:#?}");
    let steer = says
        .iter()
        .find(|s| s["args"]["to"] == "design-draft")
        .expect("the steer record");
    let err = steer["error"].as_str().unwrap_or("");
    assert!(
        err.contains("design-draft") && err.contains("finished at 20") && err.contains("archived"),
        "says it finished, when, and that it is archived: {err}"
    );
    assert!(
        err.contains(".arbos/archive/agents/design-draft/transcript.jsonl"),
        "names the report: {err}"
    );
    assert!(
        err.contains("spawn a new worker"),
        "says what to do instead: {err}"
    );
    assert!(
        !err.contains("no agent is named"),
        "never 'never existed': {err}"
    );
    let unknown = says
        .iter()
        .find(|s| s["args"]["to"] == "nobody-here")
        .expect("the unknown-name record");
    let err = unknown["error"].as_str().unwrap_or("");
    assert!(
        err.contains("no agent is named \"nobody-here\"") && err.contains("Live agents: (none)"),
        "{err}"
    );
    assert!(
        err.contains("1 archived (finished): design-draft"),
        "the empty live roster still says who ran: {err}"
    );
    let _ = k.child.kill();
}
