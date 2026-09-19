//! A place that is a subfolder of a repository (`repo/packages/app` opened
//! as the project): no checkpoint is written there, so rewind and undo are
//! off — rightly — and until now nobody was told. The kernel says it once
//! on the main chat at start, with the root and the two ways; a later
//! `git init` in the folder clears it.

mod common;

use common::{Attach, restart_replay, scratch_dir, spawn_with};
use std::time::Duration;

fn notices(place: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "notice")
        .map(|v| v["text"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn a_place_inside_a_repository_is_told_once_and_cleared_when_it_becomes_its_own() {
    let scratch = scratch_dir("inside-repo");
    // The scratch's `place` becomes `packages/app` of a repository rooted
    // one level up: scratch/place is the repo, scratch/place/packages/app
    // the place we serve.
    let repo = scratch.join("place");
    let git = |dir: &std::path::Path, a: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(a)
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("top.txt"), "top\n").unwrap();
    git(&repo, &["add", "top.txt"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "start",
        ],
    );
    let app = repo.join("packages").join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        scratch.join("xdg").join("arbos").join("config.toml"),
        "trace = false\n",
    )
    .unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    // Serve the subfolder: `spawn_with` serves scratch/place, so the
    // repository root itself would be the place. Point the kernel at the
    // subfolder by making it the scratch's place instead.
    let sub_scratch = scratch.join("sub");
    std::fs::create_dir_all(sub_scratch.join("xdg").join("arbos")).unwrap();
    std::fs::create_dir_all(sub_scratch.join("home")).unwrap();
    std::fs::write(
        sub_scratch.join("xdg").join("arbos").join("config.toml"),
        "trace = false\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&app, sub_scratch.join("place")).unwrap();
    let mut k = spawn_with(
        sub_scratch.clone(),
        &[
            "--provider",
            "replay",
            "--replies",
            replies.to_str().unwrap(),
        ],
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let said: Vec<String> = notices(&app)
        .into_iter()
        .filter(|t| t.starts_with("This folder is inside the repository at"))
        .collect();
    assert_eq!(said.len(), 1, "{:?}", notices(&app));
    assert!(said[0].contains("`git init` in this folder"), "{}", said[0]);
    let log = std::fs::read_to_string(app.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("place_inside_repository"), "{log}");
    // A second start says it in the log, not again on the chat.
    let mut k2 = restart_replay(&mut k, "");
    let mut b = Attach::connect(&k2.url);
    assert!(
        b.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|t| t.starts_with("This folder is inside the repository at"))
            .count(),
        1
    );
    let _ = k2.child.kill();
    let _ = k2.child.wait();
    // The person makes the folder its own repository: cleared, said once.
    git(&app, &["init", "-q"]);
    let mut k3 = spawn_with(
        sub_scratch.clone(),
        &[
            "--provider",
            "replay",
            "--replies",
            replies.to_str().unwrap(),
        ],
    );
    let mut c = Attach::connect(&k3.url);
    assert!(
        c.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|t| t.starts_with("This folder is a repository of its own now"))
            .count(),
        1,
        "{:?}",
        notices(&app)
    );
    assert!(!app.join(".arbos/runtime/inside-repository.said").exists());
    let _ = k3.child.kill();
}
