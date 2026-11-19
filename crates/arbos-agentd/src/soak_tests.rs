//! A soak check for one box: many sends and jobs at once against a pi that crashes and stalls.
//!
//! Ignored by default. Run it with
//! `cargo test -p arbos-agentd -- --ignored soak --nocapture`, and raise the load with
//! `ARBOS_SOAK_SENDS` / `ARBOS_SOAK_JOBS`.
//!
//! The test box has no Prime binary, so a message that arrives beside a busy turn meets a helper
//! that cannot be started, which is what a box out of pids looks like.
//!
//! What it holds the box to: every send is answered or refused in words, no row is left streaming, no job is
//! left queued or running, the turn and worker counters return to zero, every worker slot comes
//! back, and neither the number of child processes nor this process's memory runs away.

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use arbos_core::doc::{CommandView, UserMessageOpts};
use arbos_core::ids::{now_ms, ulid};
use serde_json::{json, Value};

use crate::status::tests::test_agent_with_pi;
use crate::sync::SharedHost;

const CHAT: &str = "01TESTCHATSOAK000000000001";

/// One binary for the main child and every worker. Prompts are numbered across all of them, in
/// a file beside the script, and the number decides what happens: the main child dies mid-turn
/// on every seventh and is slow on every fifth; a worker dies on one in four and is slow on one
/// in three.
const STUB_PI: &str = r#"#!/usr/bin/env python3
import fcntl, json, os, sys, time

here = os.path.dirname(os.path.abspath(__file__))
worker = os.environ.get("ARBOS_SESSION_ROLE") == "subagent"

def out(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()

def next_prompt():
    with open(os.path.join(here, "prompts"), "a+") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.seek(0)
        n = int(f.read().strip() or 0) + 1
        f.seek(0)
        f.truncate()
        f.write(str(n))
        return n

for line in sys.stdin:
    try:
        c = json.loads(line)
    except ValueError:
        continue
    t = c.get("type")
    r = {"type": "response", "id": c.get("id"), "command": t, "success": True}
    if t == "get_state":
        r["data"] = {"isStreaming": False}
    out(r)
    if t == "get_state":
        path = os.environ.get("ARBOS_AUDIT_PATH")
        if path:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "a") as f:
                f.write(json.dumps({"kind": "extension_loaded", "ts": int(time.time() * 1000)}) + "\n")
    if t != "prompt":
        continue
    n = next_prompt()
    out({"type": "agent_start"})
    if (worker and n % 4 == 1) or (not worker and n % 7 == 3):
        sys.exit(9)
    if n % (3 if worker else 5) == 0:
        time.sleep(0.4)
    out({"type": "message_update",
         "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "soak answer %d" % n}})
    out({"type": "agent_settled", "aborted": False})
"#;

fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Processes running the stub, and this process's resident size in KiB.
fn sample(stub: &str) -> (usize, u64) {
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let children = run("pgrep", &["-f", stub]).lines().count();
    let rss = run("ps", &["-o", "rss=", "-p", &std::process::id().to_string()])
        .trim()
        .parse()
        .unwrap_or(0);
    (children, rss)
}

async fn user_message(host: &SharedHost, text: &str) -> String {
    let id = ulid();
    host.lock()
        .await
        .doc
        .append_user_message(&id, text, "dev-mac", &UserMessageOpts::default())
        .unwrap();
    id
}

/// What is still owed: sends with neither an answer nor a failure line the user can read, rows
/// still streaming, jobs still open.
async fn outstanding(
    agent: &crate::agent::Agent,
    host: &SharedHost,
    sends: &[String],
    jobs: &[String],
) -> Vec<String> {
    let mut owed = Vec::new();
    let messages = host.lock().await.doc.messages();
    for ask in sends {
        let answered = messages.iter().any(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(Value::as_str) == Some(ask.as_str())
                && m.rest.get("hidden").and_then(Value::as_bool) != Some(true)
                && (!m.text().trim().is_empty()
                    || m.parts.iter().any(|p| {
                        p.get("kind").and_then(Value::as_str) == Some("error")
                            && p.get("message")
                                .and_then(Value::as_str)
                                .is_some_and(|t| !t.is_empty())
                    }))
        });
        if !answered {
            let replies: Vec<String> = messages
                .iter()
                .filter(|m| m.rest.get("inReplyTo").and_then(Value::as_str) == Some(ask.as_str()))
                .map(|m| format!("{} {} {:?}", m.id, m.status, m.parts))
                .collect();
            owed.push(format!("send {ask} has no answer: {replies:?}"));
        }
    }
    for m in messages
        .iter()
        .filter(|m| m.role == "assistant" && m.status == "streaming")
    {
        owed.push(format!(
            "row {} is still streaming: {} {:?}",
            m.id,
            serde_json::to_string(&m.rest).unwrap_or_default(),
            m.parts
        ));
    }
    for id in jobs {
        let state = agent
            .store
            .job(id)
            .ok()
            .flatten()
            .map(|r| r.state)
            .unwrap_or_default();
        if state == "queued" || state == "running" {
            owed.push(format!("job {id} is still {state}"));
        }
    }
    if agent.turn_running.load(Ordering::SeqCst) {
        owed.push("a main turn is still running".into());
    }
    if agent.subagent_turns.load(Ordering::SeqCst) > 0 {
        owed.push("a worker turn is still running".into());
    }
    if agent.subagent_slots.available_permits() != crate::agent::SUBAGENT_MAX_CONCURRENT {
        owed.push("a worker slot was not given back".into());
    }
    owed
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "soak: run on purpose with --ignored"]
async fn soak_one_box_under_concurrent_turns_and_jobs() {
    let sends_n = env_count("ARBOS_SOAK_SENDS", 24);
    let jobs_n = env_count("ARBOS_SOAK_JOBS", 10);
    let dir = std::env::temp_dir().join(format!("arbos-soak-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let pi_bin = dir.join("soak-stub-pi.py");
    std::fs::write(&pi_bin, STUB_PI).unwrap();
    std::fs::set_permissions(&pi_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stub = pi_bin.display().to_string();

    let (agent, root) = test_agent_with_pi(&stub).await;
    std::fs::write(
        agent.cfg.state_dir.join(crate::config::PI_BACKOFF_MS_FILE),
        "40",
    )
    .unwrap();
    agent
        .store
        .upsert_chat(CHAT, "soak", false, now_ms())
        .unwrap();
    let host = agent.sync.open_room(CHAT).await.unwrap();
    agent.start_pi(CHAT).await.expect("the main child boots");
    let (_, rss_before) = sample(&stub);

    let mut sends = Vec::new();
    let mut jobs = Vec::new();
    let mut refused = 0;
    let mut tasks = Vec::new();
    for i in 0..sends_n.max(jobs_n) {
        if i < sends_n {
            let ask = user_message(&host, &format!("soak question {i}")).await;
            let cmd = CommandView {
                id: ulid(),
                kind: "send".into(),
                payload: json!({ "messageId": ask }),
                issued_by: "dev-mac".into(),
                issued_at: now_ms(),
                status: "pending".into(),
            };
            sends.push(ask);
            let a = agent.clone();
            tasks.push(tokio::spawn(async move {
                let _ = a.handle_command(CHAT, cmd).await;
            }));
        }
        if i < jobs_n {
            let ask = user_message(&host, &format!("soak job {i}")).await;
            match agent
                .start_job(
                    CHAT,
                    &host,
                    &ask,
                    &format!("Soak job {i}"),
                    "Count the licence lines and report the number.",
                    "other",
                    vec![],
                    true,
                    &mut None,
                )
                .await
            {
                Ok(v) => jobs.push(v["job_id"].as_str().unwrap().to_owned()),
                // Over the caps a job is refused in words, which is an answer.
                Err(why) => {
                    assert!(!why.trim().is_empty(), "a job was refused without a reason");
                    refused += 1;
                }
            }
        }
    }

    let started = Instant::now();
    let mut peak_children = 0;
    let mut owed = vec!["not checked".to_owned()];
    while started.elapsed() < Duration::from_secs(120) {
        peak_children = peak_children.max(sample(&stub).0);
        if tasks.iter().all(|t| t.is_finished()) {
            owed = outstanding(&agent, &host, &sends, &jobs).await;
            if owed.is_empty() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (_, rss_after) = sample(&stub);
    agent.shutdown().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (left_behind, _) = sample(&stub);
    eprintln!(
        "soak: {} sends, {} jobs ({refused} refused) settled in {:?}; peak children {peak_children}, \
         left after shutdown {left_behind}, rss {rss_before} -> {rss_after} KiB",
        sends.len(),
        jobs.len(),
        started.elapsed(),
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);

    assert!(owed.is_empty(), "still owed after the soak: {owed:#?}");
    // The main child and its replacement, and for each job the box took, its worker and the one
    // fresh worker a failed job gets. Finished workers stay until the box's own retirement pass,
    // so this is a bound on the work admitted and not on what runs at once; what it rules out is
    // a count that grows with the number of messages.
    let children_max = 2 + 2 * jobs.len();
    assert!(
        peak_children <= children_max,
        "{peak_children} child processes at once, over the {children_max} this load can account for"
    );
    assert_eq!(left_behind, 0, "children outlived the box's shutdown");
    assert!(
        rss_after.saturating_sub(rss_before) < 256 * 1024,
        "memory grew from {rss_before} to {rss_after} KiB"
    );
}
