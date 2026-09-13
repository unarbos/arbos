//! The Mac wake-up incident: an orphaned job from three days earlier kept
//! appending to `.arbos/user.md` across every kernel restart; a paused
//! agent's timer had `next_due` in the past; a kernel start fired the
//! backlog. Now: leftover jobs are reaped at start, overdue timers fire
//! once with a note (or not at all, by config), a resume reschedules, and
//! a long transcript rolls into an archive between turns.

mod common;

use common::{Attach, start_kernel_replay_prepared};
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

fn write_root_agent(place: &Path) {
    let dir = place.join(".arbos/agents/root");
    std::fs::create_dir_all(dir.join("subscriptions")).unwrap();
    std::fs::write(
        dir.join("agent.md"),
        "---\nname: root\nparent: null\npaused: false\nmodel: inherit\n---\n",
    )
    .unwrap();
}

fn overdue_timer(place: &Path, hours_ago: i64) {
    let due = arbos_core::inbox::rfc3339(arbos_core::now_ms() - hours_ago * 3_600_000);
    std::fs::write(
        place.join(".arbos/agents/root/subscriptions/0007-hourly-check.toml"),
        format!(
            "id = 7\nkind = \"timer\"\nprompt = \"hourly check\"\nevery = \"1h\"\nonce = false\ndeliver_to = \"agent\"\ncreated = \"2026-09-10T00:00:00Z\"\nnext_due = \"{due}\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn an_overdue_timer_fires_once_with_a_missed_note() {
    let mut k = start_kernel_replay_prepared(
        "catchup-once",
        "{\"agent\":\"root\",\"content\":\"caught up\"}\n{\"agent\":\"root\",\"content\":\"again?\"}\n",
        "",
        |place| {
            write_root_agent(place);
            overdue_timer(place, 5);
        },
    );
    let _a = Attach::connect(&k.url);
    // The timer fires within milliseconds of start; wait on the transcript.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline
        && !transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_secs(2));
    let evs = transcript(&k.place, "root");
    let wakes: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|e| e["kind"] == "user" || e["kind"] == "wake")
        .filter(|e| e["text"].as_str().unwrap_or("").contains("hourly check"))
        .collect();
    assert!(!wakes.is_empty(), "{evs:#?}");
    let texts: Vec<&str> = wakes.iter().map(|e| e["text"].as_str().unwrap()).collect();
    assert!(
        texts.iter().any(|t| t.contains("missed 4 earlier firing") || t.contains("missed 5 earlier firing")),
        "{texts:?}"
    );
    // One firing, not five.
    assert_eq!(
        evs.iter().filter(|e| e["kind"] == "turn_complete").count(),
        1,
        "{evs:#?}"
    );
    let sub = std::fs::read_to_string(
        k.place
            .join(".arbos/agents/root/subscriptions/0007-hourly-check.toml"),
    )
    .unwrap();
    let due = sub
        .lines()
        .find_map(|l| l.strip_prefix("next_due = "))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap();
    assert!(
        arbos_core::parse_instant_ms(&due).unwrap() > arbos_core::now_ms(),
        "next_due still in the past: {due}"
    );
    let _ = k.child.kill();
}

#[test]
fn catch_up_skip_reschedules_without_a_turn() {
    let mut k = start_kernel_replay_prepared(
        "catchup-skip",
        "{\"agent\":\"root\",\"content\":\"must not run\"}\n",
        "catch_up = \"skip\"\n",
        |place| {
            write_root_agent(place);
            overdue_timer(place, 5);
        },
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    std::thread::sleep(Duration::from_secs(4));
    let evs = transcript(&k.place, "root");
    assert_eq!(
        evs.iter().filter(|e| e["kind"] == "turn_complete").count(),
        0,
        "{evs:#?}"
    );
    let sub = std::fs::read_to_string(
        k.place
            .join(".arbos/agents/root/subscriptions/0007-hourly-check.toml"),
    )
    .unwrap();
    let due = sub
        .lines()
        .find_map(|l| l.strip_prefix("next_due = "))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap();
    assert!(
        arbos_core::parse_instant_ms(&due).unwrap() > arbos_core::now_ms(),
        "{due}"
    );
    let _ = k.child.kill();
}

#[test]
fn a_paused_agent_never_fires_and_a_resume_reschedules() {
    let mut k = start_kernel_replay_prepared(
        "paused-resume",
        "{\"agent\":\"root\",\"content\":\"must not run\"}\n",
        "",
        |place| {
            write_root_agent(place);
            std::fs::write(
                place.join(".arbos/agents/root/agent.md"),
                "---\nname: root\nparent: null\npaused: true\nmodel: inherit\n---\n",
            )
            .unwrap();
            overdue_timer(place, 5);
        },
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count(),
        0
    );
    a.send(serde_json::json!({"type": "pause", "agent": "root", "paused": false}));
    std::thread::sleep(Duration::from_secs(3));
    // Resumed: the timer is due one period from now, not fired.
    assert_eq!(
        transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count(),
        0
    );
    let sub = std::fs::read_to_string(
        k.place
            .join(".arbos/agents/root/subscriptions/0007-hourly-check.toml"),
    )
    .unwrap();
    let due = sub
        .lines()
        .find_map(|l| l.strip_prefix("next_due = "))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap();
    let ms = arbos_core::parse_instant_ms(&due).unwrap() - arbos_core::now_ms();
    assert!(ms > 50 * 60_000 && ms <= 60 * 60_000, "next in {ms} ms");
    let _ = k.child.kill();
}

#[test]
fn a_leftover_job_is_reaped_at_start_unless_kept() {
    use std::os::unix::process::CommandExt;
    let spawn_sleeper = |dir: &Path| -> std::process::Child {
        std::fs::create_dir_all(dir).unwrap();
        let child = std::process::Command::new("sh")
            .args(["-c", &format!("sleep 300 # arbos-job {}", dir.display())])
            .process_group(0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(
            dir.join("meta.json"),
            serde_json::json!({
                "command": format!("sleep 300 # arbos-job {}", dir.display()),
                "cwd": "/tmp", "pid": child.id(), "started_ms": arbos_core::now_ms()
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("out.log"), "").unwrap();
        std::fs::write(dir.join("detached"), "").unwrap();
        child
    };
    let mut children: Vec<std::process::Child> = Vec::new();
    let mut k = start_kernel_replay_prepared("reap", "", "", |place| {
        write_root_agent(place);
        let jobs = place.join(".arbos/agents/root/jobs");
        children.push(spawn_sleeper(&jobs.join("j1")));
        children.push(spawn_sleeper(&jobs.join("j2")));
        std::fs::write(jobs.join("j2").join("keep"), "").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // j1 dies within a moment of the kernel's start (a zombie until we
    // wait on it, which is what `try_wait` does); j2 keeps running.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut j1_gone = false;
    while std::time::Instant::now() < deadline {
        if children[0].try_wait().unwrap().is_some() {
            j1_gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(j1_gone, "j1 should be gone");
    assert!(
        children[1].try_wait().unwrap().is_none(),
        "j2 has a keep file and should live"
    );
    let killed =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/jobs/j1/killed")).unwrap();
    assert!(killed.contains("earlier kernel run"), "{killed}");
    assert!(!k.place.join(".arbos/agents/root/jobs/j2/killed").exists());
    let _ = children[1].kill();
    let _ = children[1].wait();
    let _ = k.child.kill();
}
