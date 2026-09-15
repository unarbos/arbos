//! Process parity, slice 9: Cursor's "spend caps checked mid-run; stop at
//! the cap and report". With `[spend] cap_usd` in project.toml the kernel
//! adds every turn's cost to `spend.toml`, tells the user once at 80 % and
//! once at the cap, and past the cap refuses workers' turns and `spawn`
//! while the user's own words to root still run.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::time::Duration;

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

#[test]
fn the_cap_is_counted_announced_and_enforced_but_the_user_can_still_talk_to_root() {
    let replies = concat!(
        // Turn 1: $0.85 of a $1.00 cap → the 80 % warning.
        "{\"agent\":\"root\",\"content\":\"thinking hard\",\"cost\":0.85}\n",
        // Turn 2: a worker; its turn costs $0.20 → over the cap, so its
        // done cannot open root's turn.
        "{\"agent\":\"root\",\"content\":\"one worker\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Cheap worker\",\"task\":\"a\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"started\"}\n",
        "{\"agent\":\"cheap-worker\",\"content\":\"done a\",\"cost\":0.2}\n",
        // Turn 3, the user's words: root still runs; its spawn is refused.
        "{\"agent\":\"root\",\"content\":\"trying another\",\"calls\":[{\"name\":\"spawn\",\"arguments\":{\"name\":\"Second worker\",\"task\":\"b\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Spend cap reached; raise it to go on.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("spend-cap", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n[spend]\ncap_usd = 1.0\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "think"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        common::wait_for(Duration::from_secs(10), || {
            std::fs::read_to_string(k.place.join(".arbos/user.md"))
                .unwrap_or_default()
                .contains("Spend is at $0.85")
        }),
        "the 80 % mark is announced once"
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "do a"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    // The worker's done would open a root turn; over the cap it is refused
    // with a notice on root's transcript instead.
    assert!(
        common::wait_for(Duration::from_secs(20), || {
            transcript(&k.place, "root").iter().any(|e| {
                e["kind"] == "notice"
                    && e["text"]
                        .as_str()
                        .unwrap_or("")
                        .contains("turn not started — spend cap reached")
            })
        }),
        "the done turn was refused: {:?}",
        transcript(&k.place, "root")
    );
    let spend = std::fs::read_to_string(k.place.join(".arbos/spend.toml")).unwrap();
    assert!(spend.contains("spent_usd = 1.05"), "{spend}");
    assert!(spend.contains("turns = 3"), "{spend}");
    assert!(spend.contains("capped = true"), "{spend}");
    // The user heard twice: at 80 % and at the cap.
    let user_md = std::fs::read_to_string(k.place.join(".arbos/user.md")).unwrap();
    assert!(
        user_md.contains("Spend is at $0.85 of the $1.00 cap (85 %)"),
        "{user_md}"
    );
    assert!(
        user_md.contains("Spend cap reached: $1.05 of $1.00 over "),
        "{user_md}"
    );

    // The user's own message to root still opens a turn; its spawn is
    // refused with the reason.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "now do b"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root = transcript(&k.place, "root");
    let spawns: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "spawn")
        .collect();
    assert_eq!(spawns.len(), 2, "{root:?}");
    assert!(spawns[0].get("error").is_none(), "{:#?}", spawns[0]);
    assert!(
        spawns[1]["error"]
            .as_str()
            .unwrap_or("")
            .contains("spend cap reached: $1.05 of $1.00"),
        "{:#?}",
        spawns[1]
    );
    assert!(!k.place.join(".arbos/agents/second-worker").exists());
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant"
                && e["text"] == "Spend cap reached; raise it to go on."),
        "{root:?}"
    );

    // The prompt says where spend stands.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args([
            "prompt",
            k.place.to_str().unwrap(),
            "--agent",
            "root",
            "--dump",
        ])
        .env("XDG_CONFIG_HOME", k.scratch.join("xdg"))
        .env("HOME", k.scratch.join("home"))
        .output()
        .unwrap();
    let prompt = String::from_utf8_lossy(&out.stdout);
    assert!(prompt.contains("Spend: $1.05 of the $1.00 cap"), "{prompt}");
    assert!(
        prompt.contains("cap reached: workers and subscriptions are refused"),
        "{prompt}"
    );

    // Raising the cap lifts the refusal: the next spawn goes through.
    std::fs::write(
        k.place.join(".arbos/project.toml"),
        "schema = 2\n[root]\nrole = \"coordinator\"\narchive_children = false\n[spend]\ncap_usd = 5.0\n",
    )
    .unwrap();
    let place = arbos_core::Place::new(&k.place);
    assert!(!arbos_core::spend::over_cap(&place));
    let _ = k.child.kill();
}
