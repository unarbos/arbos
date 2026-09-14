//! T3-02: the agent that opens a pull request follows it — a `github_pr`
//! and a `github_ci` subscription appear for it — and both go when the PR
//! is merged. `gh` is a script on PATH; nothing reaches GitHub.

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
    // An old-style place: root keeps bash.
    std::fs::write(
        dir.join("place/.arbos/project.toml"),
        "schema = 2\nname = \"prs\"\n",
    )
    .unwrap();
    dir
}

/// `gh pr create` prints a PR URL; `gh pr view` answers with `state`.
fn fake_gh(dir: &Path, view_state: &str) {
    let gh = dir.join("bin/gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> {args}\ncase \"$1 $2\" in\n  \"pr create\") echo \"https://github.com/o/r/pull/12\" ;;\n  \"pr view\") echo '{{\"state\":\"{view_state}\",\"title\":\"the change\",\"headRefOid\":\"abcdef1234\",\"reviews\":[],\"comments\":[],\"statusCheckRollup\":[{{\"name\":\"ci\",\"conclusion\":\"SUCCESS\"}}]}}' ;;\n  *) echo '{{}}' ;;\nesac\n",
            args = dir.join("gh-args.txt").display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn subs(place: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(place.join(".arbos/agents/root/subscriptions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        // The kernel's own gc chore is not a subscription of interest.
        .filter(|t| !t.contains("internal = true"))
        .collect();
    out.sort();
    out
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
fn opening_a_pr_subscribes_the_agent_to_it() {
    let dir = scratch("follow-pr");
    fake_gh(&dir, "OPEN");
    std::fs::write(
        dir.join("replies.jsonl"),
        "{\"agent\":\"root\",\"content\":\"opening\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"gh pr create --base main --title t --body b --head arbos/x\"}}]}\n{\"agent\":\"root\",\"content\":\"opened\"}\n",
    )
    .unwrap();
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
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "open the PR"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(20), || subs(&k.place).len() >= 2),
        "two subscriptions follow the PR: {:?}",
        subs(&k.place)
    );
    let s = subs(&k.place);
    assert!(
        s.iter().any(|t| t.contains("kind = \"github_pr\"")
            && t.contains("pr = 12")
            && t.contains("repo = \"o/r\"")),
        "{s:?}"
    );
    assert!(
        s.iter()
            .any(|t| t.contains("kind = \"github_ci\"") && t.contains("pr = 12")),
        "{s:?}"
    );
    assert!(
        s.iter()
            .all(|t| t.contains("You opened https://github.com/o/r/pull/12")),
        "{s:?}"
    );
    let prs = std::fs::read_to_string(k.place.join(".arbos/prs.jsonl")).unwrap_or_default();
    assert!(prs.contains("\"number\":12"), "{prs}");
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert_eq!(log.matches("pr_followed").count(), 2, "{log}");
    let _ = k.child.kill();
}

#[test]
fn a_merged_pr_ends_its_subscriptions() {
    let dir = scratch("merged-pr");
    fake_gh(&dir, "MERGED");
    // A followed PR, remembered open, past due: the first look sees it
    // merged, says so, and the subscription goes.
    let seen = serde_json::json!({
        "state": "OPEN", "title": "the change", "head": "abcdef1",
        "reviews": [], "comments": 0, "last_commenter": "",
        "checks": {"ci": "SUCCESS"}
    })
    .to_string()
    .replace('"', "\\\"");
    for (id, kind) in [(1, "github_pr"), (2, "github_ci")] {
        std::fs::write(
            dir.join(format!("place/.arbos/agents/root/subscriptions/000{id}-pr.toml")),
            format!(
                "id = {id}\nkind = \"{kind}\"\nprompt = \"You opened it\"\nrepo = \"o/r\"\npr = 12\ndeliver_to = \"agent\"\ncreated = \"2026-09-10T00:00:00Z\"\nnext_due = \"2026-09-10T00:00:00Z\"\nseen = \"{seen}\"\n"
            ),
        )
        .unwrap();
    }
    std::fs::write(
        dir.join("replies.jsonl"),
        "{\"content\":\"noted\"}\n{\"content\":\"noted\"}\n",
    )
    .unwrap();
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
    assert!(
        wait_for(Duration::from_secs(40), || subs(&k.place).is_empty()),
        "both subscriptions go once the PR is merged: {:?}",
        subs(&k.place)
    );
    let t = std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default();
    assert!(
        t.contains("state: OPEN → MERGED"),
        "the agent heard about the merge: {t}"
    );
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert_eq!(log.matches("subscription_closed").count(), 2, "{log}");
    let _ = k.child.kill();
}
