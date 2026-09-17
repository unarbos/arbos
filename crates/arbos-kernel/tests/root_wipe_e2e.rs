//! qal-j15 / ra-01: `cd / && rm -rf *` ran with no ask and no refusal —
//! the old guard read the `cd` and the `rm` as separate pieces — and
//! deleted the project's own store seven times. Driven through the bash
//! tool's own path, in auto mode and in ask mode: a removal of a home
//! tree, however spelled, is refused with the reason, no card is shown,
//! no job starts, and the tree is still there.
//!
//! The home here is the kernel's `$HOME`, a scratch folder the harness
//! sets, so a guard that failed would cost this test its sentinel and not
//! the machine its files. The filesystem-root and system-tree spellings
//! (`cd / && rm -rf *`, `cd /usr && rm -rf *`, `find / -delete`) are the
//! same code path and are pinned by `tools::wipe::tests` on the string;
//! they are not run here even refused, because a regression would run
//! them for real on the box that runs the suite.

mod common;

use common::{Attach, start_kernel_replay, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

const SPELLINGS: &[&str] = &[
    "cd ~ && rm -rf *",
    "rm -rf \"$HOME\"/*",
    "cd $HOME; rm -rf ./*",
    "sh -c 'cd ~ && rm -rf *'",
    "find ~ -delete",
    "cd ~/.. && rm -rf home",
];

fn replies() -> String {
    let mut s = String::new();
    for cmd in SPELLINGS {
        let call = serde_json::json!({
            "agent": "root",
            "content": "cleaning up",
            "calls": [{"name": "bash", "arguments": {"command": cmd, "description": "Clean up"}}]
        });
        s.push_str(&call.to_string());
        s.push('\n');
    }
    s.push_str("{\"agent\":\"root\",\"content\":\"All refused.\"}\n");
    s
}

fn tool_events(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool" && e["name"] == "bash" && e["ended"].is_number())
        .collect()
}

fn home_of(k: &common::Kernel) -> std::path::PathBuf {
    k.place.parent().unwrap().join("home")
}

fn drive(k: &mut common::Kernel, label: &str) {
    let home = home_of(k);
    std::fs::create_dir_all(home.join("Documents")).unwrap();
    std::fs::write(home.join("Documents/thesis.txt"), "years of work\n").unwrap();
    let mut a = Attach::connect(&k.url);
    let _ = a.wait(Duration::from_secs(5), |f| f["type"] == "hello");
    a.send(serde_json::json!({"type":"user","agent":"root","text":"clean up","attachments":[]}));
    let mut asked = Vec::new();
    let idle = a.wait(Duration::from_secs(90), |f| {
        if f["type"] == "ask" && f["agent"] == "root" {
            asked.push(f.clone());
        }
        f["type"] == "turn" && f["agent"] == "root" && f["state"] == "idle"
    });
    assert!(idle.is_some(), "{label}: the turn ends");
    assert!(
        asked.is_empty(),
        "{label}: a home wipe is never a card: {asked:?}"
    );

    let tools = tool_events(&k.place);
    assert_eq!(
        tools.len(),
        SPELLINGS.len(),
        "{label}: one result per spelling: {tools:#?}"
    );
    for (cmd, t) in SPELLINGS.iter().zip(&tools) {
        let err = t["error"].as_str().unwrap_or("");
        assert!(
            err.contains("refused") && err.contains("never runs a removal"),
            "{label}: {cmd:?} must be refused with the reason, got error={err:?} output={:?}",
            t["output"]
        );
        assert!(
            err.contains(&home.display().to_string()),
            "{label}: the reason names the tree: {err}"
        );
    }
    assert!(
        home.join("Documents/thesis.txt").exists(),
        "{label}: the home tree stands"
    );
    let jobs = k.place.join(".arbos/agents/root/jobs");
    let started = std::fs::read_dir(&jobs)
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    assert_eq!(started, 0, "{label}: no job started for a refused command");
    let _ = k.child.kill();
}

#[test]
fn a_home_wipe_however_spelled_is_refused_in_auto_mode_no_card_no_job() {
    let mut k = start_kernel_replay("root-wipe-auto", &replies());
    drive(&mut k, "auto");
}

#[test]
fn a_home_wipe_however_spelled_is_refused_in_ask_mode_too_never_a_card() {
    let mut k = start_kernel_replay_prepared("root-wipe-ask", &replies(), "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\nname = \"a\"\n\n[root]\npermission = \"ask\"\n",
        )
        .unwrap();
    });
    drive(&mut k, "ask");
}
