//! Kickoff item 11: the coordinator ran a whole session with the project
//! page as the template, then overwrote it with a page of its own that
//! had lost the front matter and the context link. Now: while the page
//! has no items, the coordinator's `<<plan>>` says the first state change
//! writes real items; a whole-page `write` keeps the page's head; `plan
//! set` gives the page its shape.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

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
fn the_empty_page_is_named_in_the_prompt_and_a_write_keeps_its_head() {
    let replies = concat!(
        // Turn 1: the coordinator writes the page whole, with its own title.
        "{\"agent\":\"root\",\"content\":\"noting the goal\",\"calls\":[{\"name\":\"write\",\"arguments\":{\"path\":\".arbos/notes.md\",\"contents\":\"# Project Status\\n\\n- [ ] Speak in full duplex — pending\\n\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"noted\"}\n",
        // Turn 2: the page as plan set shapes it.
        "{\"agent\":\"root\",\"content\":\"shaping\",\"calls\":[{\"name\":\"plan\",\"arguments\":{\"op\":\"set\",\"items\":[{\"section\":\"Voice\",\"label\":\"Full duplex\",\"target\":\"agents/voice\",\"readout\":\"worker running\"}]}}]}\n",
        "{\"agent\":\"root\",\"content\":\"shaped\"}\n",
    );
    let mut k = start_kernel_replay_prepared("page-template", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
    });
    let xdg = k.scratch.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let page = k.place.join(".arbos/notes.md");
    // Bootstrapped: the template, no items — and the prompt says so.
    let before = std::fs::read_to_string(&page).unwrap();
    assert!(
        before.starts_with("+++\nowner = \"root\"\n+++\n"),
        "{before}"
    );
    let prompt = prompt_dump(&k.place, &xdg);
    assert!(
        prompt.contains("still the empty template") && prompt.contains("plan set"),
        "the coordinator is told the page is empty and what fills it: {prompt}"
    );

    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "goal: full duplex"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let after_write = std::fs::read_to_string(&page).unwrap();
    assert!(
        after_write.starts_with("+++\nowner = \"root\"\n+++\n# Project Status\n"),
        "the head stays, the title is the model's: {after_write}"
    );
    assert!(
        after_write.contains("[project-context](docs/project-context.md)"),
        "the context link stays: {after_write}"
    );
    assert!(after_write.contains("- [ ] Speak in full duplex — pending"));
    let root =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        root.contains("the page's head"),
        "the write result says the head was kept: {root}"
    );
    // The page has an item now: the reminder is gone from the prompt.
    let prompt = prompt_dump(&k.place, &xdg);
    assert!(!prompt.contains("still the empty template"), "{prompt}");

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "shape it"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let shaped = std::fs::read_to_string(&page).unwrap();
    assert!(
        shaped.starts_with("+++\nowner = \"root\"\n+++\n"),
        "{shaped}"
    );
    assert!(
        shaped.contains("[project-context](docs/project-context.md)"),
        "{shaped}"
    );
    assert!(
        shaped.contains("## Voice\n- [ ] [Full duplex](agents/voice) — worker running"),
        "{shaped}"
    );
    let _ = k.child.kill();
}
