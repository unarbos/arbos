//! A machine we did not choose: `~/.bash_profile` ends in `exec zsh` (the
//! way to get zsh where chsh is not allowed). The kernel ran every bash
//! call as `bash -lc`; the profile replaced bash before `-c` ran, and the
//! command never ran — exit 0, no output, a success on every step. Now the
//! login shell is probed once; when it is broken, commands run without the
//! profile and the main chat is told once what that means.

mod common;

use common::{Attach, scratch_dir, spawn_with_opts};
use std::time::Duration;

const REPLIES: &str = concat!(
    "{\"agent\":\"root\",\"content\":\"marking\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo ran-under-broken-profile > mark.txt; echo said-it\"}}]}\n",
    "{\"agent\":\"root\",\"content\":\"done\"}\n",
);

#[test]
fn a_profile_that_execs_another_shell_does_not_swallow_every_command() {
    let scratch = scratch_dir("login-shell-broken");
    // `exec sh` stands in for `exec zsh`: sh reads an empty stdin and
    // leaves, and bash never sees its -c.
    std::fs::write(scratch.join("home").join(".bash_profile"), "exec sh\n").unwrap();
    std::fs::write(
        scratch.join("xdg").join("arbos").join("config.toml"),
        "trace = false\n",
    )
    .unwrap();
    // Root needs bash: an old-style place.
    std::fs::create_dir_all(scratch.join("place").join(".arbos")).unwrap();
    std::fs::write(
        scratch.join("place").join(".arbos").join("project.toml"),
        "schema = 2\nname = \"a\"\n",
    )
    .unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, REPLIES).unwrap();
    let mut k = spawn_with_opts(
        scratch.clone(),
        &[
            "--provider",
            "replay",
            "--replies",
            replies.to_str().unwrap(),
        ],
        &[],
        false,
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "mark it"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    // Control on main 587515fd: mark.txt never written, the bash result
    // empty with exit 0. Now the command ran.
    assert!(
        common::wait_for(Duration::from_secs(10), || k
            .place
            .join("mark.txt")
            .exists()),
        "the command ran without the profile"
    );
    let root: Vec<serde_json::Value> =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
    let bash = root
        .iter()
        .find(|e| e["kind"] == "tool" && e["name"] == "bash")
        .unwrap_or_else(|| panic!("{root:#?}"));
    assert!(
        bash["body"].as_str().unwrap_or("").contains("said-it"),
        "{bash:#?}"
    );
    let notices: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "notice")
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.starts_with("Commands run without your login profile"))
        .collect();
    assert_eq!(notices.len(), 1, "{root:#?}");
    assert!(
        notices[0].contains("ARBOS_NO_LOGIN_SHELL=1"),
        "{}",
        notices[0]
    );
    assert!(notices[0].contains("exec"), "{}", notices[0]);
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("login_shell_broken"), "{log}");
    let _ = k.child.kill();
}
