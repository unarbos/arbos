//! Desktop symmetry loop, cycle 54: with a dead primary and a live
//! fallback, "<dead> rejected the request, so <live> answers this turn."
//! landed once per step — three times in one kickoff turn — because Jev
//! names a model on every hop and its pick put the dead one back. And the
//! window's chip named the dead model throughout: nothing told it which
//! model answered. Now a fallback holds for the turn, and `turn_complete`
//! carries the model that answered.

mod common;

use common::{Attach, start_kernel_with};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

/// `dead/x` answers 400; `live/y` calls bash on its first chat and answers
/// in words on its second. The decisions door sends every hop to the
/// chat model and names the primary each time.
fn server(live_calls: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let live_calls = live_calls.clone();
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
                        r#"{"data":[{"id":"dead/x","context_length":200000},{"id":"live/y","context_length":200000}]}"#,
                    );
                    return;
                }
                if text.starts_with("POST /alpha/decisions") {
                    respond(
                        &mut stream,
                        200,
                        "application/json",
                        r#"{"answers":{"act":{"choice":"llm"},"model":{"choice":"dead/x"}},"usage":{"input_tokens":10}}"#,
                    );
                    return;
                }
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let model = v["model"].as_str().unwrap_or("").to_string();
                if model == "dead/x" {
                    respond(
                        &mut stream,
                        400,
                        "application/json",
                        r#"{"error":{"message":"model not found","code":400}}"#,
                    );
                    return;
                }
                let n = live_calls.fetch_add(1, Ordering::SeqCst);
                let sse = if n == 0 {
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        serde_json::json!({"choices":[{"delta":{"role":"assistant","content":"","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"echo hi\"}"}}]}}]}),
                        serde_json::json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"total_tokens":20}})
                    )
                } else {
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        serde_json::json!({"choices":[{"delta":{"role":"assistant","content":"hi, said live/y"}}]}),
                        serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"total_tokens":20}})
                    )
                };
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
fn a_fallback_is_said_once_per_turn_and_turn_complete_names_the_model_that_answered() {
    let live_calls = Arc::new(AtomicUsize::new(0));
    let port = server(live_calls.clone());
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"dead/x\"\nfallback_models = [\"live/y\"]\njev = true\njev_model = \"jev/fake\"\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("fallback-once", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "say hi"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    // Two chat steps (a tool call, then words), both by the fallback.
    assert_eq!(live_calls.load(Ordering::SeqCst), 2, "{root:#?}");
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "hi, said live/y"),
        "{root:#?}"
    );
    let fallbacks = root
        .iter()
        .filter(|e| {
            e["kind"] == "notice"
                && e["text"] == "dead/x rejected the request, so live/y answers this turn."
        })
        .count();
    // Control on main 959e5960: 2 — one per step.
    assert_eq!(fallbacks, 1, "one plain sentence, once per turn: {root:#?}");
    let done = root
        .iter()
        .find(|e| e["kind"] == "turn_complete")
        .expect("turn_complete");
    assert_eq!(done["model"], "live/y", "who answered: {done:#?}");
    let _ = k.child.kill();
}
