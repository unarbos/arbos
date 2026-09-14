//! K-08 follow-up: a grant reaches the agent that asked and its workers,
//! not a sibling chat; the value is still redacted everywhere.

mod common;

use common::{Attach, Kernel, spawn_with_env};
use std::{path::PathBuf, time::Duration};

fn scratch(name: &str) -> PathBuf {
    let scratch = std::env::temp_dir().join(format!(
        "arbos-kernel-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    std::fs::create_dir_all(scratch.join("place/.arbos")).unwrap();
    std::fs::create_dir_all(scratch.join("xdg").join("arbos")).unwrap();
    std::fs::create_dir_all(scratch.join("home")).unwrap();
    scratch
}

fn bash_bodies(k: &Kernel, agent: &str) -> Vec<String> {
    std::fs::read_to_string(
        k.place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    .filter(|e| e["kind"] == "tool" && e["name"] == "bash")
    .map(|e| {
        format!(
            "{}{}",
            e["body"].as_str().unwrap_or(""),
            e["error"].as_str().unwrap_or("")
        )
    })
    .collect()
}

#[test]
fn a_grant_reaches_the_asker_and_its_worker_but_not_a_sibling_chat() {
    let scratch = scratch("secret-scope");
    // An old-style place: root keeps bash and spawns; workers keep the
    // folder (no archiving) so their transcripts can be read after.
    std::fs::write(
        scratch.join("place/.arbos/project.toml"),
        "schema = 2\nname = \"scope\"\n[root]\narchive_children = false\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("place/.arbos/secrets.toml"),
        "[secrets]\nTOKEN = \"env:TOKEN_SRC\"\n",
    )
    .unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    // A second top-level chat, made before the kernel starts.
    let place = arbos_core::Place::new(scratch.join("place"));
    arbos_core::bootstrap(&place).unwrap();
    let other = arbos_core::create_chat(&place).unwrap();
    let other_id = other.id.to_string();
    let replies = format!(
        concat!(
            "{{\"agent\":\"root\",\"content\":\"granting\",\"calls\":[{{\"name\":\"secret\",\"arguments\":{{\"action\":\"use\",\"name\":\"TOKEN\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"using\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"echo ROOT=${{TOKEN:-unset}}\"}}}},{{\"name\":\"spawn\",\"arguments\":{{\"name\":\"w1\",\"task\":\"echo the token\"}}}}]}}\n",
            "{{\"agent\":\"root\",\"content\":\"done\"}}\n",
            "{{\"agent\":\"w1\",\"content\":\"worker\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"echo W1=${{TOKEN:-unset}}\"}}}}]}}\n",
            "{{\"agent\":\"w1\",\"content\":\"done\"}}\n",
            "{{\"agent\":\"{other}\",\"content\":\"sibling\",\"calls\":[{{\"name\":\"bash\",\"arguments\":{{\"command\":\"echo OTHER=${{TOKEN:-unset}}\"}}}}]}}\n",
            "{{\"agent\":\"{other}\",\"content\":\"done\"}}\n",
        ),
        other = other_id
    );
    std::fs::write(scratch.join("replies.jsonl"), replies).unwrap();
    let replies_path = scratch.join("replies.jsonl").display().to_string();
    let mut k = spawn_with_env(
        scratch.clone(),
        &["--provider", "replay", "--replies", &replies_path],
        &[("TOKEN_SRC", "tok-scoped-0123456789abcdef")],
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "use the token"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while std::time::Instant::now() < deadline && bash_bodies(&k, "w1").is_empty() {
        std::thread::sleep(Duration::from_millis(200));
    }
    // Now the sibling, after the grant exists.
    a.send(serde_json::json!({"type": "user", "agent": other_id, "text": "try the token"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while std::time::Instant::now() < deadline && bash_bodies(&k, &other_id).is_empty() {
        std::thread::sleep(Duration::from_millis(200));
    }

    let root = bash_bodies(&k, "root");
    let w1 = bash_bodies(&k, "w1");
    let sibling = bash_bodies(&k, &other_id);
    assert!(root[0].contains("ROOT=[REDACTED:TOKEN]"), "{root:?}");
    assert!(
        w1[0].contains("W1=[REDACTED:TOKEN]"),
        "the worker inherits its parent's grant: {w1:?}"
    );
    assert!(
        sibling[0].contains("OTHER=unset"),
        "a sibling chat does not: {sibling:?}"
    );
    let everything = std::fs::read_dir(k.place.join(".arbos/agents"))
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("transcript.jsonl")).ok())
        .collect::<String>();
    assert!(
        !everything.contains("tok-scoped"),
        "the value never reaches a transcript"
    );
    let _ = k.child.kill();
}
