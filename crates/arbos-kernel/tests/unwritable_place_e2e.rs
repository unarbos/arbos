//! A folder the person cannot write (a shared mount, another account's
//! folder, a read-only disk): the kernel's first write is its lock, so
//! it fails there — and said "cannot lock", our mechanism, not their
//! situation. Now it says what is wrong and what to do.

mod common;

use std::process::Command;

#[cfg(unix)]
#[test]
fn a_place_the_user_cannot_write_is_refused_with_the_situation_and_what_to_do() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let scratch = common::scratch_dir("unwritable-place");
    let place = scratch.join("place");
    std::fs::set_permissions(&place, std::fs::Permissions::from_mode(0o555)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_arbos-kernel"))
        .args([
            "serve",
            place.to_str().unwrap(),
            "--provider",
            "replay",
            "--replies",
            "/dev/null",
        ])
        .env("HOME", scratch.join("home"))
        .env("XDG_CONFIG_HOME", scratch.join("xdg"))
        .output()
        .unwrap();
    std::fs::set_permissions(&place, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("cannot start in") && err.contains("is not writable by this user"),
        "{err}"
    );
    assert!(
        err.contains("pick another folder, or make this one writable"),
        "what to do: {err}"
    );
    assert!(
        !err.contains("cannot lock"),
        "the mechanism is not the message: {err}"
    );
    // Nothing was created: no half-made store in a folder that is not ours.
    assert!(!place.join(".arbos").exists());
}
