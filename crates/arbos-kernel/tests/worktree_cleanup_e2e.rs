//! K-01c: a worktree worker's checkout goes when the worker is archived
//! and nothing would be lost; `check` names the ones left behind.

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
    assert!(git(place, &["init", "-q"]));
    assert!(git(
        place,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "start"
        ],
    ));
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
fn an_archived_workers_clean_worktree_is_removed() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"w1\",\"task\":\"say one word\",\"isolate\":\"worktree\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        // After root's dispatch turn closes, so the report opens the done
        // turn that archives w1 (the fold-race family's pin; the worktree
        // add bought this test time but promised none).
        "{\"content\":\"word\",\"delay_ms\":2500}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("wt-archive", replies, "", |place| {
        make_repo(place);
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = true\n",
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
    let wt = k.place.join(".arbos/worktrees/w1");
    // The worktree existed for the worker…
    assert!(wait_for(Duration::from_secs(20), || {
        wt.is_dir() || k.place.join(".arbos/archive/agents/w1").is_dir()
    }));
    // …and goes when the worker is archived.
    assert!(
        wait_for(Duration::from_secs(30), || k
            .place
            .join(".arbos/archive/agents/w1")
            .is_dir()),
        "w1 archived"
    );
    assert!(
        wait_for(Duration::from_secs(10), || !wt.exists()),
        "the clean worktree is removed with the archive"
    );
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(log.contains("worktree_removed"), "{log}");
    assert!(log.contains("arbos/w1 had no commits; deleted"), "{log}");
    let _ = k.child.kill();
}

#[test]
fn check_names_a_worktree_whose_worker_is_gone() {
    let dir = std::env::temp_dir().join(format!(
        "arbos-wt-check-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    make_repo(&dir);
    let place = arbos_core::Place::new(&dir);
    arbos_core::bootstrap(&place).unwrap();
    // A worktree for a worker that no longer has an agent folder, with an
    // uncommitted file in it.
    let wt = arbos_kernel::worktree::create(&dir, "ghost").unwrap();
    std::fs::write(wt.path.join("left.txt"), "x\n").unwrap();
    let report = arbos_kernel::check::check(&place).unwrap();
    let finding = report
        .findings
        .iter()
        .find(|f| f.path.ends_with(".arbos/worktrees/ghost"))
        .unwrap_or_else(|| panic!("{:#?}", report.findings));
    assert_eq!(finding.level, "warning");
    assert!(
        finding.what.contains("1 uncommitted path"),
        "{}",
        finding.what
    );
    assert!(
        finding.what.contains("git worktree remove"),
        "{}",
        finding.what
    );
    let _ = std::fs::remove_dir_all(&dir);
}
