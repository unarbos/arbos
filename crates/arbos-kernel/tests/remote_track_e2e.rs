//! Remote track, cycle 11 (Templar, ArbosLife): a keyless kickoff must not
//! spend root's kickoff; a read-only worker is not given an Output file;
//! a one-off `bash` the user watches runs to its end whatever `wait_ms`
//! the model sent.

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
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

/// F-34: the first kickoff died before the model said anything (a fresh
/// machine with no key: the replay's "no more scripted replies" stands in
/// for it); the next kickoff runs.
#[test]
fn a_kickoff_that_failed_before_the_model_spoke_does_not_count() {
    // Two turns of transcript, written by hand: a kickoff wake that only
    // produced a failure notice, then nothing. Then the kernel starts with
    // real replies and gets `kickoff` again.
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"Hey — this place is ready: a small folder.\\nTell me what to work on.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("kickoff-retry", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/agents/root")).unwrap();
        std::fs::write(
            place.join(".arbos/agents/root/agent.md"),
            "name: root\nparent: \npaused: false\nmodel: inherit\nallowlist: ls, read, write, bash, spawn, ask, plan\nreadonly: false\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".arbos/agents/root/transcript.jsonl"),
            concat!(
                "{\"ts\":1789476000000,\"kind\":\"wake\",\"wake\":\"kickoff\",\"text\":\"This place was just opened…\"}\n",
                "{\"ts\":1789476000100,\"kind\":\"notice\",\"text\":\"No API key for OpenRouter: set OPENROUTER_API_KEY or run arbos-kernel setup\",\"failed\":true}\n",
                "{\"ts\":1789476000200,\"kind\":\"turn_complete\"}\n",
            ),
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let before = transcript(&k.place, "root").len();
    a.send(serde_json::json!({"type": "kickoff"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(40)));
    let root = transcript(&k.place, "root");
    assert!(root.len() > before, "a second kickoff turn ran: {root:#?}");
    let kickoffs = root
        .iter()
        .filter(|e| e["kind"] == "wake" && e["wake"] == "kickoff")
        .count();
    assert_eq!(kickoffs, 2);
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant"
                && e["text"].as_str().unwrap_or("").starts_with("Hey —")),
        "the real greeting came: {root:#?}"
    );
    // Now it counts: a third kickoff files nothing.
    a.send(serde_json::json!({"type": "kickoff"}));
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(
        transcript(&k.place, "root")
            .iter()
            .filter(|e| e["kind"] == "wake")
            .count(),
        2
    );
    let _ = k.child.kill();
}

/// F-36: a spawn that is read-only yet names Output files is refused
/// before the worker exists, with both ways out; without the Output line
/// it goes through.
#[test]
fn a_read_only_spawn_with_an_output_file_is_refused() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"two workers\",\"calls\":[",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"first-sentence\",\"task\":\"write a sentence about rivers\",\"output\":\".arbos/docs/rivers.md\",\"readonly\":true}},",
        "{\"name\":\"spawn\",\"arguments\":{\"name\":\"lookup\",\"task\":\"count the files\",\"readonly\":true}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
        "{\"content\":\"three files\"}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    let mut k = start_kernel_replay_prepared("readonly-output", replies, "", |place| {
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
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "two sub-agents please"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let spawns: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .collect();
    assert_eq!(spawns.len(), 2, "{root:#?}");
    let refused = spawns
        .iter()
        .find(|s| s["args"]["name"] == "first-sentence")
        .unwrap();
    let err = refused["error"].as_str().unwrap_or("");
    assert!(
        err.contains("readonly=true")
            && err.contains(".arbos/docs/rivers.md")
            && err.contains("Drop readonly"),
        "{refused:#?}"
    );
    assert!(!k.place.join(".arbos/agents/first-sentence").exists());
    let ok = spawns
        .iter()
        .find(|s| s["args"]["name"] == "lookup")
        .unwrap();
    assert!(ok.get("error").is_none(), "{ok:#?}");
    assert!(
        wait_for(Duration::from_secs(20), || k
            .place
            .join(".arbos/agents/lookup")
            .is_dir()),
        "the read-only worker without an output exists"
    );
    let _ = k.child.kill();
}

/// F-37: `bash` with a short `wait_ms` stays attached until the command
/// ends (the floor is two minutes), so the user sees every line.
#[test]
fn a_short_wait_ms_does_not_cut_a_command_the_user_is_watching() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"running\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"for i in 1 2 3 4; do echo step $i; sleep 1; done\",\"wait_ms\":500,\"description\":\"Count four steps\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"four steps\"}\n",
    );
    let mut k = start_kernel_replay_prepared("bash-floor", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"b\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "run it and show me the output as it arrives"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let root = transcript(&k.place, "root");
    let bash = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .expect("the bash record");
    let body = bash["body"].as_str().unwrap_or("");
    assert!(
        body.contains("step 1") && body.contains("step 4"),
        "every line, not the first: {body}"
    );
    assert!(
        !body.contains("Still running as job"),
        "not handed to a job after wait_ms: {body}"
    );
    let _ = k.child.kill();
}
