//! Process parity, slice 10: what Cursor's coordinator has in its context
//! every turn, the Arbos root has too — the date and machine (`Now:`),
//! the store path (`Store:`), a git snapshot (`Git:`), always-applied
//! project and user rules (`Rules`), and the messages waiting in its
//! inbox (`<<inbox>>`). Read off `arbos-kernel prompt --dump`, the exact
//! text a model call carries.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::process::Command;
use std::time::Duration;

fn git(dir: &std::path::Path, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

#[test]
fn the_roots_prompt_carries_cursors_context_inventory() {
    let replies = "{\"agent\":\"root\",\"content\":\"hi\"}\n";
    let mut k = start_kernel_replay_prepared("context-inventory", replies, "", |place| {
        std::fs::create_dir_all(place.join(".arbos/rules")).unwrap();
        std::fs::create_dir_all(place.join(".cursor/rules")).unwrap();
        std::fs::write(
            place.join(".arbos/project.toml"),
            "schema = 2\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".cursor/rules/style.mdc"),
            "---\ndescription: house style\nalwaysApply: true\n---\nWrite in Simplified Technical English.\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".cursor/rules/rust-only.mdc"),
            "---\nglobs: [\"*.rs\"]\nalwaysApply: false\n---\nNever unwrap in library code.\n",
        )
        .unwrap();
        std::fs::write(
            place.join(".arbos/rules/team.md"),
            "Merges are Jacob's call.\n",
        )
        .unwrap();
        std::fs::write(place.join("main.rs"), "fn main() {}\n").unwrap();
        git(place, &["init", "-q", "-b", "trunk"]);
        // The nested .arbos repo is the kernel's; the project's git must
        // not see it.
        std::fs::write(place.join(".gitignore"), ".arbos/\n").unwrap();
        git(place, &["add", "."]);
        git(place, &["commit", "-q", "-m", "start"]);
        std::fs::write(place.join("main.rs"), "fn main() { dirty() }\n").unwrap();
    });
    // The user's rules live in the user store.
    let user_rules = k.scratch.join("xdg/arbos/rules");
    std::fs::create_dir_all(&user_rules).unwrap();
    std::fs::write(
        user_rules.join("secrets.md"),
        "Secrets come from 1Password by item id, never Doppler.\n",
    )
    .unwrap();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // Two messages queued while nothing runs: they wait in the inbox for
    // the next turns, and the prompt says so.
    let place = arbos_core::Place::new(&k.place);
    for (kind, body, title) in [
        ("message", "the CI is red on main", ""),
        ("request", "add the hex codes", "Add the hex codes"),
    ] {
        let mut msg = arbos_core::inbox::Message::new("agent:watch-ci", kind, body);
        msg.wake = false;
        msg.title = title.to_string();
        arbos_core::inbox::deliver(&place, "root", &msg).unwrap();
    }

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
        .env("SHELL", "/bin/zsh")
        .output()
        .unwrap();
    let prompt = String::from_utf8_lossy(&out.stdout).to_string();
    let prompt = prompt.replace("\\n", "\n");

    // Store, date, machine.
    assert!(
        prompt.contains(&format!("Store: {}", k.place.join(".arbos").display())),
        "{prompt}"
    );
    let today = arbos_core::inbox::rfc3339(arbos_core::now_ms());
    let today = today.split('T').next().unwrap();
    assert!(prompt.contains(&format!("Now: {today}")), "{prompt}");
    assert!(
        prompt.contains("(UTC) · linux") && prompt.contains("shell zsh"),
        "{prompt}"
    );
    // Git snapshot.
    assert!(prompt.contains("Git: branch trunk at "), "{prompt}");
    assert!(prompt.contains(", 1 changed path(s)"), "{prompt}");
    // Rules: the always-applied .mdc, the project's, the user's; not the
    // glob-scoped one.
    assert!(prompt.contains("Rules (always applied"), "{prompt}");
    assert!(
        prompt.contains("Write in Simplified Technical English."),
        "{prompt}"
    );
    assert!(prompt.contains("Merges are Jacob's call."), "{prompt}");
    assert!(
        prompt.contains("Secrets come from 1Password by item id"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("Never unwrap in library code."),
        "a glob-scoped rule is not always applied: {prompt}"
    );
    assert!(
        !prompt.contains("alwaysApply"),
        "front matter is stripped: {prompt}"
    );
    // The inbox hint.
    assert!(
        prompt.contains("<<inbox>> messages waiting for you"),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "- from agent:watch-ci (message, read at your next turn): the CI is red on main"
        ),
        "{prompt}"
    );
    assert!(
        prompt
            .contains("- from agent:watch-ci (request, read at your next turn): Add the hex codes"),
        "a titled message shows its title: {prompt}"
    );
    let _ = k.child.kill();
}
