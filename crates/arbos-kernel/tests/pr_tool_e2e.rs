//! Process parity, slice 8: Cursor's ManagePullRequest / EditPullRequestLabels
//! as the `pr` tool over `gh`. Against a `gh` stand-in on PATH that records
//! what it was asked and a local bare repository as `origin`: a created
//! PR is a draft with the template folded in and its artifact uploaded to
//! the `arbos-artifacts` branch, recorded and followed; update, comment,
//! ci, status, and labels drive the right `gh` commands.

mod common;

use common::{Attach, spawn_with_env};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

/// A `gh` that logs its argv and answers like the real one would.
const FAKE_GH: &str = r#"#!/bin/sh
echo "$@" >> "$GH_LOG"
case "$1 $2" in
  "repo view") echo '{"nameWithOwner":"unarbos/demo"}' ;;
  "pr create")
    while [ $# -gt 0 ]; do
      if [ "$1" = "--body-file" ]; then echo "BODY: $(cat "$2" | tr '\n' ' ')" >> "$GH_LOG"; fi
      shift
    done
    echo "https://github.com/unarbos/demo/pull/7" ;;
  "pr edit") echo "https://github.com/unarbos/demo/pull/7" ;;
  "pr ready") echo "ok" ;;
  "pr comment") echo "https://github.com/unarbos/demo/pull/7#issuecomment-1" ;;
  "pr close") echo "closed" ;;
  "pr checks") printf 'build\tpass\t1m\thttps://x/1\ntest\tfail\t2m\thttps://x/2\n'; exit 1 ;;
  *) echo "unexpected: $@" >&2; exit 1 ;;
esac
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-kernel-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    for sub in [
        "place/.arbos/media/layout",
        "xdg/arbos",
        "home",
        "bin",
        "origin.git",
    ] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    dir
}

fn transcript(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn pr_create_is_a_draft_with_the_template_and_uploaded_artifacts_then_followed() {
    let dir = scratch("pr-tool");
    let place = dir.join("place");
    let gh_log = dir.join("gh.log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let gh = dir.join("bin/gh");
        std::fs::write(&gh, FAKE_GH).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // The project: a repo with a template, a still in the store, and a
    // local bare origin the artifacts branch is pushed to.
    std::fs::create_dir_all(place.join(".github")).unwrap();
    std::fs::write(
        place.join(".github/PULL_REQUEST_TEMPLATE.md"),
        "## Summary\n\n## Test plan\n",
    )
    .unwrap();
    std::fs::write(place.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
    std::fs::write(place.join(".arbos/media/layout/panel.png"), b"\x89PNG fake").unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"coordinator\"\n",
    )
    .unwrap();
    git(&place, &["init", "-q", "-b", "main"]);
    git(&place, &["add", "."]);
    git(&place, &["commit", "-q", "-m", "start"]);
    git(&dir.join("origin.git"), &["init", "-q", "--bare"]);
    git(
        &place,
        &[
            "remote",
            "add",
            "origin",
            dir.join("origin.git").to_str().unwrap(),
        ],
    );
    git(&place, &["push", "-q", "origin", "main"]);

    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"opening\",\"calls\":[{\"name\":\"pr\",\"arguments\":{\"action\":\"create\",\"title\":\"Add the panel\",\"body\":\"The panel is up.\\n\\n![panel](media/layout/panel.png)\",\"branch\":\"feature/panel\",\"base\":\"main\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"managing\",\"calls\":[",
        "{\"name\":\"pr\",\"arguments\":{\"action\":\"update\",\"pr\":\"7\",\"title\":\"Add the Project panel\",\"draft\":false}},",
        "{\"name\":\"pr\",\"arguments\":{\"action\":\"comment\",\"pr\":\"7\",\"body\":\"Ready for a look.\"}},",
        "{\"name\":\"pr\",\"arguments\":{\"action\":\"ci\",\"pr\":\"https://github.com/unarbos/demo/pull/7\"}},",
        "{\"name\":\"pr\",\"arguments\":{\"action\":\"labels\",\"pr\":\"7\",\"add\":[\"desktop\",\"parity\"],\"remove\":[\"wip\"]}},",
        "{\"name\":\"pr\",\"arguments\":{\"action\":\"status\",\"pr\":\"7\",\"status\":\"closed\"}},",
        "{\"name\":\"pr\",\"arguments\":{\"action\":\"template\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"PR handled.\"}\n",
    );
    std::fs::write(dir.join("replies.jsonl"), replies).unwrap();
    let replies_path = dir.join("replies.jsonl").display().to_string();
    let path_env = format!(
        "{}:{}",
        dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut k = spawn_with_env(
        dir.clone(),
        &["--provider", "replay", "--replies", &replies_path],
        &[("PATH", &path_env), ("GH_LOG", gh_log.to_str().unwrap())],
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "open the PR and manage it"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let _ = k.child.kill();

    let root = transcript(&place);
    let prs: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "pr")
        .collect();
    assert_eq!(prs.len(), 7, "{root:?}");
    for p in &prs {
        assert!(p.get("error").is_none(), "{p:#?}");
    }
    let created = prs[0]["body"].as_str().unwrap();
    assert!(
        created.starts_with("Opened draft Add the panel: https://github.com/unarbos/demo/pull/7"),
        "{created}"
    );
    assert!(
        created.contains("Uploaded 1 artifact(s) to arbos-artifacts"),
        "{created}"
    );
    assert!(created.contains("It is followed"), "{created}");

    let log = std::fs::read_to_string(&gh_log).unwrap();
    // create: draft, head, base, body file.
    let create_line = log.lines().find(|l| l.starts_with("pr create")).unwrap();
    assert!(
        create_line.contains("--draft")
            && create_line.contains("--head feature/panel")
            && create_line.contains("--base main"),
        "{create_line}"
    );
    assert!(
        create_line.contains("--title Add the panel"),
        "{create_line}"
    );
    // The artifact branch exists on origin with the file; its raw URL
    // replaced the local path in the body the PR got.
    let out = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "arbos-artifacts"])
        .current_dir(dir.join("origin.git"))
        .output()
        .unwrap();
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(
        tree.contains("root/") && tree.trim_end().ends_with("/panel.png"),
        "{tree}"
    );
    let body_line = log.lines().find(|l| l.starts_with("BODY: ")).unwrap();
    assert!(
        body_line.contains(
            "![panel](https://raw.githubusercontent.com/unarbos/demo/arbos-artifacts/root/"
        ),
        "the local path became the raw URL: {body_line}"
    );
    assert!(
        !body_line.contains("](media/layout/panel.png)"),
        "{body_line}"
    );
    assert!(
        body_line.contains("## Summary") && body_line.contains("## Test plan"),
        "the template rides along: {body_line}"
    );
    // update: edit + ready; comment; checks; labels; close.
    assert!(
        log.contains("pr edit 7 --title Add the Project panel"),
        "{log}"
    );
    assert!(log.contains("pr ready 7"), "{log}");
    assert!(
        log.contains("pr comment 7 --body Ready for a look."),
        "{log}"
    );
    assert!(
        log.contains("pr checks https://github.com/unarbos/demo/pull/7"),
        "{log}"
    );
    assert!(
        log.contains("pr edit 7 --add-label desktop,parity --remove-label wip"),
        "{log}"
    );
    assert!(log.contains("pr close 7"), "{log}");
    let ci = prs[3]["body"].as_str().unwrap();
    assert!(
        ci.starts_with("PR https://github.com/unarbos/demo/pull/7: a check failed"),
        "{ci}"
    );
    assert!(ci.contains("test\tfail"), "{ci}");
    let tpl = prs[6]["body"].as_str().unwrap();
    assert!(tpl.contains("## Summary"), "{tpl}");

    // Recorded and followed: prs.jsonl has it; root has both subscriptions.
    let recs = arbos_core::load_prs(&arbos_core::Place::new(&place));
    assert_eq!(recs.len(), 1, "{recs:?}");
    assert_eq!(recs[0].url, "https://github.com/unarbos/demo/pull/7");
    assert_eq!(recs[0].agent, "root");
    assert_eq!(recs[0].branch, "feature/panel");
    let subs = arbos_core::subscription::list(&arbos_core::Place::new(&place), "root");
    let kinds: Vec<&str> = subs
        .iter()
        .filter(|s| s.pr == Some(7))
        .map(|s| s.kind.as_str())
        .collect();
    assert!(
        kinds.contains(&"github_pr") && kinds.contains(&"github_ci"),
        "{subs:?}"
    );
}
