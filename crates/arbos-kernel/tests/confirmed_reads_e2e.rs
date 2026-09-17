//! The qal-j08 family, closed at the helper: a read that cannot tell
//! *absent* from *unknown* becomes an empty value, and a destructive step
//! acts on it with confidence. `arbos_core::record` keeps the three apart;
//! the rule is no destructive step on a record we have not confirmed.
//! Two members QA drove, driven here the same way.

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

fn tools(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool" && e["ended"].is_number())
        .collect()
}

const PAGE: &str = "# Shapes\n\nGoal: a small geometry library.\n\n## Checklist\n\n- [ ] Add area() tests — ready\n- [ ] Add perimeter() — ready\n- [x] Read the folder — done\n\n## Notes\n\nThe reporter's example used 3 by 4.\nSecond line of notes.\n";

/// qal-j09 (QA's `sw-01`): one unreadable read of `.arbos/notes.md`, and
/// the next `plan` call used to write a one-line page over it, "Set 1
/// item(s)." Now the call fails with the reason and the page keeps its
/// bytes.
#[test]
#[cfg(unix)]
fn an_unreadable_project_page_is_not_rewritten_by_plan() {
    use std::os::unix::fs::PermissionsExt;
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"plan\",\"arguments\":{\"op\":\"set\",\"items\":[{\"section\":\"Checklist\",\"label\":\"Add perimeter() tests\",\"readout\":\"ready\"}]}}]}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
    );
    let mut k = start_kernel_replay_prepared("unreadable-page", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(place.join(".arbos/notes.md"), PAGE).unwrap();
    });
    let page = k.place.join(".arbos/notes.md");
    let before = std::fs::metadata(&page).unwrap().len();
    assert_eq!(before, PAGE.len() as u64);
    // The injector: the page unreadable for the call (in life: EIO, a
    // lock, a partial view on a mount).
    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(
        serde_json::json!({"type":"user","agent":"root","text":"add the item","attachments":[]}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o644)).unwrap();
    let after = std::fs::read_to_string(&page).unwrap();
    assert_eq!(after, PAGE, "the page keeps its bytes");
    let calls = tools(&k.place);
    let plan = calls
        .iter()
        .find(|c| c["name"] == "plan")
        .unwrap_or_else(|| panic!("{calls:#?}"));
    let err = plan["error"].as_str().unwrap_or("");
    assert!(
        err.contains("could not read") && err.contains("not written"),
        "the call fails with the reason, not \"Set 1 item(s).\": {plan}"
    );
    let _ = k.child.kill();
}

/// qal-j10 (QA's `sw-02`): the turn-start mark could not be written, so
/// it kept an older turn's HEAD, and `undo` reset to it — deleting a
/// committed file from a kept turn — and said "restored". Now the mark
/// names its turn, a write that fails leaves no stale mark and is said
/// on the transcript, and `undo` refuses a mark that is not this turn's.
#[test]
#[cfg(unix)]
fn a_stale_undo_mark_is_refused_and_a_kept_commit_stands() {
    use std::os::unix::fs::PermissionsExt;
    let replies = concat!(
        // Turn 1: commit A.
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo a > a.txt && git add a.txt && git -c user.name=t -c user.email=t@t commit -q -m A\",\"description\":\"Commit A\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"committed A\"}\n",
        // Turn 2 (mark unwritable): commit B, then undo.
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo b > b.txt && git add b.txt && git -c user.name=t -c user.email=t@t commit -q -m B\",\"description\":\"Commit B\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"\",\"calls\":[{\"name\":\"undo\",\"arguments\":{}}]}\n",
        "{\"agent\":\"root\",\"content\":\"undone\"}\n",
    );
    let mut k = start_kernel_replay_prepared("stale-undo-mark", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"worker\"\n",
        )
        .unwrap();
        std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
        git(place, &["init", "-q", "-b", "work"]);
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
    a.send(serde_json::json!({"type":"user","agent":"root","text":"commit A","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let head_a = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&k.place)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    // The injector: the mark and its folder read-only before turn 2
    // starts (in life: a full disk).
    let runtime = k.place.join(".arbos/runtime");
    let mark = runtime.join("checkpoint");
    assert!(mark.exists(), "turn 1 wrote its mark");
    std::fs::set_permissions(&mark, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o555)).unwrap();
    a.send(serde_json::json!({"type":"user","agent":"root","text":"commit B then undo","attachments":[]}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::set_permissions(&mark, std::fs::Permissions::from_mode(0o644));

    let head_now = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&k.place)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let log = String::from_utf8(
        std::process::Command::new("git")
            .args(["log", "--format=%s"])
            .current_dir(&k.place)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        log.lines().any(|l| l == "A"),
        "commit A, from the kept turn, stands: {log}"
    );
    assert!(k.place.join("a.txt").exists(), "and its file");
    assert_ne!(
        head_now, head_a,
        "turn 2 moved HEAD to B; undo did not reset it past A"
    );
    let calls = tools(&k.place);
    let undo = calls
        .iter()
        .find(|c| c["name"] == "undo")
        .unwrap_or_else(|| panic!("{calls:#?}"));
    let body = undo["body"].as_str().unwrap_or("");
    assert!(
        body.contains("no checkpoint for this turn") && body.contains("nothing reset"),
        "undo refused rather than reset to the stale mark: {undo}"
    );
    // The failed write was said, once, on the transcript.
    let notices: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|e| {
                e["kind"] == "notice"
                    && e["text"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("Checkpoint not written"))
            })
            .collect();
    assert_eq!(notices.len(), 1, "{notices:#?}");
    let _ = k.child.kill();
}

/// qal-j12 (QA's `sw-05`): the one-time migration wrote its new records
/// and then `let _ = rename(plan.jsonl → .migrated)` — the only record
/// that it had happened. When that rename failed, the next start
/// migrated again: the standing cron existed twice and fired twice for
/// ever. Now the completion record comes first: a start that cannot
/// move the old plan aside migrates nothing and says so; a completed
/// migration is never repeated.
#[test]
#[cfg(unix)]
fn a_failed_migration_record_writes_nothing_and_a_completed_one_is_not_repeated() {
    use common::restart_replay;
    use std::os::unix::fs::PermissionsExt;
    let now = arbos_core::now_ms();
    let legacy = format!(
        "{{\"id\":1,\"goal\":\"tick\",\"when\":{{\"every_ms\":30000,\"next_due_ms\":{}}},\"do\":{{\"kind\":\"shell\",\"cmd\":\"echo legacy-tick >> ticks.txt\",\"report\":\"tick: {{output}}\"}},\"status\":\"pending\"}}\n\
         {{\"id\":2,\"goal\":\"Reply with the single word MIGRATED.\",\"status\":\"pending\",\"origin\":\"user\",\"when\":{{\"wake\":true}}}}\n",
        now + 3_600_000
    );
    let mut k = start_kernel_replay_prepared(
        "failed-migration",
        "{\"agent\":\"root\",\"content\":\"MIGRATED\"}\n",
        "",
        |place| {
            let agent = place.join(".arbos/agents/root");
            std::fs::create_dir_all(agent.join("subscriptions")).unwrap();
            std::fs::create_dir_all(agent.join("inbox")).unwrap();
            std::fs::write(agent.join("agent.md"), "name: root\nmodel: inherit\n").unwrap();
            std::fs::write(agent.join("plan.jsonl"), &legacy).unwrap();
            std::fs::write(agent.join("transcript.jsonl"), "").unwrap();
            // QA's injector: the agent folder read-only, its subfolders
            // writable — the migration can write its records but not its
            // completion.
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o555)).unwrap();
        },
    );
    let agent = k.place.join(".arbos/agents/root");
    let count = |sub: &str| -> usize {
        std::fs::read_dir(agent.join(sub))
            .map(|d| {
                d.flatten()
                    .filter(|e| {
                        std::fs::read_to_string(e.path())
                            .unwrap_or_default()
                            .contains("legacy-tick")
                            || e.file_name().to_string_lossy().contains("user")
                    })
                    .count()
            })
            .unwrap_or(0)
    };
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    drop(a);
    let mut k2 = restart_replay(&mut k, "{\"agent\":\"root\",\"content\":\"MIGRATED\"}\n");
    let mut a = Attach::connect(&k2.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    drop(a);
    let _ = k2.child.kill();
    let _ = k2.child.wait();
    assert!(agent.join("plan.jsonl").exists(), "the old plan stays");
    assert_eq!(
        count("subscriptions"),
        0,
        "no cron on an unrecorded migration"
    );
    assert_eq!(count("inbox"), 0, "no task either");
    let log = std::fs::read_to_string(k2.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert_eq!(
        log.matches("\"event\":\"migrate_blocked\"").count(),
        2,
        "{log}"
    );

    // Writable: one migration on the next start, none on the one after.
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut k3 = restart_replay(&mut k2, "{\"agent\":\"root\",\"content\":\"MIGRATED\"}\n");
    let mut a = Attach::connect(&k3.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    drop(a);
    let mut k4 = restart_replay(&mut k3, "{\"agent\":\"root\",\"content\":\"MIGRATED\"}\n");
    let mut a = Attach::connect(&k4.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    assert!(
        !agent.join("plan.jsonl").exists() && agent.join("plan.jsonl.migrated").exists(),
        "migrated once"
    );
    assert_eq!(
        count("subscriptions"),
        1,
        "exactly one cron after two starts"
    );
    let _ = k4.child.kill();
}

/// qal-j13: a `plan.jsonl.migrating` with no `plan.jsonl` — an earlier
/// start cut between moving the file aside and finishing — was correctly
/// never migrated blind and its source correctly kept, but only the log
/// said so: the window showed an ordinary place and a standing cron the
/// person had was simply gone from what runs. Now the cut migration is
/// finished with what it already wrote recognised, and the transcript
/// says what was carried over this time and what was already in place.
#[test]
fn a_cut_migration_is_finished_without_doubling_and_the_transcript_says_so() {
    let now = arbos_core::now_ms();
    let legacy = format!(
        "{{\"id\":1,\"goal\":\"tick\",\"when\":{{\"every_ms\":30000,\"next_due_ms\":{}}},\"do\":{{\"kind\":\"shell\",\"cmd\":\"echo legacy-tick >> ticks.txt\",\"report\":\"tick: {{output}}\"}},\"status\":\"pending\"}}\n\
         {{\"id\":2,\"goal\":\"Reply with the single word MIGRATED.\",\"status\":\"pending\",\"origin\":\"user\",\"when\":{{\"wake\":true}}}}\n",
        now + 3_600_000
    );
    // The cut, as it stands on disk: the source moved aside, the cron
    // written, the task not, and no plan.jsonl.
    let mut k = start_kernel_replay_prepared(
        "cut-migration",
        "{\"agent\":\"root\",\"content\":\"MIGRATED\"}\n",
        "",
        |place| {
            let agent = place.join(".arbos/agents/root");
            std::fs::create_dir_all(agent.join("subscriptions")).unwrap();
            std::fs::write(agent.join("agent.md"), "name: root\nmodel: inherit\n").unwrap();
            std::fs::write(agent.join("plan.jsonl.migrating"), &legacy).unwrap();
            std::fs::write(
                agent.join("subscriptions/0001-tick.toml"),
                format!(
                    "kind = \"shell\"\nevery = \"30s\"\ncmd = \"echo legacy-tick >> ticks.txt\"\ndeliver_to = \"user\"\nnotify = \"tick: {{output}}\"\nnext_due = \"{}\"\n",
                    arbos_core::inbox::rfc3339(now + 3_600_000)
                ),
            )
            .unwrap();
        },
    );
    let agent = k.place.join(".arbos/agents/root");
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    // The task was carried over this time and runs (at boot, before any
    // window attaches: read the record).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !std::fs::read_to_string(agent.join("transcript.jsonl"))
        .unwrap_or_default()
        .contains("\"text\":\"MIGRATED\"")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the pending task, missed by the cut start, ran"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let crons = std::fs::read_dir(agent.join("subscriptions"))
        .unwrap()
        .flatten()
        .filter(|e| {
            std::fs::read_to_string(e.path())
                .unwrap_or_default()
                .contains("legacy-tick")
        })
        .count();
    assert_eq!(
        crons, 1,
        "the cron the cut start had written was recognised, not doubled"
    );
    assert!(
        !agent.join("plan.jsonl.migrating").exists() && agent.join("plan.jsonl.migrated").exists(),
        "finished"
    );
    let transcript = std::fs::read_to_string(agent.join("transcript.jsonl")).unwrap();
    assert!(
        transcript.contains("cut before it finished")
            && transcript.contains("1 pending task(s) carried over this time")
            && transcript.contains("1 were already in place"),
        "the loss is visible, with both counts: {transcript}"
    );
    let _ = k.child.kill();
}
