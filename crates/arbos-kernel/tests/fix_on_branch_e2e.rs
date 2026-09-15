//! Kickoff item 8: the bug was fixed with a commit straight onto `main`,
//! no branch, no pull request. A fix lands on its own branch and comes
//! back as a PR; the git guard refuses a commit on a protected branch and
//! says what to run, and the same command with `checkout -b` first goes
//! through.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn git(dir: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "git {args:?} in {}",
        dir.display()
    );
}

fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("hello.py"), "print(\"hello\"\n").unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "start"]);
}

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

#[test]
fn a_fix_committed_on_main_is_refused_and_lands_on_a_branch_instead() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"fixing\",\"calls\":[",
        "{\"name\":\"write\",\"arguments\":{\"path\":\"toy-repo/hello.py\",\"contents\":\"print(\\\"hello\\\")\\n\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"committing\",\"calls\":[",
        "{\"name\":\"bash\",\"arguments\":{\"command\":\"git add -A && git commit -q -m fix\",\"cwd\":\"toy-repo\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"on a branch then\",\"calls\":[",
        "{\"name\":\"bash\",\"arguments\":{\"command\":\"git checkout -q -b fix/paren && git add -A && git commit -q -m fix && git log --oneline main..HEAD\",\"cwd\":\"toy-repo\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    let mut k = start_kernel_replay_prepared("fix-on-branch", replies, "", |place| {
        make_repo(&place.join("toy-repo"));
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "fix hello.py"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let root = transcript(&k.place, "root");
    let bashes: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
        .collect();
    assert_eq!(bashes.len(), 2, "{root:#?}");
    // On main: refused before it runs, with the way out.
    let on_main = bashes[0];
    let err = on_main["error"].as_str().unwrap_or("");
    assert!(
        err.contains("on `main`, a protected branch"),
        "the commit on main is refused: {on_main:#?}"
    );
    assert!(
        err.contains("git checkout -b fix/") && err.contains("pr create"),
        "and told what to run: {err}"
    );
    // The repository's main did not move.
    let repo = k.place.join("toy-repo");
    let main_log = Command::new("git")
        .args(["log", "--oneline", "main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&main_log.stdout).lines().count(),
        1,
        "main still has only the start commit"
    );
    // On a branch: the commit goes through and main..HEAD shows it.
    let on_branch = bashes[1];
    assert!(on_branch.get("error").is_none(), "{on_branch:#?}");
    let body = on_branch["body"].as_str().unwrap_or("");
    assert!(body.contains(" fix"), "the branch has the fix: {body}");
    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "fix/paren");
    let _ = k.child.kill();
}
