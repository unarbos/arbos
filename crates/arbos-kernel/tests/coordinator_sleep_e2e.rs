//! Jacob's desktop feedback, 2026-09-17 (twice): "Never get response from
//! the 3 subagents." The coordinator spawned three workers, wrote the
//! right thought ("end this turn and let their completion reports come
//! through"), then ran `sleep 75; echo waited` anyway. All three reports
//! had landed before the sleep began and sat in its inbox behind the
//! running turn while the person watched *Waiting on three sorting
//! workers*. Driven here: a bare sleep with workers running is refused
//! with the right move, and an attached command that is not a sleep
//! yields when a worker's report lands.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

fn transcript(place: &std::path::Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(format!(".arbos/agents/{agent}/transcript.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn coordinator_place(place: &std::path::Path) {
    std::fs::create_dir_all(place.join(".arbos")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"coordinator\"\n",
    )
    .unwrap();
}

#[test]
fn a_coordinators_bare_sleep_while_its_workers_run_is_refused_with_the_right_move() {
    let replies = concat!(
        // Spawn a worker that takes a while, then try to wait with sleep.
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"quicksort\",\"task\":\"Do: bash `sleep 6; echo sorted`. Report sorted.\"}}]}\n",
        "{\"agent\":\"quicksort\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 6; echo sorted\",\"description\":\"Sort\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"waiting\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 75; echo waited\",\"description\":\"Give the worker time\",\"wait_ms\":120000}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Ending the turn; the report will wake me.\"}\n",
        "{\"agent\":\"quicksort\",\"content\":\"sorted\"}\n",
        "{\"agent\":\"root\",\"content\":\"quicksort reports: sorted.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("coordinator-sleep", replies, "", coordinator_place);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let asked = std::time::Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"sort it three ways","attachments":[]}));
    // Root's turn ends long before 75 seconds: the sleep was refused.
    let refused = a
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
                && f["event"]["ended"].is_number()
        })
        .expect("the bash call ends");
    let err = refused["event"]["error"].as_str().unwrap_or("");
    assert!(err.contains("refused"), "the sleep is refused: {refused}");
    assert!(err.contains("1 worker(s) of yours run"), "{err}");
    assert!(err.contains("End the turn now"), "{err}");
    assert!(
        asked.elapsed() < Duration::from_secs(15),
        "refused at once, not after the sleep: {:?}",
        asked.elapsed()
    );
    // The report then reaches root in its own turn.
    a.wait(Duration::from_secs(40), |f| {
        f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("quicksort reports"))
    })
    .expect("the worker's report is answered");
    let _ = k.child.kill();
}

#[test]
fn a_coordinators_attached_command_yields_when_a_workers_report_lands() {
    let replies = concat!(
        // A worker that finishes in two seconds; the coordinator then runs
        // a long wait the guard cannot read (a sleep inside an
        // interpreter) — the yield's job, not the refusal's.
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"quick\",\"task\":\"Do: bash `sleep 2; echo done`. Report done.\"}}]}\n",
        "{\"agent\":\"quick\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 2; echo done\",\"description\":\"Quick\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"waiting\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"python3 -c 'import time\\nfor i in range(60):\\n  print(i, flush=True); time.sleep(1)'\",\"description\":\"Wait\",\"wait_ms\":120000}}]}\n",
        "{\"agent\":\"quick\",\"content\":\"done\"}\n",
        "{\"agent\":\"root\",\"content\":\"quick reports: done.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("coordinator-yield", replies, "", coordinator_place);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let asked = std::time::Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do it","attachments":[]}));
    let ended = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "tool"
                && f["event"]["name"] == "bash"
                && f["event"]["ended"].is_number()
        })
        .expect("the poll yields");
    let body = ended["event"]["body"].as_str().unwrap_or("");
    assert!(
        body.contains("A worker's report landed while it ran"),
        "the yield says why: {body}"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(20),
        "yielded within seconds of the report, not after the poll: {:?}",
        asked.elapsed()
    );
    a.wait(Duration::from_secs(40), |f| {
        f["type"] == "event"
            && f["agent"] == "root"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("quick reports"))
    })
    .expect("the report is read and answered");
    let root = transcript(&k.place, "root");
    assert!(
        root.iter().any(|e| e["kind"] == "say"),
        "the report is on the record: {root:#?}"
    );
    let _ = k.child.kill();
}

/// QA (`co-*`): the first guard read the literal fragment, and these all
/// ran with a worker live. Each is a sleep in a coat, and each is refused.
#[test]
fn a_sleep_in_a_coat_is_refused_the_same() {
    let spellings = [
        "sh -c 'sleep 8'",
        "/bin/sleep 8",
        "timeout 20 sleep 8",
        "true && sleep 8",
        "while :; do sleep 1; done",
    ];
    let mut replies = String::from(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"slow\",\"task\":\"Do: bash `sleep 20; echo done`. Report done.\"}}]}\n{\"agent\":\"slow\",\"content\":\"working\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 20; echo done\",\"description\":\"Slow\"}}]}\n",
    );
    for cmd in spellings {
        let call = serde_json::json!({"agent":"root","content":"waiting","calls":[{"name":"bash","arguments":{"command":cmd,"description":"Wait","wait_ms":120000}}]});
        replies.push_str(&call.to_string());
        replies.push('\n');
    }
    replies.push_str("{\"agent\":\"root\",\"content\":\"All refused; ending the turn.\"}\n");
    let mut k =
        start_kernel_replay_prepared("coordinator-sleep-coats", &replies, "", coordinator_place);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let asked = std::time::Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"do it","attachments":[]}));
    let mut seen = 0;
    while seen < spellings.len() {
        let f = a
            .wait(Duration::from_secs(20), |f| {
                f["type"] == "event"
                    && f["agent"] == "root"
                    && f["event"]["kind"] == "tool"
                    && f["event"]["name"] == "bash"
                    && f["event"]["ended"].is_number()
            })
            .expect("each bash call ends");
        let err = f["event"]["error"].as_str().unwrap_or("");
        assert!(
            err.contains("refused") && err.contains("worker(s) of yours run"),
            "{:?} ran: {f}",
            f["event"]["args"]["command"]
        );
        seen += 1;
    }
    assert!(
        asked.elapsed() < Duration::from_secs(15),
        "all five refused at once: {:?}",
        asked.elapsed()
    );
    let _ = k.child.kill();
}
