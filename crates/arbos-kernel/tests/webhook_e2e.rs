//! The webhook door: an HTTP POST on the attach port becomes an inbox
//! message. Loopback needs no token (this machine); the token path is
//! exercised by hand against a `--bind 0.0.0.0` kernel (QA note).

mod common;

use common::{Attach, start_kernel_replay};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn post(url: &str, path: &str, body: &str, content_type: &str) -> (u16, String) {
    let addr = url.trim_start_matches("tcp://");
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let status: u16 = out
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = out.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

#[test]
fn a_post_on_the_attach_port_is_a_message_for_the_agent() {
    let mut k = start_kernel_replay("webhook", "");
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // Slack's shape: a JSON object with `text`; the other fields ride along.
    let (status, body) = post(
        &k.url,
        "/hook/root",
        r##"{"text":"deploy finished","channel":"#ops"}"##,
        "application/json",
    );
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("\"ok\":true") && body.contains("webhook:local"),
        "{body}"
    );
    // An outside event, not the user's own words: it lands as a `say`
    // from `webhook:<client>`, like the GitHub door's lines.
    let said = a
        .wait(Duration::from_secs(10), |f| {
            f["type"] == "event"
                && f["agent"] == "root"
                && f["event"]["kind"] == "say"
                && f["event"]["from"] == "webhook:local"
        })
        .expect("the hook's words reach the agent as a say line");
    let text = said["event"]["text"].as_str().unwrap_or("");
    assert!(text.starts_with("deploy finished"), "{text}");
    assert!(text.contains("#ops"), "the extra fields follow: {text}");

    // Plain text bodies are the message as they are.
    let (status, _) = post(&k.url, "/hook/root", "build green", "text/plain");
    assert_eq!(status, 200);

    // Wrong path, unknown agent, empty body.
    assert_eq!(post(&k.url, "/other", "x", "text/plain").0, 404);
    assert_eq!(post(&k.url, "/hook/nobody", "x", "text/plain").0, 404);
    assert_eq!(post(&k.url, "/hook/root", "   ", "text/plain").0, 400);

    // The attach socket still takes a plain client after the hooks.
    let mut b = Attach::connect(&k.url);
    assert!(
        b.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let _ = k.child.kill();
}
