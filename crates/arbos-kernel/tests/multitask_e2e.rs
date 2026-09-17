//! Multitasking audit (docs/multitasking-audit-2026-09-13.md), kernel items:
//! the child cap counts live workers only; one report per child, with the
//! done files of several children batched into one parent turn; a steer
//! never cancels the tool calls the user already asked for unless it says
//! stop. All on the replay provider: no key, deterministic.

mod common;

use common::{Attach, restart_replay, start_kernel_replay_prepared};
use std::{path::Path, time::Duration};

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

fn count(evs: &[serde_json::Value], kind: &str) -> usize {
    evs.iter().filter(|e| e["kind"] == kind).count()
}

fn agents(place: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(place.join(".arbos/agents"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("agent.md").exists())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

/// These tests read finished workers' folders under `.arbos/agents/`;
/// keep them there (archiving finished workers is the default).
fn start(name: &str, replies: &str, config: &str) -> common::Kernel {
    start_kernel_replay_prepared(name, replies, config, |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    })
}

fn spawn_call(name: &str, extra: &str) -> String {
    format!(
        r#"{{"name":"spawn","arguments":{{"name":"{name}","task":"say one word and stop"{extra}}}}}"#
    )
}

/// Wait until nothing has changed on any transcript for `quiet`.
fn settle(place: &Path, quiet: Duration, timeout: Duration) {
    let start = std::time::Instant::now();
    let mut last = String::new();
    let mut since = std::time::Instant::now();
    while start.elapsed() < timeout {
        let mut sig = String::new();
        for a in agents(place) {
            sig.push_str(&format!("{a}:{};", transcript(place, &a).len()));
        }
        if sig != last {
            last = sig;
            since = std::time::Instant::now();
        } else if since.elapsed() >= quiet {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Scenario 9: eight (here three, the cap set low) finished workers do not
/// stop the next spawn. Scenario 12/15: each child reports once, and every
/// done file reaches root in exactly one `done` wake (children ending
/// together share one, when the timing allows — not asserted, see below).
#[test]
fn finished_children_do_not_count_toward_the_cap_and_each_done_reaches_root_once() {
    let calls: Vec<String> = ["w1", "w2", "w3"]
        .iter()
        .map(|n| spawn_call(n, ""))
        .collect();
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"starting three\",\"calls\":[{}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"three started\"}}\n",
            "{{\"content\":\"one word\"}}\n",
            "{{\"content\":\"one word\"}}\n",
            "{{\"content\":\"one word\"}}\n",
        ),
        calls.join(",")
    );
    let mut k = start("multitask-cap", &replies, "max_children = 3\n");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "start three workers"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    settle(&k.place, Duration::from_secs(2), Duration::from_secs(40));

    let root = transcript(&k.place, "root");
    let spawned = agents(&k.place);
    assert_eq!(spawned, vec!["root", "w1", "w2", "w3"], "{spawned:?}");
    // One report per child: the done file. No `say` from the child on top.
    let says: Vec<&serde_json::Value> = root.iter().filter(|e| e["kind"] == "say").collect();
    assert_eq!(says.len(), 3, "{root:#?}");
    for w in ["w1", "w2", "w3"] {
        assert_eq!(says.iter().filter(|e| e["from"] == w).count(), 1, "{w}");
    }
    // Batching is opportunistic: a done file that lands while root is
    // still on a turn joins the next done wake; one that lands after root
    // went idle opens its own. How many of the three fall into one turn
    // depends on how close together the children finish, which a loaded
    // runner decides — so the turn count is not a fact about the kernel.
    // What batching is *for* is: every report reaches root in a `done`
    // wake, exactly once, and no turn opens without a report to carry.
    let done_wakes: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "wake" && e["wake"] == "done")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(
        (1..=3).contains(&done_wakes.len()),
        "done wakes: {done_wakes:?}\n{root:#?}"
    );
    // A report that lands while root is on a turn folds into that turn as
    // a say line (the done-wake fold) and opens no wake of its own; one
    // that lands between turns opens a wake that names it. So a child is
    // named in at most one wake, and every child's report is on the
    // transcript once (asserted above) — never twice, never missing.
    for w in ["w1", "w2", "w3"] {
        let named = done_wakes
            .iter()
            .filter(|t| {
                let (reported, _) = t.split_once(" above").unwrap_or((t, ""));
                reported.contains(w)
            })
            .count();
        assert!(
            named <= 1,
            "{w} is named in at most one wake: {done_wakes:?}"
        );
    }
    let turns = count(&root, "turn_complete");
    assert_eq!(
        turns,
        1 + done_wakes.len(),
        "the spawn turn, then one turn per done wake and no other: {turns}\n{root:#?}"
    );

    // The three are finished: the cap of three does not stop a fourth —
    // on a fresh kernel too, since what counts is on disk, not in memory.
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"one more\",\"calls\":[{}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"fourth started\"}}\n",
            "{{\"content\":\"one word\"}}\n",
        ),
        spawn_call("w4", "")
    );
    let mut k = restart_replay(&mut k, &replies);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "one more worker"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    settle(&k.place, Duration::from_secs(2), Duration::from_secs(40));
    let root = transcript(&k.place, "root");
    let refused = root.iter().any(|e| {
        e["kind"] == "tool"
            && e["name"] == "spawn"
            && e["error"]
                .as_str()
                .is_some_and(|m| m.contains("live children"))
    });
    assert!(!refused, "{root:#?}");
    assert!(
        agents(&k.place).contains(&"w4".to_string()),
        "{:?}",
        agents(&k.place)
    );
    let _ = k.child.kill();
}

/// Scenario 13: `spawn wait=true` hands the child's words back as the tool
/// result, and no done file follows for that turn.
#[test]
fn a_waited_spawn_reports_once() {
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"asking\",\"calls\":[{}]}}\n",
            "{{\"content\":\"forty-two\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"the answer is forty-two\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"this turn must not happen\"}}\n",
        ),
        spawn_call("oracle", r#","wait":true"#)
    );
    let mut k = start("multitask-wait", &replies, "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "ask the oracle"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    settle(&k.place, Duration::from_secs(3), Duration::from_secs(30));
    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("spawn tool line");
    assert!(
        spawn["body"].as_str().unwrap_or("").contains("forty-two"),
        "{spawn:#?}"
    );
    assert_eq!(count(&root, "say"), 0, "{root:#?}");
    assert_eq!(count(&root, "turn_complete"), 1, "{root:#?}");
    assert!(
        std::fs::read_dir(k.place.join(".arbos/agents/root/inbox"))
            .map(|d| d.count() == 0)
            .unwrap_or(true)
    );
    let _ = k.child.kill();
}

/// A page that takes `secs` to answer: the tool step a steer can arrive
/// during, before the model's next batch.
fn slow_server(secs: u64) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                std::thread::sleep(Duration::from_secs(secs));
                let body = "slow page";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    port
}

/// Root fetches a slow page, the steer arrives meanwhile, then root's next
/// batch is the spawn the user asked for.
fn steered_replies(port: u16, spawn: &str, last: &str) -> String {
    format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"looking\",\"calls\":[{{\"name\":\"fetch\",\"arguments\":{{\"url\":\"http://127.0.0.1:{}/slow\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"spawning\",\"calls\":[{}]}}\n",
            "{{\"content\":\"one word\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"{}\"}}\n",
            "{{\"agent\":\"root\",\"content\":\"noted\"}}\n",
        ),
        port, spawn, last
    )
}

/// Send `text` as a steer once root is running.
fn steer_when_running(a: &mut Attach, text: &str) {
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(600));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": text, "steer": true}));
}

/// Scenario 3: a steer waiting when the batch starts does not skip the
/// spawn the user asked for; it lands after it.
#[test]
fn a_steer_before_a_batch_does_not_cancel_the_spawn() {
    let port = slow_server(3);
    let replies = steered_replies(
        port,
        &spawn_call("helper", ""),
        "spawned, and noted the steer",
    );
    let mut k = start("multitask-steer", &replies, "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "look, then spawn a helper"}),
    );
    steer_when_running(&mut a, "also mention the weather");
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
    settle(&k.place, Duration::from_secs(2), Duration::from_secs(30));
    let root = transcript(&k.place, "root");
    let spawn = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("spawn tool line");
    assert!(spawn["error"].is_null(), "{spawn:#?}");
    assert!(
        !spawn["body"].as_str().unwrap_or("").contains("skipped"),
        "{spawn:#?}"
    );
    assert!(agents(&k.place).contains(&"helper".to_string()));
    // The steer landed too, on the transcript as the user's words.
    assert!(
        root.iter()
            .any(|e| e["kind"] == "user" && e["text"].as_str().unwrap_or("").contains("weather")),
        "{root:#?}"
    );
    let _ = k.child.kill();
}

/// "stop" typed at a running agent is the Stop button, not a steer: the
/// turn ends there and the spawn never starts.
#[test]
fn a_stop_word_typed_while_running_stops_the_turn() {
    let port = slow_server(3);
    let replies = steered_replies(port, &spawn_call("helper", ""), "stopped as asked");
    let mut k = start("multitask-stop-steer", &replies, "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "look, then spawn a helper"}),
    );
    steer_when_running(&mut a, "stop");
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
    common::wait_for(Duration::from_secs(5), || {
        count(&transcript(&k.place, "root"), "interrupted") >= 1
    });
    let root = transcript(&k.place, "root");
    assert!(count(&root, "interrupted") >= 1, "{root:#?}");
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "tool" && e["name"] == "spawn"),
        "{root:#?}"
    );
    assert!(!agents(&k.place).contains(&"helper".to_string()));
    let _ = k.child.kill();
}
