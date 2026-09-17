//! Restart with state parked: the claims made to the self-update work
//! (an ask survives a restart; a pending approval does not, and is not
//! eaten in silence; a parent waiting on a worker picks up after) —
//! driven, not reasoned about. Each case kills the kernel with SIGKILL
//! in the parked state and starts a new one on the same place.

mod common;

use common::{Attach, restart_replay, start_kernel_replay_prepared};
use std::time::{Duration, Instant};

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

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn plain_place(place: &std::path::Path) {
    std::fs::create_dir_all(place.join(".arbos")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\nname = \"p\"\n",
    )
    .unwrap();
}

/// A question parked on the user survives the kernel: the answer given
/// to the new kernel opens the turn, once, with the question's context.
#[test]
fn a_parked_ask_survives_a_restart_and_its_answer_opens_the_turn() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one thing first\",\"calls\":[{\"name\":\"ask\",\"arguments\":{\"question\":\"Which colour for the header?\",\"options\":[\"teal\",\"plum\"]}}]}\n",
    );
    let mut k = start_kernel_replay_prepared("restart-ask", replies, "", plain_place);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"style the header","attachments":[]}));
    let ask = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the question is asked");
    let id = ask["id"].as_str().unwrap().to_string();
    assert!(
        wait_for(Duration::from_secs(10), || transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "the turn parks"
    );
    drop(a);

    // The kernel dies with the question open; a new one starts.
    let mut k2 = restart_replay(
        &mut k,
        "{\"agent\":\"root\",\"content\":\"Teal it is; the header is styled.\"}\n",
    );
    let mut b = Attach::connect(&k2.url);
    // The question is still pending: the new kernel says so on attach.
    let pending = b
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the parked question is offered again after the restart");
    assert_eq!(pending["id"], id, "{pending}");
    assert!(
        pending["question"]
            .as_str()
            .unwrap()
            .contains("Which colour")
    );
    let waiting = k2.place.join(".arbos/agents/root/waiting");
    assert!(
        std::fs::read_dir(&waiting).map(|d| d.count()).unwrap_or(0) >= 1,
        "the ask file is still there"
    );

    b.send(serde_json::json!({"type":"answer","agent":"root","text":"teal","id": id}));
    let reply = b
        .wait(Duration::from_secs(20), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Teal it is"))
        })
        .expect("the answer opens the turn on the new kernel");
    let _ = reply;
    assert!(wait_for(Duration::from_secs(10), || transcript(
        &k2.place, "root"
    )
    .iter()
    .filter(|e| e["kind"] == "turn_complete")
    .count()
        >= 2));
    let events = transcript(&k2.place, "root");
    assert_eq!(
        events.iter().filter(|e| e["kind"] == "answer").count(),
        1,
        "answered once: {events:?}"
    );
    assert_eq!(
        std::fs::read_dir(&waiting).map(|d| d.count()).unwrap_or(0),
        0,
        "nothing parked after the answer"
    );
    let _ = k2.child.kill();
}

/// A pending approval (ask mode) does not survive the kernel — and the
/// user is not left with a dead card: on the new kernel the tool call
/// that waited is on record as cut, the turn ends coherently, and the
/// next prompt runs.
#[test]
fn a_pending_approval_is_not_eaten_in_silence_across_a_restart() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"editing\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\"src/note.txt\",\"content\":\"fine\"}}]}\n",
    );
    let mut k = start_kernel_replay_prepared("restart-approve", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::create_dir_all(place.join("src")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"p\"\n\n[root]\npermission = \"ask\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"write the note","attachments":[]}),
    );
    let card = a
        .wait(Duration::from_secs(15), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("the approval card");
    assert!(
        card["question"].as_str().unwrap().contains("src/note.txt"),
        "{card}"
    );
    drop(a);
    // Nobody clicked; the kernel dies with the card up.
    std::thread::sleep(Duration::from_millis(500));
    let mut k2 = restart_replay(
        &mut k,
        "{\"agent\":\"root\",\"content\":\"The write did not happen before the restart; say if I should write the note.\"}\n",
    );
    let mut b = Attach::connect(&k2.url);
    // No approval is offered again (it belonged to a turn that is gone),
    // and the file was not written.
    let stale = b.wait(Duration::from_secs(3), |f| {
        f["type"] == "ask" && f["agent"] == "root"
    });
    assert!(
        stale.is_none(),
        "a blocking approval does not outlive its turn: {stale:?}"
    );
    assert!(
        !k2.place.join("src/note.txt").exists(),
        "nothing ran without the click"
    );
    // The continued turn ends coherently, on record.
    assert!(
        wait_for(Duration::from_secs(20), || transcript(&k2.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "the cut turn ends"
    );
    let events = transcript(&k2.place, "root");
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "wake" && e["wake"] == "serve"),
        "{events:?}"
    );
    // The record says the truth: it never ran, it was waiting for the click.
    let cut = events
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "write")
        .expect("the write is on record");
    assert!(
        cut["error"]
            .as_str()
            .unwrap()
            .starts_with("not run: the kernel restarted while this waited"),
        "{cut}"
    );
    assert!(
        cut["body"].as_str().unwrap().contains("never ran")
            && cut["body"].as_str().unwrap().contains("Nothing changed"),
        "{cut}"
    );
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    // A later prompt runs as normal.
    let mut k3 = restart_replay(
        &mut k2,
        "{\"agent\":\"root\",\"content\":\"Still here.\"}\n",
    );
    let mut c = Attach::connect(&k3.url);
    c.send(
        serde_json::json!({"type":"user","agent":"root","text":"are you there?","attachments":[]}),
    );
    assert!(
        c.wait(Duration::from_secs(20), |f| f["type"] == "event"
            && f["event"]["kind"] == "assistant"
            && f["event"]["text"] == "Still here.")
            .is_some(),
        "{kinds:?}"
    );
    let _ = k3.child.kill();
}

/// A parent blocked in `spawn wait=true` when the kernel dies: the child's
/// turn is continued by its own serve wake, the parent's cut spawn is on
/// record, and the child's report reaches the parent exactly once.
#[test]
fn a_parent_waiting_on_a_worker_picks_up_after_a_restart_and_hears_the_report_once() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"delegating\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"slow\",\"task\":\"Do: bash `sleep 8; echo built`. Report built.\",\"wait\":true}}]}\n",
        "{\"agent\":\"slow\",\"content\":\"building\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 8; echo built\",\"description\":\"Slow build\"}}]}\n",
    );
    let mut k = start_kernel_replay_prepared("restart-spawn-wait", replies, "", plain_place);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"build it and wait","attachments":[]}));
    assert!(a.wait_turn("slow", "running", Duration::from_secs(15)));
    std::thread::sleep(Duration::from_millis(1500));
    drop(a);
    // Both mid-flight: root inside spawn wait, slow inside its bash.
    let after = concat!(
        "{\"agent\":\"root\",\"content\":\"Restarted while waiting on slow; it is still building.\"}\n",
        "{\"agent\":\"slow\",\"content\":\"built\"}\n",
        "{\"agent\":\"root\",\"content\":\"slow reports: built.\"}\n",
    );
    let mut k2 = restart_replay(&mut k, after);
    let mut b = Attach::connect(&k2.url);
    let _ = b.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    assert!(
        wait_for(Duration::from_secs(40), || {
            transcript(&k2.place, "root")
                .iter()
                .any(|e| e["kind"] == "assistant" && e["text"] == "slow reports: built.")
        }),
        "the report reaches root after the restart: {:?}",
        transcript(&k2.place, "root")
    );
    let root = transcript(&k2.place, "root");
    let cut = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .expect("the spawn record");
    assert!(
        cut["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("interrupted")),
        "the cut wait is on record: {cut}"
    );
    assert_eq!(
        root.iter()
            .filter(|e| e["kind"] == "say" && e["from"] == "slow")
            .count(),
        1,
        "the report arrives once: {root:?}"
    );
    assert_eq!(
        root.iter()
            .filter(|e| e["kind"] == "wake" && e["wake"] == "done")
            .count(),
        1,
        "one done wake: {root:?}"
    );
    let slow = transcript(&k2.place, "slow");
    assert!(
        slow.iter()
            .any(|e| e["kind"] == "wake" && e["wake"] == "serve"),
        "the child continued: {slow:?}"
    );
    assert_eq!(
        slow.iter()
            .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
            .count(),
        1,
        "the build ran once: {slow:?}"
    );
    let _ = k2.child.kill();
}
