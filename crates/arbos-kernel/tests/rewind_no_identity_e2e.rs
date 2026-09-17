//! qal-j08 (QA's `rw-04`): in a repository with no git identity — a new
//! machine, a fresh place — every checkpoint carried HEAD alone, because
//! the working-tree commit failed silently; a rewind with `files: true`
//! then `reset --hard` + `clean -fd`, deleting the kept turns' untracked
//! files, and reported success. Now the internal commit carries its own
//! identity, a checkpoint that could not be saved says so, and a restore
//! never cleans on a checkpoint that does not know its tree.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

fn git(dir: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

fn checkpoints(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/checkpoints.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn rewind_with_files_in_a_repo_without_git_identity_keeps_the_kept_turns_files() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo first > f1.txt\",\"description\":\"Write f1\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"wrote f1\"}\n",
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo second > f2.txt\",\"description\":\"Write f2\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"wrote f2\"}\n",
        "{\"agent\":\"root\",\"content\":\"third, nothing written\"}\n",
    );
    let mut k = start_kernel_replay_prepared("rewind-no-identity", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        // Root works in place; a git repository with one commit and no
        // identity anywhere (the repo's own config says empty, which beats
        // the machine's global one).
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"worker\"\n",
        )
        .unwrap();
        std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
        git(place, &["init", "-q"]);
        git(place, &["add", ".gitignore"]);
        git(
            place,
            &[
                "-c",
                "user.name=setup",
                "-c",
                "user.email=setup@t",
                "commit",
                "-q",
                "-m",
                "start",
            ],
        );
        git(place, &["config", "user.name", ""]);
        git(place, &["config", "user.email", ""]);
        git(place, &["config", "user.useConfigOnly", "true"]);
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    for text in ["write f1", "write f2", "third"] {
        a.send(serde_json::json!({"type":"user","agent":"root","text":text,"attachments":[]}));
        assert!(
            a.wait_turn("root", "idle", Duration::from_secs(30)),
            "{text}"
        );
    }
    assert!(k.place.join("f1.txt").exists() && k.place.join("f2.txt").exists());
    let cps = checkpoints(&k.place);
    assert_eq!(cps.len(), 3, "{cps:#?}");
    // Turn 1 started on a clean tree; turns 2 and 3 carry the files as a
    // work commit. Nothing is silently empty.
    assert_eq!(cps[0]["clean"], true, "{:?}", cps[0]);
    for cp in &cps[1..] {
        assert!(
            cp["work"].is_string(),
            "a work commit despite no identity: {cp}"
        );
        assert!(cp.get("work_error").is_none(), "{cp}");
    }

    // Rewind to turn 3 with the files: turns 1 and 2 are kept, so f1 and
    // f2 must be there afterwards.
    a.send(serde_json::json!({"type":"rewind","agent":"root","turn":3,"files":true}));
    let restored = a
        .wait(Duration::from_secs(20), |f| {
            (f["type"] == "rewound" && !f["restored"].is_null()) || f["type"] == "error"
        })
        .expect("the restore reports");
    assert_eq!(restored["type"], "rewound", "{restored}");
    assert!(
        restored["restored"]
            .as_str()
            .is_some_and(|s| s.contains("working tree")),
        "the restore says it brought the tree back: {restored}"
    );
    assert_eq!(
        std::fs::read_to_string(k.place.join("f1.txt"))
            .ok()
            .as_deref(),
        Some("first\n"),
        "the kept turn's file"
    );
    assert_eq!(
        std::fs::read_to_string(k.place.join("f2.txt"))
            .ok()
            .as_deref(),
        Some("second\n"),
        "the kept turn's file"
    );
    let _ = k.child.kill();
}

/// A checkpoint that knows it has no tree (an old line, or a save that
/// failed for another reason) must not delete on the strength of it: the
/// transcript is rewound, the files stay, and the client hears why.
#[test]
fn a_rewind_on_a_checkpoint_without_a_tree_leaves_the_files_and_says_so() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo first > f1.txt\",\"description\":\"Write f1\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"wrote f1\"}\n",
        "{\"agent\":\"root\",\"content\":\"second\"}\n",
    );
    let mut k = start_kernel_replay_prepared("rewind-old-checkpoint", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"worker\"\n",
        )
        .unwrap();
        std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
        git(place, &["init", "-q"]);
        git(place, &["add", ".gitignore"]);
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
                "start",
            ],
        );
    });
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    for text in ["write f1", "second"] {
        a.send(serde_json::json!({"type":"user","agent":"root","text":text,"attachments":[]}));
        assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    }
    // Rewrite the checkpoints as a kernel from before the record wrote
    // them: HEAD alone, no `clean`, no `work`.
    let path = k.place.join(".arbos/agents/root/checkpoints.jsonl");
    let old: String = checkpoints(&k.place)
        .iter()
        .map(|cp| {
            format!(
                "{}\n",
                serde_json::json!({"line": cp["line"], "ts": cp["ts"], "head": cp["head"]})
            )
        })
        .collect();
    std::fs::write(&path, old).unwrap();
    a.send(serde_json::json!({"type":"rewind","agent":"root","turn":2,"files":true}));
    // Not an `error` frame (a rewind that half-worked drawn as a crash):
    // a second `rewound` closes the pending state with nothing restored,
    // and a notice on the transcript says why, and when it stops.
    // `pending` is omitted when false.
    let closed = a
        .wait(Duration::from_secs(20), |f| {
            (f["type"] == "rewound" && f["pending"] != true) || f["type"] == "error"
        })
        .expect("the restore reports");
    assert_eq!(closed["type"], "rewound", "not an error: {closed}");
    assert!(closed["restored"].is_null(), "{closed}");
    let report = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "event"
                && f["event"]["kind"] == "notice"
                && f["event"]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("Files were not restored"))
        })
        .expect("the notice on the transcript");
    assert_eq!(report["event"]["failed"], false, "{report}");
    let text = report["event"]["text"].as_str().unwrap_or("");
    assert!(text.contains("no checkpoint of the working tree"), "{text}");
    assert!(
        text.contains("turns recorded from now on restore their files"),
        "says when it stops: {text}"
    );
    assert!(
        k.place.join("f1.txt").exists(),
        "nothing untracked was removed"
    );
    let _ = k.child.kill();
}
