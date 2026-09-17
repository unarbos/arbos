//! qa-036: a plain HTTP GET on the attach port used to get the socket
//! closed with no reply, which cloudflared and every uptime probe read as
//! 502 while the kernel was fine. `GET /` and `/healthz` answer 200 with
//! a small JSON; any other plain GET gets 426 Upgrade Required; a plain
//! TCP client (the desktop, the CLI) attaches as before.

mod common;

use common::{Attach, start_kernel_replay_prepared};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn http_get(url: &str, path: &str) -> (u16, String, String) {
    let addr = url.trim_start_matches("tcp://");
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: curl/8.0\r\nAccept: */*\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

#[test]
fn plain_gets_get_a_health_reply_or_426_and_tcp_clients_still_attach() {
    let mut k = start_kernel_replay_prepared("port-health", "", "", |place| {
        std::fs::create_dir_all(place.join(".arbos")).unwrap();
        std::fs::write(place.join(".arbos/project.toml"), "schema = 2\n").unwrap();
    });
    for path in ["/", "/healthz", "/healthz?probe=1"] {
        let (status, head, body) = http_get(&k.url, path);
        assert_eq!(status, 200, "{path}: {head}\n{body}");
        assert!(head.contains("Content-Type: application/json"), "{head}");
        let json: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert!(
            json["kernel"].as_str().is_some_and(|v| !v.is_empty()),
            "{json}"
        );
        assert_eq!(json["attach"], "websocket");
        assert_eq!(json["auth"], "loopback", "no clients configured here");
        assert!(json["protocol"].is_u64());
    }
    let (status, head, body) = http_get(&k.url, "/somewhere/else");
    assert_eq!(status, 426, "{head}\n{body}");
    assert!(head.contains("Upgrade: websocket"), "{head}");
    assert!(body.contains("attach protocol"), "{body}");
    // The refusal log is not spammed by probes.
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    assert!(
        !log.contains("attach_refused"),
        "a probe is not an attach refusal: {log}"
    );
    // A plain TCP client still attaches and gets its hello.
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "hello")
            .is_some()
    );
    let _ = k.child.kill();
}
