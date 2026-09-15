//! Process parity, slice 7: Cursor's user store. `remember scope=user`
//! writes `~/.config/arbos/preferences.md` (the index, each line with where
//! it applies), `workflows/<name>.md`, `principles/<name>.md`, and
//! `scripts/<name>`; the index names each file; the prompt carries the
//! index and the names; a revised entry replaces the old one.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::process::Command;
use std::time::Duration;

#[test]
fn remember_scope_user_fills_the_user_store_and_the_prompt_shows_it() {
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"keeping what you said\",\"calls\":[",
        "{\"name\":\"remember\",\"arguments\":{\"scope\":\"user\",\"text\":\"Short answers, one idea per chunk\",\"applies\":\"every project\"}},",
        "{\"name\":\"remember\",\"arguments\":{\"scope\":\"user\",\"kind\":\"workflow\",\"name\":\"Ship a PR\",\"text\":\"# Ship a PR\\n\\n1. Branch from main.\\n2. Tests green.\\n3. Open the PR as draft.\"}},",
        "{\"name\":\"remember\",\"arguments\":{\"scope\":\"user\",\"kind\":\"principle\",\"name\":\"hold-merges\",\"text\":\"Hold merges for the user. Applies: every repo. Stop: once the user says merge.\"}},",
        "{\"name\":\"remember\",\"arguments\":{\"scope\":\"user\",\"kind\":\"script\",\"name\":\"green.sh\",\"text\":\"#!/bin/sh\\ncargo test --workspace\"}}",
        "]}\n",
        "{\"agent\":\"root\",\"content\":\"Kept.\"}\n",
        "{\"agent\":\"root\",\"content\":\"revising\",\"calls\":[{\"name\":\"remember\",\"arguments\":{\"scope\":\"user\",\"kind\":\"workflow\",\"name\":\"ship-a-pr\",\"text\":\"# Ship a PR\\n\\n1. Branch from main.\\n2. Tests green.\\n3. Open the PR ready for review.\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"Revised.\"}\n",
    );
    let mut k = start_kernel_replay_prepared("user-store", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
    });
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "remember how I like things"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    let store = k.scratch.join("xdg/arbos");
    let prefs = std::fs::read_to_string(store.join("preferences.md")).unwrap();
    assert!(prefs.starts_with("# Preferences"), "{prefs}");
    assert!(
        prefs.contains("- Short answers, one idea per chunk (applies: every project)"),
        "{prefs}"
    );
    assert!(
        prefs.contains("- workflow: [ship-a-pr](workflows/ship-a-pr.md) — 1. Branch from main."),
        "{prefs}"
    );
    assert!(
        prefs.contains(
            "- principle: [hold-merges](principles/hold-merges.md) — Hold merges for the user."
        ),
        "{prefs}"
    );
    assert!(
        prefs.contains("- script: [green](scripts/green.sh) — cargo test --workspace"),
        "{prefs}"
    );
    let wf = std::fs::read_to_string(store.join("workflows/ship-a-pr.md")).unwrap();
    assert!(wf.contains("Open the PR as draft."), "{wf}");
    assert!(store.join("principles/hold-merges.md").is_file());
    let script = store.join("scripts/green.sh");
    assert!(script.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            std::fs::metadata(&script).unwrap().permissions().mode() & 0o111 != 0,
            "executable"
        );
    }

    // The prompt carries the index and the names.
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
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
    assert!(
        prompt.contains("Preferences (user, every place; remember scope=user)"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Short answers, one idea per chunk"),
        "{prompt}"
    );
    assert!(
        prompt.contains("workflow ship-a-pr (") && prompt.contains("principle hold-merges ("),
        "{prompt}"
    );
    assert!(prompt.contains("script green ("), "{prompt}");

    // A revision replaces the file and its index line; nothing stacks.
    a.send(
        serde_json::json!({"type": "user", "agent": "root", "text": "PRs go up ready, not draft"}),
    );
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let wf = std::fs::read_to_string(store.join("workflows/ship-a-pr.md")).unwrap();
    assert!(
        wf.contains("ready for review") && !wf.contains("as draft"),
        "{wf}"
    );
    let prefs = std::fs::read_to_string(store.join("preferences.md")).unwrap();
    assert_eq!(prefs.matches("[ship-a-pr]").count(), 1, "{prefs}");
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let bodies: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "tool" && e["name"] == "remember")
        .filter_map(|e| e["body"].as_str())
        .collect();
    assert!(
        bodies
            .iter()
            .any(|b| b.starts_with("saved workflow ship-a-pr")),
        "{bodies:?}"
    );
    assert!(
        bodies
            .iter()
            .any(|b| b.starts_with("revised workflow ship-a-pr")),
        "{bodies:?}"
    );
    let _ = k.child.kill();
}
