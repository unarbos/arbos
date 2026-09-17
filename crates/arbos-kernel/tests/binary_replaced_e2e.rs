//! JB-6: on arboslife every spawn was refused with `start arbos-kernel
//! serve: No such file or directory` for two days. The worker daemon
//! started kernels from `current_exe()`, which on Linux is `/proc/self/exe`
//! — and once an update had replaced the binary by unlink-and-write, that
//! link read `… (deleted)`. The new build sat at the very path the daemon
//! was started from.
//!
//! Driven here on the real mechanism: a copy of the kernel is started, its
//! file is unlinked and rewritten while it runs, and it is asked which
//! binary it would start a kernel from. Control (the same copy, not
//! replaced): its own file, no note.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-binary-replaced-{tag}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// ETXTBSY: the two tests here run in parallel threads of one process,
/// and one's fork can hold the other's freshly copied executable open for
/// writing for the instant between its fork and exec. Not the thing
/// under test; try again.
fn spawn_retrying(cmd: &mut Command) -> std::process::Child {
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

fn copy_kernel(to: &std::path::Path) {
    std::fs::copy(env!("CARGO_BIN_EXE_arbos-kernel"), to).unwrap();
}

#[test]
fn a_kernel_whose_binary_was_replaced_under_it_starts_kernels_from_its_start_path() {
    let dir = scratch("replaced");
    let bin = dir.join("arbos-kernel");
    copy_kernel(&bin);
    let flag = dir.join("go");
    // Started by its path: the path is what it remembers.
    let child = spawn_retrying(
        Command::new(&bin)
            .args(["binary", "--wait-for", flag.to_str().unwrap()])
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    );
    std::thread::sleep(Duration::from_millis(300));
    // The update: unlink, then write the new build at the same path.
    std::fs::remove_file(&bin).unwrap();
    copy_kernel(&bin);
    std::fs::write(&flag, b"").unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let mut lines = stdout.lines();
    let chosen = PathBuf::from(lines.next().unwrap_or(""));
    assert_eq!(
        std::fs::canonicalize(&chosen).ok(),
        std::fs::canonicalize(&bin).ok(),
        "the new build at the start path: {stdout}"
    );
    let note = lines.next().unwrap_or("");
    assert!(
        note.contains("replaced or moved under it") && note.contains("restart the daemon"),
        "says what happened and what to do: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn control_a_kernel_whose_binary_stands_names_its_own_file_with_no_note() {
    let dir = scratch("standing");
    let bin = dir.join("arbos-kernel");
    copy_kernel(&bin);
    let out = spawn_retrying(
        Command::new(&bin)
            .arg("binary")
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .wait_with_output()
    .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let mut lines = stdout.lines();
    assert_eq!(
        std::fs::canonicalize(lines.next().unwrap_or("")).ok(),
        std::fs::canonicalize(&bin).ok(),
        "{stdout}"
    );
    assert!(
        lines.next().is_none(),
        "no note when nothing moved: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
