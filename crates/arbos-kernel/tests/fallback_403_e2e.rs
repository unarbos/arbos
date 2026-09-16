//! 2026-09-16: an OpenRouter key blocked for `openai/*` returns 403 for
//! every one of those models; a 403 used to end the turn ("Check billing
//! or access for this key") even with a fallback configured, so a
//! transient failure of the primary — or a primary the key cannot call —
//! became a hard stop. A 403 now falls through to the next model; the
//! transcript says who answered. A key refused everywhere still fails,
//! with words that name the model, not the billing.

mod common;

use common::{Attach, start_kernel_with};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

/// Two models: `blocked` answers every chat with 403 "this user has been
/// blocked"; `open` answers.
fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if head.starts_with("GET") || buf.len() >= pos + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let respond = |stream: &mut std::net::TcpStream,
                               status: u16,
                               ctype: &str,
                               body: &str| {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.flush();
                };
                if text.starts_with("GET") {
                    respond(
                        &mut stream,
                        200,
                        "application/json",
                        r#"{"data":[{"id":"blocked","context_length":8000},{"id":"open","context_length":8000}]}"#,
                    );
                    return;
                }
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let model = v["model"].as_str().unwrap_or("").to_string();
                if model == "blocked" || model == "blocked-too" {
                    respond(
                        &mut stream,
                        403,
                        "application/json",
                        r#"{"error":{"message":"Policy Violation: this user has been blocked","code":403}}"#,
                    );
                    return;
                }
                let sse = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"choices":[{"delta":{"role":"assistant","content":"answered by open"}}]}),
                    serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"total_tokens":20}})
                );
                respond(&mut stream, 200, "text/event-stream", &sse);
            });
        }
    });
    port
}

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_403_on_the_primary_falls_through_to_the_fallback() {
    let port = server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"blocked\"\nfallback_models = [\"open\"]\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("fallback-403", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "answered by open"),
        "{root:#?}"
    );
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .starts_with("switched to open for this turn: blocked")),
        "{root:#?}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "notice" && e["failed"] == true),
        "no failed notice: {root:#?}"
    );
    let _ = k.child.kill();
}

#[test]
fn a_key_refused_everywhere_still_fails_and_names_the_model() {
    let port = server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"blocked\"\nfallback_models = [\"blocked-too\"]\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("fallback-403-all", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    let failed = root
        .iter()
        .find(|e| e["kind"] == "notice" && e["failed"] == true)
        .unwrap_or_else(|| panic!("{root:#?}"));
    let text = failed["text"].as_str().unwrap();
    assert!(text.contains("blocked-too"), "{text}");
    assert!(text.contains("may not call this model"), "{text}");
    assert!(!text.contains("billing"), "{text}");
    let _ = k.child.kill();
}
