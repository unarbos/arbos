//! K-04: `github_ci` with a `branch` and no `pr` watches the branch's
//! workflow runs — the shape of a "keep main green" loop. `gh` is a script
//! here: the kernel runs whatever `gh` PATH gives it.

mod common;

use common::{Attach, spawn_with_env};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-kernel-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    for sub in [
        "place/.arbos/agents/root/subscriptions",
        "xdg/arbos",
        "home",
        "bin",
    ] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    dir
}

fn transcript_text(place: &Path) -> String {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl")).unwrap_or_default()
}

#[test]
fn a_branch_ci_subscription_wakes_the_agent_when_a_run_goes_red() {
    let dir = scratch("ci-branch");
    // `gh run list …` answers: the newest commit's ci run failed.
    let gh = dir.join("bin/gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> {args}\ncat <<'EOF'\n{json}\nEOF\n",
            args = dir.join("gh-args.txt").display(),
            json = serde_json::json!([
                {"databaseId": 3, "workflowName": "ci", "name": "ci", "status": "completed", "conclusion": "failure", "headSha": "bbbbbbb1", "url": "https://github.com/o/r/actions/runs/3"},
                {"databaseId": 2, "workflowName": "ci", "name": "ci", "status": "completed", "conclusion": "success", "headSha": "aaaaaaa1", "url": "https://github.com/o/r/actions/runs/2"}
            ])
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // A hand-written subscription, already past due, that remembers the
    // branch green on the earlier commit: the first look diffs and fires.
    let seen = serde_json::json!({
        "state": "green", "title": "main", "head": "aaaaaaa",
        "reviews": [], "comments": 0, "last_commenter": "",
        "checks": {"ci": "success"}
    })
    .to_string()
    .replace('"', "\\\"");
    std::fs::write(
        dir.join("place/.arbos/agents/root/subscriptions/0003-main-ci.toml"),
        format!(
            "id = 3\nkind = \"github_ci\"\nprompt = \"keep main green\"\nrepo = \"o/r\"\nbranch = \"main\"\ndeliver_to = \"agent\"\ncreated = \"2026-09-10T00:00:00Z\"\nnext_due = \"2026-09-10T00:00:00Z\"\nseen = \"{seen}\"\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("replies.jsonl"), "{\"content\":\"on it\"}\n").unwrap();
    let path = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let replies = dir.join("replies.jsonl").display().to_string();
    let mut k = spawn_with_env(
        dir.clone(),
        &["--provider", "replay", "--replies", &replies],
        &[("PATH", path.as_str())],
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    let start = Instant::now();
    let text = loop {
        let t = transcript_text(&k.place);
        if t.contains("turn_complete") || start.elapsed() > Duration::from_secs(40) {
            break t;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(text.contains("o/r@main (red)"), "{text}");
    assert!(
        text.contains("check ci: success → failure (https://github.com/o/r/actions/runs/3)"),
        "{text}"
    );
    assert!(text.contains("new commits: head is now bbbbbbb"), "{text}");
    assert!(text.contains("You asked: keep main green"), "{text}");
    let args = std::fs::read_to_string(dir.join("gh-args.txt")).unwrap_or_default();
    assert!(
        args.contains("run list --repo o/r --branch main"),
        "gh was asked for the branch's runs: {args}"
    );
    let _ = k.child.kill();
}
