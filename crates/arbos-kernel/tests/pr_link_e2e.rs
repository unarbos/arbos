//! Cold-track cycle 11 (cold-p5): a worker reported "A pull request has
//! been opened: PR 1" with a github.com URL on a repository that has no
//! remote — invented. Now `pr create` refuses in plain words when there is
//! no remote, and a reply that links a PR no tool output or `prs.jsonl`
//! record backs gets one reminder before the turn ends.

mod common;

use common::{Attach, start_kernel_replay, start_kernel_replay_prepared};
use std::path::Path;
use std::process::Command;
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

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

const INVENTED: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"A pull request has been opened: [PR 1](https://github.com/dmaydan/cold-p5/pull/1).\"}\n",
    "{\"agent\":\"root\",\"content\":\"No pull request was opened; the change is on branch fix/greeting in this checkout only.\"}\n",
);

#[test]
fn a_pr_link_no_tool_produced_gets_one_reminder() {
    let mut k = start_kernel_replay("pr-link-invented", INVENTED);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "open a PR for the greeting fix"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let nudge = root
        .iter()
        .find(|e| e["kind"] == "nudge" && e["reason"] == "pr link not from a tool")
        .unwrap_or_else(|| panic!("one reminder about the unbacked link: {root:#?}"));
    let text = nudge["text"].as_str().unwrap_or("");
    assert!(
        text.contains("https://github.com/dmaydan/cold-p5/pull/1")
            && text.contains("no tool opened"),
        "{text}"
    );
    let answers: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(
        answers.len(),
        2,
        "the corrected reply ends the turn: {root:#?}"
    );
    assert!(answers[1].starts_with("No pull request was opened"));
    assert_eq!(
        root.iter()
            .filter(|e| e["kind"] == "nudge" && e["reason"] == "pr link not from a tool")
            .count(),
        1
    );
    let _ = k.child.kill();
}

const BACKED: &str = "{\"agent\":\"root\",\"content\":\"[PR 7](https://github.com/unarbos/demo/pull/7) is open.\"}\n";

#[test]
fn a_recorded_pr_link_is_not_questioned() {
    let mut k = start_kernel_replay_prepared("pr-link-backed", BACKED, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/prs.jsonl"),
            "{\"ts\":1,\"agent\":\"root\",\"url\":\"https://github.com/unarbos/demo/pull/7\",\"repo\":\"unarbos/demo\",\"number\":7,\"branch\":\"fix/x\"}\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "where is the PR?"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    assert!(
        !root.iter().any(|e| e["kind"] == "nudge"),
        "a PR the store records is a real one: {root:#?}"
    );
    let _ = k.child.kill();
}

const NO_REMOTE: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"opening\",\"calls\":[{\"name\":\"pr\",\"arguments\":{\"action\":\"create\",\"title\":\"Greeting fix\",\"body\":\"Says hello.\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"The fix is on branch fix/greeting in this checkout; the repository has no remote, so there is no pull request.\"}\n",
);

#[test]
fn pr_create_refuses_in_a_repository_with_no_remote() {
    let mut k = start_kernel_replay_prepared("pr-no-remote", NO_REMOTE, "", |place| {
        std::fs::create_dir_all(place).unwrap();
        git(place, &["init", "-q", "-b", "main"]);
        std::fs::write(place.join("hello.txt"), "hi\n").unwrap();
        git(place, &["add", "hello.txt"]);
        git(place, &["commit", "-q", "-m", "hello"]);
        git(place, &["checkout", "-q", "-b", "fix/greeting"]);
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "open the PR"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(wait_for(Duration::from_secs(5), || {
        transcript(&k.place, "root")
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "pr")
        .unwrap_or_else(|| panic!("the pr call: {root:#?}"));
    let err = call["error"].as_str().unwrap_or("");
    assert!(
        err.contains("no remote")
            && err.contains("fix/greeting")
            && err.contains("report it as local"),
        "the refusal names the branch and what to say: {call:#?}"
    );
    assert!(
        !k.place.join(".arbos/prs.jsonl").exists(),
        "nothing was recorded"
    );
    let _ = k.child.kill();
}
