//! In-app feedback: the kernel hands over one exchange's trajectory and the
//! kernel log for its span in a form a client can send as it is —
//! credentials redacted, tool bodies budgeted by outcome, fat arguments
//! glanced, children included, bounded — the exchange the user is looking
//! at rather than everything.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::Duration;

fn transcript(place: &std::path::Path, agent: &str) -> Vec<serde_json::Value> {
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
fn feedback_hands_over_one_exchange_redacted_budgeted_and_bounded() {
    let big = "x".repeat(20_000);
    let replies = format!(
        concat!(
            // Exchange 1, spawn-first: root spawns a worker and its turn ends;
            // the worker prints a key and writes a big file; the done wake
            // opens root's second turn, which answers. The worker's `sleep 2`
            // keeps its report behind root's first turn on a loaded runner:
            // a report landing mid-turn would fold in and open no done wake,
            // and the done wake inside the exchange is what this asserts.
            "{{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"checker\",\"task\":\"Do: bash `echo OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef0123456789abcdef; seq 1 500`. Then write big.txt. Report.\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"checker is on it.\"}}\n",
            "{{\"agent\":\"checker\",\"content\":\"checking\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"sleep 2; echo OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef0123456789abcdef; seq 1 500; exit 3\",\"description\":\"Env check\"}}}},{{\"name\":\"write\",\"arguments\":{{\"path\":\"big.txt\",\"content\":\"{big}\"}}}}]}}\n",
            "{{\"agent\":\"checker\",\"content\":\"The env is set; big.txt written.\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"checker reports the env is set.\"}}\n",
            // Exchange 2.
            "{{\"agent\":\"root\",\"content\":\"Second exchange, unrelated.\"}}\n",
        ),
        big = big
    );
    let mut k = start_kernel_replay("feedback-bundle", &replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"check the env","attachments":[]}),
    );
    let answer = a
        .wait(Duration::from_secs(40), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"] == "checker reports the env is set."
        })
        .expect("the done turn answers");
    let answer_seq = answer["event"]["seq"].as_u64().unwrap();
    let _ = a.wait(Duration::from_secs(10), |f| {
        f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "turn_complete"
            && f["event"]["seq"].as_u64() > Some(answer_seq)
    });
    a.send(serde_json::json!({"type":"user","agent":"root","text":"and now something else","attachments":[]}));
    let _ = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"] == "Second exchange, unrelated."
        })
        .expect("exchange 2 answers");

    // Point at the answer line of exchange 1: the whole exchange comes —
    // Jacob's words, the spawn, the done wake, the answer — as one.
    a.send(serde_json::json!({"type":"feedback","agent":"root","seq": answer_seq, "note":"it printed my key sk-or-v1-0123456789abcdef0123456789abcdef"}));
    let b = a
        .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
        .expect("the bundle");
    let events = b["events"].as_array().unwrap();
    assert_eq!(events[0]["kind"], "wake");
    assert_eq!(events[0]["wake"], "user");
    assert_eq!(events[1]["text"], "check the env");
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "wake" && e["wake"] == "done"),
        "the done wake is inside: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "tool" && e["name"] == "spawn")
    );
    assert!(
        events
            .iter()
            .any(|e| e["text"] == "checker reports the env is set."),
        "the answer is inside"
    );
    assert!(
        events.iter().all(|e| e["text"] != "and now something else"),
        "only this exchange"
    );
    assert!(b["turn"]["complete"].as_bool().unwrap());

    // The child rides along, slimmed the same way: the failed bash gets
    // its error and a tail-weighted body; the write's file is glanced with
    // its true length; the key is gone everywhere.
    let children = b["children"].as_array().unwrap();
    assert_eq!(children.len(), 1, "{children:?}");
    assert_eq!(children[0]["agent"], "checker");
    let theirs = children[0]["events"].as_array().unwrap();
    let bash = theirs
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .expect("the child's bash");
    let out = bash["output"].as_str().unwrap();
    assert!(
        out.contains("500") && out.contains("exit 3"),
        "the end is there: {out}"
    );
    assert!(bash.get("body").is_none());
    assert!(
        bash["result_size"].is_number(),
        "the true size stays: {bash}"
    );
    let write = theirs
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "write")
        .expect("the child's write");
    assert!(
        write["args"]["content"].as_str().unwrap().len() < 500,
        "the file is glanced"
    );
    assert_eq!(
        write["args"]["content"]
            .as_str()
            .unwrap()
            .matches('…')
            .count(),
        1
    );
    assert_eq!(write["args_clipped"]["content"], 20_000);
    assert_eq!(write["args"]["path"], "big.txt");
    let text = b.to_string();
    assert!(!text.contains("sk-or-v1-0123"), "{text}");
    assert!(
        b["redacted"]["tokens"].as_u64().unwrap() >= 1,
        "{}",
        b["redacted"]
    );
    assert!(
        b["note"]
            .as_str()
            .unwrap()
            .starts_with("it printed my key [redacted:"),
        "{}",
        b["note"]
    );
    let kernel = &b["kernel"];
    assert_eq!(kernel["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        kernel["git_sha"].is_string() && kernel["built_at"].is_string() && kernel["os"].is_string()
    );
    let log = b["log"].as_array().unwrap();
    assert!(
        !log.is_empty() && log.iter().all(|l| l["ts"].is_number()),
        "{log:?}"
    );
    assert!(
        log.iter().any(|l| l["agent"] == "checker"),
        "the child's log lines too: {log:?}"
    );
    assert_eq!(b["truncated"], false);
    let claimed = b["bytes"].as_u64().unwrap();
    assert!(claimed > 1000 && claimed < 1024 * 1024, "{claimed}");
    assert!(
        (b.to_string().len() as i64 - claimed as i64).abs() < 64,
        "bytes is the frame's size: {} vs {claimed}",
        b.to_string().len()
    );

    // A call id: its exchange, and its body whole.
    let child_bash_id = transcript(&k.place, "checker")
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .map(|e| e["call_id"].as_str().unwrap().to_string())
        .unwrap();
    let root_spawn_id = transcript(&k.place, "root")
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .map(|e| e["call_id"].as_str().unwrap().to_string())
        .unwrap();
    a.send(serde_json::json!({"type":"feedback","agent":"root","call_id": root_spawn_id}));
    let by_call = a
        .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
        .expect("by call id");
    assert_eq!(
        by_call["events"][1]["text"], "check the env",
        "the call's exchange"
    );
    assert_eq!(by_call["turn"]["call_id"], root_spawn_id);

    // `tail: 40` from exchange 2: the last lines of the transcript
    // whatever exchange they fall in, minus what the anchor already
    // carries; the wake lines show the turn structure.
    a.send(serde_json::json!({"type":"feedback","agent":"root","tail": 40}));
    let with_tail = a
        .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
        .expect("with a tail");
    assert_eq!(
        with_tail["events"][1]["text"], "and now something else",
        "no seq: the last user exchange"
    );
    let tail = with_tail["tail"].as_array().unwrap();
    assert!(!tail.is_empty());
    assert!(
        tail.iter()
            .any(|e| e["kind"] == "wake" && e["wake"] == "user" && e["text"] == "check the env"),
        "{tail:?}"
    );
    assert!(
        tail.iter()
            .any(|e| e["kind"] == "wake" && e["wake"] == "done"),
        "{tail:?}"
    );
    assert!(
        tail.iter().all(|e| e["text"] != "and now something else"),
        "the anchor's lines are not repeated: {tail:?}"
    );
    assert!(
        tail.iter()
            .filter(|e| e["kind"] == "tool")
            .all(|e| e.get("body").is_none()),
        "slimmed the same"
    );
    assert_eq!(with_tail["turn"]["tail"], 40);

    // The companion: one body whole, redacted.
    a.send(serde_json::json!({"type":"tool_body","agent":"checker","call_id": child_bash_id}));
    let body = a
        .wait(Duration::from_secs(10), |f| f["type"] == "tool_body_reply")
        .expect("the body");
    let text = body["body"].as_str().unwrap();
    assert!(
        text.contains("500") && text.contains("[redacted:"),
        "{text}"
    );
    assert_eq!(body["truncated"], false);
    assert!(body["size"].as_u64().unwrap() > 1000);

    // Unknown agent, unknown call: errors, not silence.
    a.send(serde_json::json!({"type":"feedback","agent":"nobody"}));
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "error"
            && f["detail"].as_str().unwrap().contains("nobody"))
            .is_some()
    );
    a.send(serde_json::json!({"type":"tool_body","agent":"root","call_id":"nope"}));
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "error"
            && f["detail"].as_str().unwrap().contains("nope"))
            .is_some()
    );
    let _ = k.child.kill();
}

/// The smoke report's complaint, "the worker line says Starting forever",
/// could not be diagnosed from the first bundle: no roster, no key state,
/// no inbox. Built here for real — a worker spawned, then the key gone,
/// so its brief waits — the bundle must let a reader see the cause: the
/// worker exists, is not running, has a waking brief in its inbox, and
/// the place has no key.
#[test]
fn a_bundle_lets_a_reader_diagnose_a_worker_that_never_starts() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"slow starter\",\"task\":\"Do: bash `sleep 20`. Report done.\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started slow starter\"}\n",
        "{\"agent\":\"slow-starter\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 20\",\"description\":\"Long step\"}}]}\n",
    );
    let mut k = start_kernel_replay("feedback-roster", &replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"start a slow worker","attachments":[]}));
    assert!(a.wait_turn("slow-starter", "running", Duration::from_secs(20)));
    // Root's own turn ends (it spawned and said so); the worker runs on.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while transcript(&k.place, "root")
        .iter()
        .all(|e| e["kind"] != "turn_complete")
    {
        assert!(std::time::Instant::now() < deadline, "root's turn ends");
        std::thread::sleep(Duration::from_millis(50));
    }
    // The worker is mid-turn: the bundle's roster says so, from the
    // kernel's own state. That state follows the disk by a moment — the
    // turn writes `turn_complete`, then the serve loop marks the agent
    // not running — so the bundle is asked again until root reads idle
    // (CI on `main`, 2026-09-17: root still `running` in the first
    // bundle taken right after the line landed).
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let b = loop {
        a.send(serde_json::json!({"type":"feedback","agent":"root","note":"the worker line says Starting forever"}));
        let b = a
            .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
            .expect("the bundle");
        let root_running = b["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == "root")
            .map(|r| r["running"] == true)
            .unwrap_or(true);
        if !root_running {
            break b;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "root reads idle once the serve loop has caught up: {b}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    let agents = b["agents"].as_array().unwrap();
    let worker = agents
        .iter()
        .find(|x| x["id"] == "slow-starter")
        .expect("the worker is on the roster: {agents:?}");
    assert_eq!(worker["parent"], "root");
    assert_eq!(worker["running"], true, "{worker}");
    assert_eq!(worker["archived"], false);
    assert!(
        worker["transcript_lines"].as_u64().unwrap() >= 1,
        "{worker}"
    );
    let root = agents.iter().find(|x| x["id"] == "root").unwrap();
    assert_eq!(root["running"], false);
    assert_eq!(root["pending_asks"], 0);
    let place = &b["place"];
    assert_eq!(place["key"], true, "{place}");
    assert!(place["keyless_reason"].is_null());
    assert!(
        place.get("permission").is_some()
            && place["spend"].is_object()
            && place.get("window_tokens").is_some(),
        "{place}"
    );
    assert_eq!(place["archive_children"], true);
    assert!(place["max_children"].as_u64().unwrap() >= 1);
    let _ = k.child.kill();
}
