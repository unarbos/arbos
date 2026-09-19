//! Desktop symmetry loop, cycle 53: Jev ruled "done, no change" on a turn
//! where nothing had been said yet, the chat model asked to say it
//! returned nothing, the empty-reply nudge fired, the second empty went to
//! the fallback, and the next Jev hop's model pick put the first model
//! back — 50 nudges and 37 fallbacks in two minutes, the window on
//! *Working* until the kernel was killed by hand. Now the verdict is the
//! answer when the say is empty, a fallback is not undone by a later pick,
//! and empty replies have a ceiling per turn.

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

/// The decisions door always says "done, no change"; every chat model
/// answers with no words. Counts chat calls and decisions.
fn server(chats: Arc<AtomicUsize>, decisions: Arc<AtomicUsize>, decision: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (chats, decisions) = (chats.clone(), decisions.clone());
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
                let respond = |stream: &mut std::net::TcpStream, ctype: &str, body: &str| {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.flush();
                };
                if text.starts_with("GET") {
                    respond(
                        &mut stream,
                        "application/json",
                        r#"{"data":[{"id":"zeta/empty","context_length":200000},{"id":"omega/empty","context_length":200000}]}"#,
                    );
                    return;
                }
                if text.starts_with("POST /alpha/decisions") {
                    decisions.fetch_add(1, Ordering::SeqCst);
                    respond(&mut stream, "application/json", decision);
                    return;
                }
                chats.fetch_add(1, Ordering::SeqCst);
                let sse = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"choices":[{"delta":{"role":"assistant","content":""}}]}),
                    serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"total_tokens":10}})
                );
                respond(&mut stream, "text/event-stream", &sse);
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
fn a_no_change_verdict_with_an_empty_say_ends_the_turn_with_the_verdict_as_the_answer() {
    let chats = Arc::new(AtomicUsize::new(0));
    let decisions = Arc::new(AtomicUsize::new(0));
    // Jev rules "done, no change" and, as the real one did, picks the
    // primary model on every hop.
    let port = server(
        chats.clone(),
        decisions.clone(),
        r#"{"answers":{"act":{"choice":"done"},"no_change":{"noul":1.0},"model":{"choice":"zeta/empty"}},"usage":{"input_tokens":10}}"#,
    );
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"zeta/empty\"\nfallback_models = [\"omega/empty\"]\njev = true\njev_model = \"jev/fake\"\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("no-change-loop", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "Run `ls /definitely-not-here` with bash and tell me the exact error text"}));
    // Control at main 05d15668, this fake: 1245 chat calls and 1246
    // decisions in 30 s, never idle. Now: one decision, one say, the
    // verdict as the answer.
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(30)),
        "the turn ends: chats={} decisions={} {:#?}",
        chats.load(Ordering::SeqCst),
        decisions.load(Ordering::SeqCst),
        transcript(&k.place)
    );
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    let answers: Vec<&serde_json::Value> = root
        .iter()
        .filter(|e| e["kind"] == "assistant" && !e["text"].as_str().unwrap_or("").is_empty())
        .collect();
    assert_eq!(answers.len(), 1, "{root:#?}");
    assert_eq!(
        answers[0]["text"],
        "No change needed: the tree already does what the request asks."
    );
    assert_eq!(
        root.iter().filter(|e| e["kind"] == "nudge").count(),
        0,
        "no nudge on Jev's silence: {root:#?}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "notice"
                && e["text"].as_str().unwrap_or("").starts_with("no change:")),
        "the verdict is not a chat line: {root:#?}"
    );
    assert!(
        !root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("returned nothing")),
        "no fallback on one silence: {root:#?}"
    );
    assert_eq!(chats.load(Ordering::SeqCst), 1, "one say, then the answer");
    assert_eq!(decisions.load(Ordering::SeqCst), 1);
    let _ = k.child.kill();
}

/// The same picks without the verdict: Jev sends every hop to the chat
/// model and names the primary each time; every model is empty. This
/// already ended on main 05d15668 (2 nudges, 1 fallback, the notice, 4
/// chat calls); pinned so the held fallback and the ceiling on empties per
/// turn keep it so.
#[test]
fn a_fallback_holds_against_jevs_pick_and_empty_replies_have_a_ceiling() {
    let chats = Arc::new(AtomicUsize::new(0));
    let decisions = Arc::new(AtomicUsize::new(0));
    let port = server(
        chats.clone(),
        decisions.clone(),
        r#"{"answers":{"act":{"choice":"llm"},"model":{"choice":"zeta/empty"}},"usage":{"input_tokens":10}}"#,
    );
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"zeta/empty\"\nfallback_models = [\"omega/empty\"]\njev = true\njev_model = \"jev/fake\"\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("empty-ceiling", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(30)),
        "the turn ends: chats={} decisions={}",
        chats.load(Ordering::SeqCst),
        decisions.load(Ordering::SeqCst),
    );
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    let fallbacks = root
        .iter()
        .filter(|e| {
            e["kind"] == "notice"
                && e["text"]
                    == "zeta/empty returned nothing twice, so omega/empty answers this turn."
        })
        .count();
    assert_eq!(fallbacks, 1, "one fallback, held: {root:#?}");
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["failed"] == true
            && e["text"]
                .as_str()
                .unwrap_or("")
                .starts_with("omega/empty returned nothing twice.")),
        "the second model's silence is said and ends the turn: {root:#?}"
    );
    // Two per model: the primary's two, the fallback's two.
    assert_eq!(chats.load(Ordering::SeqCst), 4, "{root:#?}");
    assert_eq!(
        root.iter().filter(|e| e["kind"] == "nudge").count(),
        2,
        "{root:#?}"
    );
    let _ = k.child.kill();
}
