//! Process parity, slice 1: Cursor's `SendToAgent(title, rename)`. A
//! coordinator steers a running worker with a `title`: the title becomes
//! the worker's live line at once and is kept in that turn's record; a
//! queued message's title is the live line of the turn it opens; `rename`
//! gives the worker a new durable name.

mod common;

use common::{Attach, start_kernel_replay_prepared};
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
        std::thread::sleep(Duration::from_millis(200));
    }
    ok()
}

fn status_step(place: &Path, agent: &str) -> Option<String> {
    let text = std::fs::read_to_string(place.join(".arbos/agents").join(agent).join("status.toml"))
        .ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("step").and_then(|s| s.as_str()).map(str::to_string)
}

#[test]
fn a_steer_with_a_title_labels_the_running_turn_and_rename_renames_the_worker() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Draft the river poem\",\"task\":\"write a poem\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"draft-the-river-poem\",\"content\":\"drafting\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 12\",\"wait_ms\":30000}}]}\n",
        "{\"agent\":\"draft-the-river-poem\",\"content\":\"poem done\"}\n",
        "{\"agent\":\"root\",\"content\":\"steering\",\"calls\":[{\"name\":\"say\",\"arguments\":{\"to\":\"draft-the-river-poem\",\"mode\":\"steer\",\"title\":\"Make it rhyme\",\"rename\":\"Rhyme the river poem\",\"text\":\"make every couplet rhyme\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"steer sent\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted the done\"}\n",
    );
    let mut k = start_kernel_replay_prepared("say-title", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "write me a river poem"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let worker = "draft-the-river-poem";
    // The worker is mid-turn on its sleep.
    let jobs = k.place.join(".arbos/agents").join(worker).join("jobs");
    assert!(
        wait_for(Duration::from_secs(30), || {
            std::fs::read_dir(&jobs)
                .map(|rd| rd.flatten().any(|e| e.path().join("out.log").exists()))
                .unwrap_or(false)
        }),
        "the worker's job started"
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "make it rhyme"}));

    // The title is the worker's live line now, before its turn ends: the
    // frame goes out during root's turn, so it is read before root's idle.
    let status = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "status" && f["agent"] == worker && f["step"] == "Make it rhyme"
        })
        .expect("a status frame with the title reached the window");
    assert_eq!(status["source"], "title");
    assert_eq!(
        status_step(&k.place, worker).as_deref(),
        Some("Make it rhyme"),
        "status.toml holds the title"
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    // The worker was renamed, its id unchanged.
    let agent_md =
        std::fs::read_to_string(k.place.join(".arbos/agents").join(worker).join("agent.md"))
            .unwrap();
    assert!(
        agent_md.contains("name: Rhyme the river poem"),
        "renamed in agent.md: {agent_md}"
    );
    let root = transcript(&k.place, "root");
    let say = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "say")
        .expect("say tool line");
    let body = say["body"].as_str().unwrap_or("");
    assert!(body.contains("as a steer"), "{body}");
    assert!(
        body.contains("Renamed \"Draft the river poem\" to \"Rhyme the river poem\""),
        "{body}"
    );

    // The steer landed in the running turn as the parent's words.
    assert!(
        wait_for(Duration::from_secs(40), || {
            transcript(&k.place, worker)
                .iter()
                .any(|e| e["kind"] == "turn_complete")
        }),
        "the worker's turn ended"
    );
    let text = std::fs::read_to_string(
        k.place
            .join(".arbos/agents")
            .join(worker)
            .join("transcript.jsonl"),
    )
    .unwrap();
    assert!(text.contains("make every couplet rhyme"), "{text}");
    let _ = k.child.kill();
}

#[test]
fn a_queued_message_with_a_title_labels_the_turn_it_opens() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Count the colours\",\"task\":\"count\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"count-the-colours\",\"content\":\"counted\"}\n",
        "{\"agent\":\"root\",\"content\":\"queueing\",\"calls\":[{\"name\":\"say\",\"arguments\":{\"to\":\"count-the-colours\",\"mode\":\"request\",\"title\":\"Add the hex codes\",\"text\":\"now add hex codes\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"queued\"}\n",
        "{\"agent\":\"count-the-colours\",\"content\":\"adding hex codes\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sleep 6\",\"wait_ms\":30000}}]}\n",
        "{\"agent\":\"count-the-colours\",\"content\":\"hex codes added\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("say-title-queue", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "count the colours"}));
    let worker = "count-the-colours";
    // Root spawns; the worker's first turn ends; its [done] opens root's
    // turn, which queues the request with a title; the queued message
    // opens the worker's second turn, and the title is its live line from
    // the start.
    let status = a
        .wait(Duration::from_secs(40), |f| {
            f["type"] == "status" && f["agent"] == worker && f["step"] == "Add the hex codes"
        })
        .expect("the title is the live line of the turn the request opened");
    assert_eq!(status["source"], "title");
    assert!(a.wait_turn(worker, "idle", Duration::from_secs(40)));
    let turns = k.place.join(".arbos/agents").join(worker).join("turns");
    let metas: Vec<String> = std::fs::read_dir(&turns)
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("meta.toml")).ok())
        .collect();
    assert!(
        metas
            .iter()
            .any(|m| m.contains("title = \"Add the hex codes\"")),
        "the turn's meta.toml keeps the title: {metas:?}"
    );
    let _ = k.child.kill();
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
