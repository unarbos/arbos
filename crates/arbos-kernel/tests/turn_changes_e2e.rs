//! Per-turn changed paths (side-panels handover 5): the rewind
//! checkpoints' diff, readable by a client.
//!
//! Two turns on a git place. The first writes a file; the second grows it
//! and adds another. `turn_changes` answers with one row per turn, oldest
//! first: the first turn's files are final (`ended`), measured between
//! its checkpoint and the next turn's; the second turn's are measured
//! against the working tree now and may still grow. `touched` names what
//! the tools said they wrote. `limit` cuts to the newest; an unknown
//! agent is an error.

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
    std::fs::create_dir_all(dir.join(".arbos")).unwrap();
    // A plain worker: a coordinator writes only the store.
    std::fs::write(
        dir.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"worker\"\n",
    )
    .unwrap();
    std::fs::write(dir.join(".gitignore"), ".arbos/\n").unwrap();
    std::fs::write(dir.join("README.md"), "# toy\n").unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "start"]);
}

fn by_path<'a>(files: &'a [serde_json::Value], path: &str) -> &'a serde_json::Value {
    files
        .iter()
        .find(|f| f["path"] == path)
        .unwrap_or_else(|| panic!("no row for {path} in {files:?}"))
}

#[test]
fn each_turns_files_are_readable_from_the_checkpoints() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one line\",\"calls\":[",
        "{\"name\":\"write\",\"arguments\":{\"path\":\"notes.txt\",\"content\":\"one\\n\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"written\"}\n",
        "{\"agent\":\"root\",\"content\":\"two more\",\"calls\":[",
        "{\"name\":\"write\",\"arguments\":{\"path\":\"notes.txt\",\"content\":\"one\\ntwo\\nthree\\n\"}},",
        "{\"name\":\"write\",\"arguments\":{\"path\":\"extra.txt\",\"content\":\"x\\n\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"written\"}\n",
    );
    let k = start_kernel_replay_prepared("turn-changes", replies, "", make_repo);
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    for text in ["write one line", "add two more"] {
        a.send(serde_json::json!({"type":"user","agent":"root","text":text,"attachments":[]}));
        assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
        assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    }

    a.send(serde_json::json!({"type":"turn_changes","agent":"root"}));
    let list = a
        .wait(Duration::from_secs(20), |f| f["type"] == "turn_change_list")
        .expect("the list");
    assert_eq!(list["agent"], "root");
    let turns = list["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2, "{turns:#?}");

    let first = &turns[0];
    assert_eq!(first["ended"], true, "{first}");
    assert!(first.get("unmeasured").is_none(), "{first}");
    let files = first["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    let notes = by_path(files, "notes.txt");
    assert_eq!(notes["kind"], "added");
    assert_eq!(notes["added"], 1);
    assert_eq!(notes["removed"], 0);
    assert_eq!(first["touched"], serde_json::json!(["notes.txt"]));

    let second = &turns[1];
    assert_eq!(second["ended"], false, "{second}");
    assert!(second.get("unmeasured").is_none(), "{second}");
    let files = second["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{files:?}");
    let notes = by_path(files, "notes.txt");
    assert_eq!(notes["kind"], "modified");
    assert_eq!(notes["added"], 2);
    assert_eq!(notes["removed"], 0);
    let extra = by_path(files, "extra.txt");
    assert_eq!(extra["kind"], "added");
    assert_eq!(extra["added"], 1);
    assert_eq!(
        second["touched"],
        serde_json::json!(["extra.txt", "notes.txt"])
    );
    assert!(
        turns[0]["line"].as_u64().unwrap() < turns[1]["line"].as_u64().unwrap(),
        "oldest first"
    );
    assert!(turns[0]["ts"].as_i64().unwrap() <= turns[1]["ts"].as_i64().unwrap());

    // The newest turn alone.
    a.send(serde_json::json!({"type":"turn_changes","agent":"root","limit":1}));
    let list = a
        .wait(Duration::from_secs(20), |f| f["type"] == "turn_change_list")
        .expect("the list");
    let turns = list["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["line"], second["line"]);

    // A file changed by hand after the last turn shows on the open row —
    // it is measured against the tree now, not a saved end.
    std::fs::write(k.place.join("README.md"), "# toy\nmore\n").unwrap();
    a.send(serde_json::json!({"type":"turn_changes","agent":"root","limit":1}));
    let list = a
        .wait(Duration::from_secs(20), |f| f["type"] == "turn_change_list")
        .expect("the list");
    let files = list["turns"][0]["files"].as_array().unwrap();
    assert_eq!(by_path(files, "README.md")["kind"], "modified");

    a.send(serde_json::json!({"type":"turn_changes","agent":"nobody"}));
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("an error");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("no agent is named nobody"),
        "{err}"
    );
}
