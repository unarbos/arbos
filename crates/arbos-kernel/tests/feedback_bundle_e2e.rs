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
            // opens root's second turn, which answers.
            "{{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"checker\",\"task\":\"Do: bash `echo OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef0123456789abcdef; seq 1 500`. Then write big.txt. Report.\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"checker is on it.\"}}\n",
            "{{\"agent\":\"checker\",\"content\":\"checking\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"echo OPENROUTER_API_KEY=sk-or-v1-0123456789abcdef0123456789abcdef; seq 1 500; exit 3\",\"description\":\"Env check\"}}}},{{\"name\":\"write\",\"arguments\":{{\"path\":\"big.txt\",\"content\":\"{big}\"}}}}]}}\n",
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

    // `turns: 2` from exchange 2: exchange 1 thinned to a line.
    a.send(serde_json::json!({"type":"feedback","agent":"root","turns": 2}));
    let two = a
        .wait(Duration::from_secs(10), |f| f["type"] == "feedback_bundle")
        .expect("two exchanges");
    assert_eq!(
        two["events"][1]["text"], "and now something else",
        "no seq: the last user exchange"
    );
    let earlier = two["earlier"].as_array().unwrap();
    assert_eq!(earlier.len(), 1);
    assert_eq!(earlier[0]["asked"], "check the env");
    assert_eq!(earlier[0]["answered"], "checker reports the env is set.");
    assert_eq!(earlier[0]["tools"]["spawn"], 1);

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
