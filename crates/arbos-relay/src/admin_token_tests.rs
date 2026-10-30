//! The generated admin token is a credential, so it must not reach the log.
//!
//! When `ARBOS_RELAY_ADMIN_TOKEN` is unset the relay mints an admin token for the operator. That
//! token opens all of `/admin/*`. It used to be written into the startup log at INFO, which under
//! systemd means journald, where it persists for the journal's retention and is readable by
//! everyone in the `systemd-journal` group. Every self-host in `docs/self-host.md` and every dev
//! deploy that omits the variable hit that path.
//!
//! It now goes to a 0600 file in the data directory and the log carries the path.

use std::os::unix::fs::PermissionsExt;

use arbos_core::ids::ulid;

use crate::generated_admin_token;

#[test]
fn generated_admin_token_lands_in_an_owner_only_file() {
    let dir = std::env::temp_dir().join(format!("arbos-admin-token-test-{}", ulid()));
    let (token, path) = generated_admin_token(&dir).expect("mint");

    assert!(
        token.starts_with("adm_"),
        "admin tokens keep the adm_ prefix so a leaked one is identifiable"
    );
    assert_eq!(path, dir.join("admin-token"));

    let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the admin token file must be owner-only; anything wider hands the admin surface \
         to every account on the host"
    );

    let on_disk = std::fs::read_to_string(&path).expect("read");
    assert_eq!(
        on_disk.trim_end(),
        token,
        "the file holds the token the relay uses"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generated_admin_token_is_not_formatted_into_a_log_line() {
    // A regression guard on the call site itself: the startup path may name the file, never the
    // value. Checked against the source because the value only exists inside `main`'s future and
    // a tracing subscriber cannot be installed twice in one test binary.
    let main_rs = include_str!("main.rs");
    assert!(
        !main_rs.contains("generated admin token: {"),
        "main.rs interpolates the generated admin token into a log line again"
    );
}
