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
