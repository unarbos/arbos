//! A secret exported in the shell the kernel was started from must not
//! reach a job unless the secrets door grants it; `check` names the stray.

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

fn tool_bodies(k: &Kernel, name: &str) -> Vec<String> {
    std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "tool" && e["name"] == name)
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
fn a_stray_key_in_the_kernels_environment_never_reaches_a_job_and_a_granted_one_does() {
    let scratch = scratch("envallow");
    // An old place: root keeps bash (a new place's root is a coordinator).
    std::fs::write(
        scratch.join("place/.arbos/project.toml"),
        "schema = 2\nname = \"envallow\"\n",
    )
    .unwrap();
    // A declared secret the door can grant, sourced from the environment.
    std::fs::write(
        scratch.join("place/.arbos/secrets.toml"),
        "[secrets]\nGRANTED_TOKEN = \"env:GRANTED_TOKEN_SRC\"\n",
    )
    .unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let replies = concat!(
        "{\"agent\":\"root\",\"content\":\"env check\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo STRAY=${STRAY_API_KEY:-unset} LANG=${LANG:-unset} PATH_SET=${PATH:+yes}\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"granting\",\"calls\":[{\"name\":\"secret\",\"arguments\":{\"action\":\"use\",\"name\":\"GRANTED_TOKEN\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"using\",\"calls\":[{\"name\":\"bash\",\"arguments\":{\"command\":\"echo GRANTED=${GRANTED_TOKEN:-unset} STRAY=${STRAY_API_KEY:-unset}\"}}]}\n",
        "{\"agent\":\"root\",\"content\":\"done\"}\n",
    );
    std::fs::write(scratch.join("replies.jsonl"), replies).unwrap();
    let replies_path = scratch.join("replies.jsonl").display().to_string();
    let mut k = spawn_with_env(
        scratch.clone(),
        &["--provider", "replay", "--replies", &replies_path],
        &[
            ("STRAY_API_KEY", "sk-stray-should-not-leak-0123456789"),
            ("GRANTED_TOKEN_SRC", "tok-granted-0123456789abcdef"),
            ("LANG", "C.UTF-8"),
        ],
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "check the environment"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while std::time::Instant::now() < deadline && tool_bodies(&k, "bash").len() < 2 {
        std::thread::sleep(Duration::from_millis(200));
    }
    let bash = tool_bodies(&k, "bash");
    assert_eq!(bash.len(), 2, "{bash:#?}");
    // The stray never reached the job; the allowlist did.
    assert!(bash[0].contains("STRAY=unset"), "{}", bash[0]);
    assert!(bash[0].contains("LANG=C.UTF-8"), "{}", bash[0]);
    assert!(bash[0].contains("PATH_SET=yes"), "{}", bash[0]);
    // The granted one is there (its value redacted in what comes back);
    // the stray still is not.
    assert!(
        bash[1].contains("GRANTED=[REDACTED:GRANTED_TOKEN]"),
        "{}",
        bash[1]
    );
    assert!(bash[1].contains("STRAY=unset"), "{}", bash[1]);

    // kernel.json names the stray; `check` warns about it.
    let kj: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.json")).unwrap(),
    )
    .unwrap();
    let stray: Vec<&str> = kj["stray_secret_env"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(stray.contains(&"STRAY_API_KEY"), "{kj}");
    assert!(!stray.contains(&"GRANTED_TOKEN_SRC"), "{kj}");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args(["check", k.place.to_str().unwrap()])
        .env("XDG_CONFIG_HOME", scratch.join("xdg"))
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("STRAY_API_KEY"), "{text}");
    assert!(
        !text.contains("sk-stray"),
        "value leaked into check output: {text}"
    );
    let _ = k.child.kill();
}
