//! Mobile cycle 1, item 3: every worker on a fresh machine that tried to
//! commit reported "the author identity (user.name / user.email) is not
//! configured" and asked the user. A repository with no identity now
//! commits as `Arbos <unarbos@users.noreply.github.com>` — the kernel
//! sets the author in the job's environment and the git guard no longer
//! asks — while a configured identity is used as it is.

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
            .env("HOME", dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "git {args:?} in {}",
        dir.display()
    );
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

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"committing\",\"calls\":[",
    "{\"name\":\"bash\",\"arguments\":{\"command\":\"git checkout -q -b fix/greeting && git add -A && git commit -q -m greet && git log -1 --format='%an <%ae>'\",\"cwd\":\"toy-repo\",\"description\":\"Commit the greeting on a branch\"}}",
    "]}\n",
    "{\"agent\":\"root\",\"content\":\"committed\"}\n",
);

#[test]
fn a_repository_with_no_identity_commits_as_arbos_without_asking() {
    let mut k = start_kernel_replay_prepared("git-author", REPLIES, "", |place| {
        let repo = place.join("toy-repo");
        std::fs::create_dir_all(&repo).unwrap();
        // The kernel's HOME is the scratch home (no .gitconfig): a fresh
        // machine. The seed commit gets an explicit author on the
        // command line only.
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("hello.py"), "print('hi')\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=seed",
                "-c",
                "user.email=seed@t",
                "commit",
                "-q",
                "-m",
                "start",
            ],
        );
        std::fs::write(repo.join("hello.py"), "print('hello')\n").unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "commit the greeting"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    let root = transcript(&k.place, "root");
    let call = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .unwrap_or_else(|| panic!("the bash call: {root:#?}"));
    assert!(
        call.get("error").is_none(),
        "no identity ask, no refusal: {call:#?}"
    );
    let body = call["body"].as_str().unwrap_or("");
    assert!(
        body.contains("Arbos <unarbos@users.noreply.github.com>"),
        "the commit is authored as Arbos: {body}"
    );
    let _ = k.child.kill();
}
