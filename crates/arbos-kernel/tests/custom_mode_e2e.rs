//! T3-03: `/mode <skill>` pins a skill to a chat; its SKILL.md heads the
//! prompt every turn until `/mode off`.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn notices(place: &Path) -> Vec<String> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "notice")
        .filter_map(|e| e["text"].as_str().map(str::to_owned))
        .collect()
}

fn wait_notice(place: &Path, needle: &str) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if notices(place).iter().any(|n| n.contains(needle)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn prompt_dump(place: &Path, xdg: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args(["prompt"])
        .arg(place)
        .args(["--agent", "root", "--dump"])
        .env("XDG_CONFIG_HOME", xdg)
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn slash_mode_pins_a_skill_to_the_chat_and_the_prompt_carries_it() {
    let mut k = start_kernel_replay_prepared(
        "custom-mode",
        "{\"content\":\"ok\"}\n",
        "",
        |place| {
            std::fs::create_dir_all(place.join(".arbos/skills/haiku")).unwrap();
            std::fs::write(
            place.join(".arbos/skills/haiku/SKILL.md"),
            "---\nname: haiku\ndescription: answer in haiku\n---\nAnswer every request as a haiku: three lines, 5-7-5.\n",
        )
        .unwrap();
        },
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let xdg = k.scratch.join("xdg");

    // Nothing pinned: the prompt has no Mode line.
    assert!(!prompt_dump(&k.place, &xdg).contains("Mode: haiku"));

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "/mode haiku"}));
    assert!(
        wait_notice(&k.place, "Mode: haiku is pinned to this chat"),
        "{:?}",
        notices(&k.place)
    );
    let md = std::fs::read_to_string(k.place.join(".arbos/agents/root/agent.md")).unwrap();
    assert!(md.contains("skill: haiku"), "{md}");
    let dump = prompt_dump(&k.place, &xdg);
    assert!(dump.contains("Mode: haiku (this skill is pinned"), "{dump}");
    assert!(dump.contains("Answer every request as a haiku"), "{dump}");
    // It was a setting, not a prompt: no turn ran for it.
    assert!(
        a.wait(Duration::from_secs(2), |f| f["type"] == "turn"
            && f["agent"] == "root")
            .is_none()
    );

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "/mode nosuch"}));
    assert!(
        wait_notice(&k.place, "no skill named \"nosuch\""),
        "{:?}",
        notices(&k.place)
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "/mode"}));
    assert!(
        wait_notice(&k.place, "Mode: haiku is pinned to this chat. `/mode off`"),
        "{:?}",
        notices(&k.place)
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "/mode off"}));
    assert!(
        wait_notice(&k.place, "Mode off: haiku is no longer pinned"),
        "{:?}",
        notices(&k.place)
    );
    assert!(
        !std::fs::read_to_string(k.place.join(".arbos/agents/root/agent.md"))
            .unwrap()
            .contains("skill:")
    );
    assert!(!prompt_dump(&k.place, &xdg).contains("Mode: haiku"));
    let _ = k.child.kill();
}
