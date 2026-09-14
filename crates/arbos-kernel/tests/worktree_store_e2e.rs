//! qa-035: a worker in its own worktree is confined to that checkout, but
//! the project store is every worker's — `.arbos/docs/project-context.md`
//! to read, `.arbos/docs/` to write into, its own agent folder — while
//! the page (`.arbos/notes.md`) stays root's and the parent's checkout
//! stays out of reach.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn make_repo(place: &Path) {
    std::fs::create_dir_all(place).unwrap();
    std::fs::write(place.join("main.py"), "print(1)\n").unwrap();
    assert!(git(place, &["init", "-q"]));
    assert!(git(place, &["add", "-A"]));
    assert!(git(
        place,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "start"
        ],
    ));
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

fn wait_for(timeout: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    ok()
}

#[test]
fn a_worktree_worker_reads_and_writes_the_project_store() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"read the context, write a report\",\"isolate\":\"worktree\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        // The worker: the brief's first read, a deliverable under docs/, a
        // note in its own folder, a write to its worktree, and two it must
        // not have — the page, and the parent's checkout by absolute path.
        "{\"content\":\"working\",\"calls\":[",
        "{\"name\":\"read\",\"arguments\":{\"path\":\".arbos/docs/project-context.md\"}},",
        "{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/docs/report.md\",\"contents\":\"# Report\\nfrom the worktree\\n\"}},",
        "{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/agents/w1/scratch.md\",\"contents\":\"mine\\n\"}},",
        "{\"name\":\"write\",\"arguments\":{\"path\":\"src/new.py\",\"contents\":\"print(2)\\n\"}},",
        "{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/notes.md\",\"contents\":\"- [ ] hijack\\n\"}}",
        "]}\n",
        "{\"content\":\"done\"}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("wt-store", replies, "", |place| {
        make_repo(place);
        std::fs::create_dir_all(place.join(".arbos/docs")).unwrap();
        std::fs::write(
            place.join(".arbos/docs/project-context.md"),
            "# Context\nGoal: the codeword is xylophone\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "one worker please"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        wait_for(Duration::from_secs(40), || transcript(&k.place, "w1")
            .iter()
            .any(|e| e["kind"] == "turn_complete")),
        "w1's turn ends"
    );
    let w1 = transcript(&k.place, "w1");
    let tool = |name: &str, path: &str| -> serde_json::Value {
        w1.iter()
            .find(|e| e["kind"] == "tool" && e["name"] == name && e["args"]["path"] == path)
            .unwrap_or_else(|| panic!("no {name} {path} record: {w1:#?}"))
            .clone()
    };
    // The worker ran in a worktree, not the checkout.
    assert!(k.place.join(".arbos/worktrees/w1").is_dir());
    let read = tool("read", ".arbos/docs/project-context.md");
    assert!(read.get("error").is_none(), "{read:#?}");
    assert!(
        read["body"].as_str().unwrap_or("").contains("xylophone"),
        "{read:#?}"
    );
    let report = tool("write", ".arbos/docs/report.md");
    assert!(report.get("error").is_none(), "{report:#?}");
    assert_eq!(
        std::fs::read_to_string(k.place.join(".arbos/docs/report.md")).unwrap(),
        "# Report\nfrom the worktree\n"
    );
    let scratch = tool("write", ".arbos/agents/w1/scratch.md");
    assert!(scratch.get("error").is_none(), "{scratch:#?}");
    assert!(k.place.join(".arbos/agents/w1/scratch.md").exists());
    // Code lands in the worktree, never in the parent's checkout.
    let code = tool("write", "src/new.py");
    assert!(code.get("error").is_none(), "{code:#?}");
    assert!(k.place.join(".arbos/worktrees/w1/src/new.py").exists());
    assert!(!k.place.join("src/new.py").exists());
    // The page is root's, from a worktree as from anywhere.
    let page = tool("write", ".arbos/notes.md");
    let err = page["error"].as_str().unwrap_or("");
    assert!(err.contains("root"), "the page write is refused: {page:#?}");
    assert!(
        !k.place.join(".arbos/notes.md").exists() || {
            !std::fs::read_to_string(k.place.join(".arbos/notes.md"))
                .unwrap_or_default()
                .contains("hijack")
        }
    );
    let _ = k.child.kill();
}
