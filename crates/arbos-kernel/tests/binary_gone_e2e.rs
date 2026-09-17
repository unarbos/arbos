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

/// ETXTBSY: the tests here run in parallel threads of one process, and
/// one's fork can hold another's freshly copied executable open for
/// writing for the instant between its fork and exec. Not the thing under
/// test; try again.
fn spawn_retrying(cmd: &mut std::process::Command) -> std::process::Child {
    for _ in 0..50 {
        match cmd.spawn() {
            Ok(c) => return c,
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("spawn: {e}"),
        }
    }
    panic!("spawn: text file busy for a second");
}

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
    let scratch = common::scratch_dir(tag);
    let bin = scratch.join("arbos-kernel");
    std::fs::copy(env!("CARGO_BIN_EXE_arbos-kernel"), &bin).unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let place = scratch.join("place");
    let xdg = scratch.join("xdg");
    let child = spawn_retrying(
        std::process::Command::new(&bin)
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
            .env("ARBOS_NO_REEXEC", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
    );
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

/// The app's update, not unlink-and-write: the whole directory holding
/// the kernel is renamed to a backup and a new directory with the new
/// build takes its place. The running kernel's inode travels with the
/// backup, so its own current path is a real, unchanged file — judged by
/// that, nothing happened. Judged by what is at the path it was started
/// from, it is replaced: `binary_gone: true`.
#[test]
fn a_kernel_whose_directory_was_renamed_to_a_backup_reads_gone() {
    let scratch = common::scratch_dir("binary-dir-rename");
    let app = scratch.join("Arbos.app");
    std::fs::create_dir_all(&app).unwrap();
    let bin = app.join("arbos-kernel");
    std::fs::copy(env!("CARGO_BIN_EXE_arbos-kernel"), &bin).unwrap();
    let replies = scratch.join("replies.jsonl");
    std::fs::write(&replies, "").unwrap();
    std::fs::write(scratch.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    let place = scratch.join("place");
    let child = spawn_retrying(
        std::process::Command::new(&bin)
            .arg("serve")
            .arg(&place)
            .args([
                "--provider",
                "replay",
                "--replies",
                replies.to_str().unwrap(),
            ])
            .env("XDG_CONFIG_HOME", scratch.join("xdg"))
            .env("HOME", scratch.join("home"))
            .env("ARBOS_NO_REEXEC", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
    );
    let pid = child.id();
    let kernel_json = place.join(".arbos/runtime/kernel.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let url = loop {
        if let Ok(text) = std::fs::read_to_string(&kernel_json)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            && v["pid"].as_u64() == Some(pid as u64)
        {
            break v["url"].as_str().unwrap().to_string();
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut k = common::Kernel {
        child,
        place: place.clone(),
        url,
        scratch: scratch.clone(),
    };
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .unwrap();
    assert!(hello.get("binary_gone").is_none(), "{hello}");

    // The app's swap: the directory becomes the backup; a new directory
    // with a new build (a distinct file) at the same path.
    std::fs::rename(&app, scratch.join("Arbos.app.backup")).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_arbos-kernel"), &bin).unwrap();
    let backup_bin = scratch.join("Arbos.app.backup").join("arbos-kernel");
    assert!(
        backup_bin.exists(),
        "the running inode travelled with the backup"
    );

    let mut b = Attach::connect(&k.url);
    let hello = b
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .unwrap();
    assert_eq!(
        hello["binary_gone"], true,
        "judged by the start path, not by where the inode went: {hello}"
    );
    assert_eq!(healthz(&k.url)["binary_gone"], true);
    let _ = k.child.kill();
}
