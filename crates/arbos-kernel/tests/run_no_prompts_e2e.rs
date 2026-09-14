//! T3-04: `arbos-kernel run --no-prompts` never parks on a person. An
//! approval is denied, a question is told no one is here; both are named
//! on stderr and the run goes on to the turn's end.

mod common;

use common::start_kernel_replay_prepared;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn transcript(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
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

#[test]
fn run_with_no_prompts_denies_an_approval_and_answers_a_question_itself() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"needs root\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"sudo true\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"asking\",\"calls\":[{\"name\":\"ask\",\"arguments\":{\"question\":\"Which region?\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"picked a default and finished\"}\n",
    );
    let k = start_kernel_replay_prepared("run-noprompts", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"r\"\n",
        )
        .unwrap();
    });
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args(["run", "--place"])
        .arg(&k.place)
        .args([
            "--no-spawn",
            "--no-prompts",
            "--timeout",
            "60",
            "do the risky thing",
        ])
        .env("XDG_CONFIG_HOME", k.scratch.join("xdg"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "not exit 3 (waiting): {stderr}\n{stdout}"
    );
    assert!(
        stderr.contains("--no-prompts: denied:"),
        "the approval was denied and named: {stderr}"
    );
    assert!(
        stderr.contains("--no-prompts: unanswered: Which region?"),
        "the question was named: {stderr}"
    );

    // The denied tool is a tool error the model read; the question got the
    // unattended answer, which opened the last turn.
    assert!(wait_for(Duration::from_secs(30), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "picked a default and finished")
    }));
    let t = transcript(&k.place);
    let bash = t
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .expect("bash line");
    assert!(
        bash["error"].as_str().unwrap_or("").contains("denied bash"),
        "{bash:#?}"
    );
    assert!(
        t.iter().any(|e| e["kind"] == "answer"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("No one is here to answer")),
        "{t:?}"
    );
    let mut k = k;
    let _ = k.child.kill();
}
