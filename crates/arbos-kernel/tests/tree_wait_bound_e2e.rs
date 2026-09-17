//! The checkpoint of the working tree is taken beside the turn, and the
//! turn's first write waits for it (qal-j17). On a repository where
//! `add -A` runs for minutes — a home folder with a dotfiles repo, a
//! monorepo with build output untracked — that wait would hold every
//! turn's first command. Past a bound the turn goes on without a file
//! checkpoint, says so once, and the tree that finishes later is dropped:
//! a tree taken beside the turn's own writes is the wrong checkpoint,
//! worse than none.

mod common;

use common::Attach;
use std::{path::Path, process::Command, time::Duration};

fn git(dir: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success(),
        "git {args:?}"
    );
}

fn events(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_tree_that_takes_too_long_does_not_hold_the_turn_and_is_not_kept_afterwards() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo first > f1.txt\",\"description\":\"Write f1\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"wrote f1\"}\n",
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo second > f2.txt\",\"description\":\"Write f2\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"wrote f2\"}\n",
    );
    let scratch = common::scratch_dir("tree-wait-bound");
    let place = scratch.join("place");
    std::fs::create_dir_all(place.join(".arbos")).unwrap();
    std::fs::write(
        place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"worker\"\n",
    )
    .unwrap();
    std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
    git(&place, &["init", "-q"]);
    git(&place, &["add", ".gitignore"]);
    git(
        &place,
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
    let file = scratch.join("replies.jsonl");
    std::fs::write(&file, replies).unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    // The tree takes four seconds; the turn waits at most one and a half.
    let mut k = common::spawn_with_env(
        scratch,
        &[
            "--provider",
            "replay",
            "--replies",
            &file.display().to_string(),
        ],
        &[
            ("ARBOS_TEST_TREE_DELAY_MS", "4000"),
            ("ARBOS_TREE_WAIT_MS", "1500"),
        ],
    );
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    let sent = std::time::Instant::now();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"write f1","attachments":[]}));
    let mut tool_started_at: Option<std::time::Instant> = None;
    let idle = a.wait(Duration::from_secs(30), |f| {
        if f["type"] == "event"
            && f["event"]["kind"] == "tool"
            && f["event"]["name"] == "bash"
            && tool_started_at.is_none()
        {
            tool_started_at = Some(std::time::Instant::now());
        }
        f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle"
    });
    assert!(idle.is_some(), "turn 1 ends");
    let held = tool_started_at
        .expect("the bash started")
        .duration_since(sent);
    assert!(
        held < Duration::from_millis(3500),
        "the write went on after the bound, not after the tree: held {held:?}"
    );
    assert!(k.place.join("f1.txt").exists());

    // Said once, on the transcript, with the cause and the advice.
    let evs = events(&k.place);
    let said: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|e| {
            e["kind"] == "notice"
                && e["text"].as_str().is_some_and(|t| {
                    t.starts_with("Saving a checkpoint of the working tree took longer")
                })
        })
        .collect();
    assert_eq!(said.len(), 1, "{evs:#?}");
    assert!(
        said[0]["text"].as_str().unwrap().contains(".gitignore"),
        "{}",
        said[0]
    );

    // The tree finishes later and is dropped: the record says why, and a
    // rewind of files to this turn is refused rather than guessed.
    assert!(
        common::wait_for(Duration::from_secs(10), || {
            std::fs::read_to_string(k.place.join(".arbos/agents/root/checkpoints.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .any(|cp| {
                    cp["work_error"]
                        .as_str()
                        .is_some_and(|e| e.contains("took too long to snapshot"))
                })
        }),
        "the late tree is not kept: {}",
        std::fs::read_to_string(k.place.join(".arbos/agents/root/checkpoints.jsonl"))
            .unwrap_or_default()
    );
    let cps: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/checkpoints.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    assert!(cps.iter().all(|cp| cp["work"].is_null()), "{cps:#?}");

    // A second turn on the same slow tree: the same rule, but the notice
    // is not repeated.
    a.send(serde_json::json!({"type":"user","agent":"root","text":"write f2","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let evs = events(&k.place);
    assert_eq!(
        evs.iter()
            .filter(|e| e["kind"] == "notice"
                && e["text"].as_str().is_some_and(
                    |t| t.starts_with("Saving a checkpoint of the working tree took longer")
                ))
            .count(),
        1,
        "once per place"
    );
    let _ = k.child.kill();
}
