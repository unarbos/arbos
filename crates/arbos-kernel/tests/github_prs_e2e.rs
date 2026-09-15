//! Projects-post gap 2: "follow all my PRs" was impossible — `github_pr`
//! needed one repo + pr, and nothing fired on a merge. `github_prs`
//! follows a whole repository's pull requests (optionally one author's):
//! one `[github]` message per look with what opened, merged, closed, got
//! commits, or went red. A `github_pr` written with a repo and no `pr`
//! is read as one. `gh` is a script on PATH; nothing reaches GitHub.

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
    std::fs::write(
        dir.join("place/.arbos/project.toml"),
        "schema = 2\nname = \"prs\"\n",
    )
    .unwrap();
    dir
}

/// `gh pr list` answers with three pull requests: #1 open with a red
/// check, #2 merged just now, #3 opened — and #4 merged years ago but
/// touched (in the window, not news).
fn fake_gh(dir: &Path) {
    let gh = dir.join("bin/gh");
    let list = r#"[{"number":1,"title":"Fix the parser","state":"OPEN","headRefOid":"aaaaaaa1111","author":{"login":"jacob"},"mergedAt":null,"closedAt":null,"url":"https://github.com/o/r/pull/1","statusCheckRollup":[{"name":"build","conclusion":"FAILURE"}]},{"number":2,"title":"Add the hub","state":"MERGED","headRefOid":"bbbbbbb2222","author":{"login":"jacob"},"mergedAt":"2026-09-15T18:00:00Z","closedAt":"2026-09-15T18:00:00Z","url":"https://github.com/o/r/pull/2","statusCheckRollup":[]},{"number":3,"title":"Docs pass","state":"OPEN","headRefOid":"ccccccc3333","author":{"login":"jacob"},"mergedAt":null,"closedAt":null,"url":"https://github.com/o/r/pull/3","statusCheckRollup":[]},{"number":4,"title":"Ancient","state":"MERGED","headRefOid":"ddddddd4444","author":{"login":"jacob"},"mergedAt":"2020-01-01T00:00:00Z","closedAt":"2020-01-01T00:00:00Z","url":"https://github.com/o/r/pull/4","statusCheckRollup":[]}]"#;
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> {args}\ncase \"$1 $2\" in\n  \"pr list\") echo '{list}' ;;\n  *) echo '{{}}' ;;\nesac\n",
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

fn transcript(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_repository_subscription_reports_what_opened_merged_and_went_red() {
    let dir = scratch("github-prs");
    fake_gh(&dir);
    // The previous look: #1 open and green, #2 open; #3 and #4 unseen.
    let seen = serde_json::json!({
        "at_ms": 1_789_500_000_000i64,
        "prs": {
            "1": {"title": "Fix the parser", "state": "OPEN", "head": "aaaaaaa", "author": "jacob", "checks": {"build": "SUCCESS"}, "url": "https://github.com/o/r/pull/1"},
            "2": {"title": "Add the hub", "state": "OPEN", "head": "bbbbbbb", "author": "jacob", "url": "https://github.com/o/r/pull/2"}
        }
    })
    .to_string()
    .replace('"', "\\\"");
    std::fs::write(
        dir.join("place/.arbos/agents/root/subscriptions/0001-prs.toml"),
        format!(
            "id = 1\nkind = \"github_prs\"\nprompt = \"Read each new PR; tell me about merges\"\nrepo = \"o/r\"\nauthor = \"@me\"\ndeliver_to = \"agent\"\ncreated = \"2026-09-10T00:00:00Z\"\nnext_due = \"2026-09-10T00:00:00Z\"\nseen = \"{seen}\"\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("replies.jsonl"), "{\"content\":\"noted\"}\n").unwrap();
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
    // The overdue subscription fires; the message opens a turn for root.
    assert!(
        wait_for(Duration::from_secs(30), || {
            transcript(&k.place)
                .iter()
                .any(|e| e["kind"] == "turn_complete")
        }),
        "{:#?}",
        transcript(&k.place)
    );
    let root = transcript(&k.place);
    let say = root
        .iter()
        .find(|e| e["kind"] == "say" && e["from"] == "github")
        .unwrap_or_else(|| panic!("the github message opened the turn: {root:#?}"));
    let text = say["text"].as_str().unwrap();
    assert!(text.contains("o/r (PRs by @me)"), "{text}");
    assert!(text.contains("check build failed on #1"), "{text}");
    assert!(
        text.contains("merged #2 \"Add the hub\" by jacob"),
        "{text}"
    );
    assert!(text.contains("opened #3 \"Docs pass\""), "{text}");
    assert!(!text.contains("#4"), "an old merge is not news: {text}");
    assert!(text.contains("You asked: Read each new PR"), "{text}");
    // gh was asked for the whole repository, this author, every state.
    let args = std::fs::read_to_string(dir.join("gh-args.txt")).unwrap_or_default();
    assert!(args.contains("pr list --repo o/r --state all"), "{args}");
    assert!(args.contains("--author @me"), "{args}");
    // The subscription stays (a repository never "closes") and remembers.
    let sub = std::fs::read_to_string(
        k.place
            .join(".arbos/agents/root/subscriptions/0001-prs.toml"),
    )
    .unwrap();
    assert!(sub.contains("kind = \"github_prs\""), "{sub}");
    assert!(
        sub.contains("Docs pass"),
        "the new look is remembered: {sub}"
    );
    let _ = k.child.kill();
}

/// The model writes the singular kind it knows: `github_pr` with a repo
/// and no `pr` is the repository's pull requests; `author: "me"` is `@me`.
#[test]
fn github_pr_without_a_number_is_read_as_the_repositorys_pull_requests() {
    let parse = |v: serde_json::Value| -> arbos_core::subscription::Subscription {
        serde_json::from_value(v).unwrap()
    };
    let mut sub = parse(serde_json::json!({"kind": "github_pr", "repo": "o/r", "author": "me"}));
    assert!(sub.validate().is_err(), "as written it is refused…");
    let notes = sub.coerce();
    assert_eq!(sub.kind, "github_prs");
    assert_eq!(sub.author.as_deref(), Some("@me"));
    assert!(notes.iter().any(|n| n.contains("github_prs")), "{notes:?}");
    sub.validate().unwrap_or_else(|e| panic!("{e}"));
    // Repo alone is enough; the default period is GitHub's.
    let bare = parse(serde_json::json!({"kind": "github_prs", "repo": "o/r"}));
    bare.validate().unwrap();
    assert!(bare.every_ms().is_some());
    let none = parse(serde_json::json!({"kind": "github_prs"}));
    assert!(none.validate().is_err());
}
