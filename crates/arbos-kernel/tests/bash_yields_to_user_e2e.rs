//! Jacob's Mac, 2026-09-16: a worker sat in `python3 bubble_sort.py` for
//! 2m 26s; he typed "run it" four times; four bubbles, no reply, and the
//! window said "nothing has arrived". The kernel's half, driven: an
//! attached command yields to the user's words (it keeps running as a
//! job, the turn answers now); its output streams as `job` frames while
//! it runs; and the same words a second time are not stacked.

mod common;

use common::{Attach, start_kernel_replay};
use std::time::{Duration, Instant};

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn an_attached_command_yields_to_the_users_words_streams_and_a_repeat_is_not_stacked() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"running it\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"for i in $(seq 1 40); do echo tick $i; sleep 0.5; done\",\"description\":\"Slow script\"}}]}\n",
        // After the yield: the user's words are read at this boundary.
        "{\"agent\":\"root\",\"content\":\"It is running — 20 seconds in, still going; I will report when it ends.\"}\n",
    );
    let mut k = start_kernel_replay("bash-yields", replies);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"start it","attachments":[]}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // Output streams while the tool call holds the turn.
    let job = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "job"
                && f["agent"] == "root"
                && f["delta"].as_str().is_some_and(|d| d.contains("tick"))
        })
        .expect("the attached command's output arrives as it is printed");
    assert_eq!(job["running"], true, "{job}");
    std::thread::sleep(Duration::from_millis(1500));

    // "run it" — twice, as an impatient person does.
    let asked = Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"run it","steer":true,"attachments":[]}));
    a.send(serde_json::json!({"type":"user","agent":"root","text":"run it","steer":true,"attachments":[]}));
    let reply = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "assistant"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("It is running"))
        })
        .expect("the turn answers the user while the command runs");
    assert!(
        asked.elapsed() < Duration::from_secs(8),
        "answered in {:?}, not after the command",
        asked.elapsed()
    );
    let _ = reply;
    let events = transcript(&k.place);
    // One "run it" on the transcript, one notice for the repeat.
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "user" && e["text"] == "run it")
            .count(),
        1,
        "{events:?}"
    );
    let dup = events
        .iter()
        .find(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Already queued: \"run it\""))
        })
        .expect("the repeat is acknowledged, not stacked");
    assert!(
        dup["text"].as_str().unwrap().contains("not added again"),
        "{dup}"
    );
    // The bash record says it yielded and the command runs on as a job.
    let bash = events
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .unwrap();
    let body = bash["body"].as_str().unwrap();
    assert!(body.contains("Still running as job"), "{body}");
    assert!(
        body.contains("The user said something while it ran"),
        "{body}"
    );
    assert!(
        body.contains("tick"),
        "the output so far is in the result: {body}"
    );
    let jobs = arbos_engine::JobsRoot::for_agent(
        &arbos_core::Place::new(&k.place),
        &arbos_core::AgentId::new("root"),
    );
    assert!(
        jobs.list().iter().any(|j| j.running() && j.detached()),
        "the command runs on"
    );
    let _ = k.child.kill();
}
