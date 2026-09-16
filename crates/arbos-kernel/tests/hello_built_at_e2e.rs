//! Semver sat at 0.2.0 for weeks, so `hello` could not tell this morning's
//! kernel from last week's — the failure a self-updater has to see.
//! `hello` and the health reply now carry the short git sha and the
//! build time beside the version.

mod common;

use common::{Attach, start_kernel_replay};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[test]
fn hello_and_healthz_carry_the_git_sha_and_built_at() {
    let mut k = start_kernel_replay("hello-build", "");
    let mut a = Attach::connect(&k.url);
    let hello = a
        .wait(Duration::from_secs(5), |f| f["type"] == "hello")
        .expect("hello");
    let sha = hello["git_sha"].as_str().expect("git_sha on hello");
    let build = hello["built_at"].as_str().expect("built_at on hello");
    assert!(!sha.is_empty() && sha != "unknown", "{hello}");
    assert!(
        sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()),
        "{sha}"
    );
    // 2026-09-16T11:55Z
    assert!(
        build.len() == 17 && build.ends_with('Z') && &build[4..5] == "-" && &build[10..11] == "T",
        "{build}"
    );
    assert_eq!(hello["kernel"], env!("CARGO_PKG_VERSION"));

    let addr = k.url.trim_start_matches("tcp://");
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
    let json: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(json["git_sha"], sha);
    assert_eq!(json["built_at"], build);
    let _ = k.child.kill();
}
