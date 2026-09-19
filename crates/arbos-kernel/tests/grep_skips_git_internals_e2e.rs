//! `grep` walked hidden files — right for `.github/` and dotfiles — and so
//! reached the repository's own `.git/`: a grep for a word that was once
//! a commit message came back with `.git/logs/HEAD`, `.git/logs/refs/…`
//! and `.git/COMMIT_EDITMSG` beside the one file that had it, as if they
//! were the project's files (probe on main f56a6877: 4 hits, 3 of them
//! `.git/`). Cursor's grep never shows `.git/`. Now neither the index nor
//! the walk holds it, and an explicit `.git/…` path still reaches it.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"looking\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"xylophone\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"case-insensitive walk\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"XYLOPHONE\",\"ignore_case\":true}}]}\n",
    "{\"agent\":\"root\",\"content\":\"asked for the reflog\",\"calls\":[{\"name\":\"grep\",\"arguments\":{\"pattern\":\"xylophone\",\"path\":\".git/logs\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"done\"}\n",
);

fn git(place: &std::path::Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(args)
        .current_dir(place)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

#[test]
fn grep_finds_the_file_and_not_the_reflog_unless_asked() {
    let mut k = start_kernel_replay_prepared("grep-git", REPLIES, "", |place| {
        git(place, &["init", "-q"]);
        std::fs::write(place.join("msg.txt"), "xylophone in a file\n").unwrap();
        std::fs::create_dir_all(place.join(".github")).unwrap();
        std::fs::write(place.join(".github/ci.yml"), "name: xylophone\n").unwrap();
        git(place, &["add", "-A"]);
        git(
            place,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "xylophone tuned",
            ],
        );
        assert!(
            std::fs::read_to_string(place.join(".git/logs/HEAD"))
                .unwrap()
                .contains("xylophone"),
            "the reflog holds the word"
        );
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // The index, when it is ready, or the walk: both must agree. Give the
    // index its moment first so the first grep is likely to use it.
    std::thread::sleep(Duration::from_millis(1500));
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "find xylophone"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap_or_default()
            .contains("turn_complete")
    }));
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let greps: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "grep")
        .map(|e| e["body"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(greps.len(), 3, "{root:#?}");
    for body in &greps[..2] {
        let paths: Vec<&str> = body
            .lines()
            .filter_map(|l| l.split(':').next())
            .filter(|p| !p.is_empty() && !p.starts_with('…'))
            .collect();
        assert!(paths.contains(&"msg.txt"), "{body}");
        assert!(
            paths.contains(&".github/ci.yml"),
            "hidden project files stay: {body}"
        );
        assert!(
            !body.contains(".git/"),
            "the repository's machinery is not the project: {body}"
        );
    }
    assert!(
        greps[2].contains(".git/logs/HEAD"),
        "asked for by path, the reflog is reached: {}",
        greps[2]
    );
    let _ = k.child.kill();
}
