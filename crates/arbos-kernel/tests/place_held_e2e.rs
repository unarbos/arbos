//! The pod test, 2026-09-17: a stale kernel held a place's lock, and the
//! supervisor's replacement logged `Error: place already served` 1411
//! times over 32 minutes with nothing that named the holder and nothing
//! that said a person should look. Driven: twelve relaunches against a
//! held place, with the "once" clock moved so the heartbeat and the
//! escalation both happen — the place's kernel.log tells the story in
//! three kinds of line, not twelve copies of one; every relaunch exits 3
//! with the desktop's phrase on stderr; and the first start after the
//! holder goes says the place is free.

mod common;

use common::start_kernel_replay;
use std::path::Path;
use std::process::Command;

fn kernel_log(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/runtime/kernel.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn relaunch(k: &common::Kernel) -> (i32, String) {
    relaunch_env(k, &[])
}

fn relaunch_env(k: &common::Kernel, env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"));
    for (key, v) in env {
        cmd.env(key, v);
    }
    let out = cmd
        .arg("serve")
        .arg(&k.place)
        .env("XDG_CONFIG_HOME", k.scratch.join("xdg"))
        .env("HOME", k.scratch.join("home"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn a_held_place_is_said_once_then_beats_then_escalates_and_every_relaunch_exits_three() {
    let mut k = start_kernel_replay("place-held", "");
    let holder_pid = k.child.id();
    let record = k.place.join(".arbos/runtime/place-held.json");

    // Relaunch 1: the full line, naming the holder.
    let (code, err) = relaunch(&k);
    assert_eq!(code, 3, "{err}");
    assert!(
        err.contains("place already served"),
        "the desktop's phrase: {err}"
    );
    assert!(err.contains(&format!("pid {holder_pid}")), "{err}");
    // Relaunches 2..=5 within the minute: quiet in kernel.log, still 3 on
    // stderr with the phrase.
    for _ in 0..4 {
        let (code, err) = relaunch(&k);
        assert_eq!(code, 3);
        assert!(err.contains("place already served"), "{err}");
    }
    let held: Vec<_> = kernel_log(&k.place)
        .into_iter()
        .filter(|e| e["event"] == "place_held")
        .collect();
    assert_eq!(
        held.len(),
        1,
        "five refusals, one line in the log: {held:#?}"
    );
    let first = held[0]["detail"].as_str().unwrap();
    assert!(first.contains(&format!("pid {holder_pid}")), "{first}");
    assert!(first.contains("url tcp://"), "the holder's url: {first}");
    assert!(first.contains("exits 3"), "{first}");

    // A minute passes (the record's clock moved back): one heartbeat.
    let mut rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    rec["last_said_ms"] = serde_json::json!(rec["last_said_ms"].as_i64().unwrap() - 61_000);
    rec["first_ms"] = serde_json::json!(rec["first_ms"].as_i64().unwrap() - 61_000);
    std::fs::write(&record, rec.to_string()).unwrap();
    let (code, _) = relaunch(&k);
    assert_eq!(code, 3);
    let (code, _) = relaunch(&k);
    assert_eq!(code, 3);
    let held: Vec<_> = kernel_log(&k.place)
        .into_iter()
        .filter(|e| e["event"] == "place_held")
        .collect();
    assert_eq!(held.len(), 2, "one heartbeat for the minute: {held:#?}");
    assert!(
        held[1]["detail"]
            .as_str()
            .unwrap()
            .starts_with("still held after"),
        "{}",
        held[1]
    );
    assert!(
        held[1]["detail"]
            .as_str()
            .unwrap()
            .contains("start(s) refused"),
        "{}",
        held[1]
    );

    // Five minutes pass: the plain word that a person needs to look.
    let mut rec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    rec["first_ms"] = serde_json::json!(rec["first_ms"].as_i64().unwrap() - 300_000);
    std::fs::write(&record, rec.to_string()).unwrap();
    let (code, err) = relaunch(&k);
    assert_eq!(code, 3);
    assert!(err.contains("a person needs to look"), "{err}");
    let held: Vec<_> = kernel_log(&k.place)
        .into_iter()
        .filter(|e| e["event"] == "place_held")
        .collect();
    assert_eq!(held.len(), 3, "{held:#?}");
    assert_eq!(held[2]["level"], "error");
    let text = held[2]["detail"].as_str().unwrap();
    assert!(text.contains("a person needs to look"), "{text}");
    assert!(text.contains("arbos-kernel stop"), "{text}");
    // And the escalation is not repeated on the next relaunch.
    let (code, _) = relaunch(&k);
    assert_eq!(code, 3);
    assert_eq!(
        kernel_log(&k.place)
            .into_iter()
            .filter(|e| e["event"] == "place_held")
            .count(),
        3
    );

    // The holder goes: the next start serves, and says the place is free.
    let _ = k.child.kill();
    let _ = k.child.wait();
    let _ = std::fs::remove_file(k.place.join(".arbos/runtime/kernel.json"));
    let k2 = common::restart_replay(&mut k, "");
    let freed = kernel_log(&k2.place)
        .into_iter()
        .filter(|e| e["event"] == "place_freed")
        .count();
    assert_eq!(freed, 1, "the place is said to be free once");
    assert!(!record.exists(), "the held record is cleared");
    drop(k2);
}

/// qal-j19: `HeldRecord::save` was `let _ = write; let _ = rename`, so a
/// runtime folder that could not be written turned the say-once into the
/// long line on every relaunch, then the error-level escalation for
/// ever. Now the record falls back to the machine's temp folder when the
/// place cannot hold it, and when nowhere can, every start says one
/// short warn line and never the long form or the escalation.
#[cfg(unix)]
#[test]
fn a_record_that_cannot_be_kept_never_turns_the_refusal_into_spam() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let mut k = start_kernel_replay("place-held-ro", "");
    let runtime = k.place.join(".arbos/runtime");
    let tmp = k.scratch.join("tmp-rw");
    std::fs::create_dir_all(&tmp).unwrap();
    let ro = k.scratch.join("tmp-ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o555)).unwrap();
    let restore = |k: &common::Kernel| {
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = k;
    };

    // The place cannot hold the record; the temp folder can: the story
    // is still told once. Six relaunches, one long line.
    let tmp_s = tmp.display().to_string();
    let mut long = 0;
    let mut escalations = 0;
    for _ in 0..6 {
        let (code, err) = relaunch_env(&k, &[("TMPDIR", tmp_s.as_str())]);
        assert_eq!(code, 3, "{err}");
        assert!(err.contains("place already served"), "{err}");
        if err.contains("another kernel already serves") {
            long += 1;
        }
        if err.contains("a person needs to look") {
            escalations += 1;
        }
    }
    if long != 1 || escalations != 0 {
        restore(&k);
        panic!(
            "with the record in the temp folder: {long} long line(s), {escalations} escalation(s) in six relaunches"
        );
    }
    assert!(
        std::fs::read_dir(&tmp).unwrap().flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("arbos-place-held-")
        }),
        "the record went to the temp folder"
    );

    // Nowhere to keep it: the short form on every start, warn level,
    // never the long line, never the escalation — even with the record's
    // clock the long-form and escalation branches would have taken.
    let ro_s = ro.display().to_string();
    let mut short = 0;
    for _ in 0..6 {
        let (code, err) = relaunch_env(&k, &[("TMPDIR", ro_s.as_str())]);
        assert_eq!(code, 3, "{err}");
        assert!(err.contains("place already served"), "{err}");
        if err.contains("another kernel already serves") || err.contains("a person needs to look") {
            restore(&k);
            panic!("the long form or the escalation came with no record: {err}");
        }
        if err.contains("the held record could not be written") {
            short += 1;
        }
    }
    restore(&k);
    assert_eq!(
        short, 6,
        "each start says, in short, that the record could not be kept"
    );

    // The third shape, the realistic one: the folder was writable at
    // first and stopped being so. A stale record sits in runtime/; the
    // live one goes to the temp folder; the next start must read the
    // live one, not the escalation-ready stale one, on every relaunch.
    let record = runtime.join("place-held.json");
    let (code, _) = relaunch_env(&k, &[("TMPDIR", tmp_s.as_str())]);
    assert_eq!(code, 3);
    let mut stale: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    // Six minutes old, never escalated, said long ago — the shape that
    // would escalate on the next read if it were believed.
    stale["first_ms"] = serde_json::json!(stale["first_ms"].as_i64().unwrap() - 360_000);
    stale["last_said_ms"] = serde_json::json!(stale["last_said_ms"].as_i64().unwrap() - 360_000);
    stale["escalated"] = serde_json::json!(false);
    std::fs::write(&record, stale.to_string()).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut escalations = 0;
    for _ in 0..6 {
        let (code, err) = relaunch_env(&k, &[("TMPDIR", tmp_s.as_str())]);
        assert_eq!(code, 3, "{err}");
        if err.contains("a person needs to look") {
            escalations += 1;
        }
    }
    restore(&k);
    assert!(
        escalations <= 1,
        "the stale runtime copy was believed over the live temp copy: {escalations} escalations in six relaunches"
    );
    let _ = k.child.kill();
    let _ = k.child.wait();
}
