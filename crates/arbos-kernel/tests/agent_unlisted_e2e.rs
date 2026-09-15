//! ba2262db79: a folder under `agents/` with no `agent.md` (a harness
//! probe, a half-made spawn, a hand-made scratch dir) is not an agent, so
//! `list_agents` leaves it out — and until now nothing said so. The kernel
//! names each such folder once at boot in the kernel log, keeps listing
//! the real agents, and root's own pre-bootstrap folder is not one.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::time::Duration;

#[test]
fn a_folder_without_agent_md_is_named_at_boot_and_left_out_of_the_roster() {
    let mut k = start_kernel_replay_prepared("agent-unlisted", "", "", |place| {
        std::fs::create_dir_all(place.join(".arbos/agents/agentA")).unwrap();
        std::fs::write(
            place.join(".arbos/agents/agentA/mechanism.md"),
            "a probe wrote here\n",
        )
        .unwrap();
        // A second stray folder, empty: named on its own line.
        std::fs::create_dir_all(place.join(".arbos/agents/scratch")).unwrap();
    });
    let mut a = Attach::connect(&k.url);
    let tree = a
        .wait(Duration::from_secs(10), |f| f["type"] == "tree")
        .expect("a tree frame follows attach");
    let names: Vec<String> = tree["tree"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x["id"].as_str())
        .map(str::to_owned)
        .collect();
    assert!(names.iter().any(|n| n == "root"), "root is listed: {names:?}");
    assert!(
        !names.iter().any(|n| n == "agentA" || n == "scratch"),
        "a folder without a readable agent.md is not listed: {names:?}"
    );

    let log_path = k.place.join(".arbos/runtime/kernel.log");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let log = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.matches("agent_unlisted").count() >= 2 || std::time::Instant::now() > deadline {
            break log;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        log.contains("agent_unlisted") && log.contains("agents/agentA: no agent.md"),
        "the folder is named at boot: {log}"
    );
    assert!(
        log.contains("agents/scratch: no agent.md"),
        "each stray folder gets its own line: {log}"
    );
    assert!(
        !log.contains("agents/root"),
        "root's own folder is never reported: {log}"
    );
    let _ = k.child.kill();
}
