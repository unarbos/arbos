//! Mesh sweep, 2026-09-17: seven processes on two machines ran an image
//! whose file was gone — replaced under them by an update — for up to
//! four days, answering `hello` and registering as healthy. A worker
//! daemon in that state refused every spawn for 5.5 hours. Nothing they
//! sent said so. Now a kernel whose binary is gone says `binary_gone:
//! true` on `hello` and `/healthz` (and on `register`, through the same
//! function), computed live at every send; the control says nothing.

mod common;

use common::Attach;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

fn healthz(url: &str) -> serde_json::Value {
    let addr = url.trim_start_matches("tcp://");
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET /healthz HTTP/1.1\r\nHost: x\r\nUser-Agent: curl/8.0\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let body = out.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body.trim()).unwrap()
}

/// A kernel served from a copy of the binary at `bin`, so the copy can be
/// replaced under it.
fn start_from_copy(tag: &str) -> (common::Kernel, PathBuf) {
    start_from_copy_with(tag, &[("ARBOS_NO_REEXEC", "1")])
}

/// `env` on the kernel: the tests of the *report* keep the kernel on its
/// old image with `ARBOS_NO_REEXEC`; the test of the restart does not.
fn start_from_copy_with(tag: &str, env: &[(&str, &str)]) -> (common::Kernel, PathBuf) {
    let scratch = common::scratch_dir(tag);
    let bin = scratch.join("arbos-kernel");
    std::fs::copy(env!("CARGO_BIN_EXE_arbos-kernel"), &bin).unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let place = scratch.join("place");
    let xdg = scratch.join("xdg");
    let mut cmd = std::process::Command::new(&bin);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd
        .arg("serve")
        .arg(&place)
        .args([
            "--provider",
            "replay",
            "--replies",
            replies.to_str().unwrap(),
        ])
        .env("XDG_CONFIG_HOME", &xdg)
        .env("HOME", scratch.join("home"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let kernel_json = place.join(".arbos/runtime/kernel.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let url = loop {
        if let Ok(text) = std::fs::read_to_string(&kernel_json)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            && v["pid"].as_u64() == Some(child.id() as u64)
        {
            break v["url"].as_str().unwrap().to_string();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "kernel never wrote kernel.json"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    (
        common::Kernel {
            child,
            place,
            url,
            scratch,
        },
        bin,
    )
}

#[test]
fn a_kernel_whose_binary_was_replaced_under_it_says_so_on_hello_and_healthz() {
    let (mut k, bin) = start_from_copy("binary-gone");
    // Before: nothing to say, and no key for an old client to trip on.
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .unwrap();
    assert!(hello.get("binary_gone").is_none(), "{hello}");
    assert_eq!(healthz(&k.url)["binary_gone"], false);

    // The update as the installer does it: a new build staged beside,
    // then renamed over the same path. The path still exists and holds
    // the new file; the process keeps serving the old image. (A check on
    // the path alone would say "not gone" here — the macOS shape; the
    // kernel compares the file's identity with the one it started from.)
    std::fs::copy(
        env!("CARGO_BIN_EXE_arbos-kernel"),
        bin.with_extension("new"),
    )
    .unwrap();
    std::fs::rename(bin.with_extension("new"), &bin).unwrap();
    assert!(bin.exists());

    let mut b = Attach::connect(&k.url);
    let hello = b
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .unwrap();
    assert_eq!(
        hello["binary_gone"], true,
        "computed live, not at start: {hello}"
    );
    assert_eq!(healthz(&k.url)["binary_gone"], true);
    let _ = k.child.kill();
}

#[test]
fn control_a_kernel_whose_binary_stands_says_nothing() {
    let (mut k, _bin) = start_from_copy("binary-stands");
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .unwrap();
    assert!(hello.get("binary_gone").is_none(), "{hello}");
    assert_eq!(healthz(&k.url)["binary_gone"], false);
    let _ = k.child.kill();
}

/// The product answer for a kernel with no supervisor (subnet120,
/// started detached with init as its parent): when its file is replaced
/// under it and nothing is in flight, it execs onto the new file — same
/// pid, same place, clients reconnect — instead of serving stale code
/// until a person notices.
#[test]
fn a_kernel_whose_binary_was_replaced_restarts_onto_the_new_one_when_idle() {
    let (mut k, bin) = start_from_copy_with("binary-reexec", &[]);
    let pid = k.child.id();
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .unwrap();
    let served_from = hello["git_sha"].as_str().unwrap_or("").to_string();
    let started_before: i64 = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.json")).unwrap(),
    )
    .unwrap()["started"]
        .as_i64()
        .unwrap();
    // The update: the same build staged and renamed over the path (a
    // different build would do; the same one proves the mechanism and
    // keeps the test's kernel testable).
    std::fs::copy(
        env!("CARGO_BIN_EXE_arbos-kernel"),
        bin.with_extension("new"),
    )
    .unwrap();
    std::fs::rename(bin.with_extension("new"), &bin).unwrap();

    // Within two ticks: binary_gone said, then the re-exec. kernel.json
    // is rewritten by the new image with a later `started` and the same
    // pid.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let restarted = loop {
        let text =
            std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.json")).unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            && v["pid"].as_u64() == Some(pid as u64)
            && v["started"].as_i64().unwrap_or(0) > started_before
        {
            break v;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the kernel never restarted"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    let url = restarted["url"].as_str().unwrap().to_string();
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("\"event\":\"binary_gone\""), "{log}");
    assert!(log.contains("\"event\":\"reexec\""), "{log}");
    assert_eq!(
        log.matches("\"event\":\"kernel_start\"").count(),
        2,
        "the new image started in the same process: {log}"
    );
    // The new image runs the file at its path: not gone.
    let mut b = Attach::connect(&url);
    let hello = b
        .wait(Duration::from_secs(10), |f| f["type"] == "hello")
        .unwrap();
    assert!(hello.get("binary_gone").is_none(), "{hello}");
    assert_eq!(
        hello["git_sha"], served_from,
        "(the same build was installed)"
    );
    let transcript =
        std::fs::read_to_string(k.place.join(".arbos/agents/root/transcript.jsonl")).unwrap();
    assert!(
        transcript.contains("restarting onto the new build"),
        "{transcript}"
    );
    let _ = k.child.kill();
}
