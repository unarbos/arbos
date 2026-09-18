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
                        r#"{"data":[{"id":"blocked","context_length":200000},{"id":"open","context_length":200000},{"id":"openai/blocked","context_length":200000},{"id":"anthropic/open","context_length":200000},{"id":"slow/silent","context_length":200000},{"id":"zeta/empty","context_length":200000}]}"#,
                    );
                    return;
                }
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let model = v["model"].as_str().unwrap_or("").to_string();
                if model.ends_with("/empty") {
                    // A model that answers with no words at all.
                    let sse = format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        serde_json::json!({"choices":[{"delta":{"role":"assistant","content":""}}]}),
                        serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"total_tokens":10}})
                    );
                    respond(&mut stream, 200, "text/event-stream", &sse);
                    return;
                }
                if model.ends_with("/silent") {
                    // A provider that queues the request and says nothing.
                    std::thread::sleep(std::time::Duration::from_secs(40));
                    return;
                }
                if model.ends_with("/blocked") || model == "blocked" || model == "blocked-too" {
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
                    serde_json::json!({"choices":[{"delta":{"role":"assistant","content":format!("answered by {model}")}}]}),
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

/// Every string anywhere in these lines, however deep — a tool's error, a
/// notice, an assistant's words, a nested detail. Keys as well as values: a
/// provider that put its complaint in a field name would still be putting it
/// in the chat.
fn strings_of(events: &[serde_json::Value]) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => {
                for (k, value) in map {
                    out.push(k.clone());
                    walk(value, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    events.iter().for_each(|e| walk(e, &mut out));
    out
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
    let notice = root
        .iter()
        .find(|e| {
            e["kind"] == "notice"
                && e["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("open answers this turn")
        })
        .unwrap_or_else(|| panic!("{root:#?}"));
    assert_eq!(
        notice["text"], "blocked is not available to this key, so open answers this turn.",
        "one plain sentence"
    );
    // Every string the transcript holds, and no numbers. What this asserts is
    // that the provider's *words* never reach the chat, and `403` is three
    // digits: read as raw file text, it matched the millisecond clock on a
    // line — `"ts":1789575052403` — and failed the test at random, in about
    // one run in forty (run 35119788607, on a diff that touched no Rust).
    // Numbers are clocks, sizes and token counts; prose is what a provider
    // sends. So this walks the strings.
    let said = strings_of(&root).join("\n");
    // A negative assertion over nothing passes for the wrong reason, so prove
    // the walk found the chat before trusting it not to find the provider.
    assert!(
        said.contains("answered by open") && said.contains("hello"),
        "the walk reached the chat's own words: {said}"
    );
    assert!(
        !said.contains("Policy Violation")
            && !said.contains("403")
            && !said.contains("platform.openai.com"),
        "the provider's words stay out of the chat: {said}"
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
    assert!(
        text.starts_with("blocked-too is not available to this key."),
        "{text}"
    );
    assert!(text.contains("pick another model"), "{text}");
    assert!(
        !text.contains("billing") && !text.contains("Policy Violation"),
        "{text}"
    );
    let _ = k.child.kill();
}

/// A refused family is remembered: the next turn does not start on it,
/// and says so in one sentence; the fallback list drops it too. A new
/// project's kickoff probes the key first, so its very first line is
/// the greeting, not a provider's refusal.
#[test]
fn a_blocked_family_is_remembered_and_a_kickoff_probes_before_the_first_word() {
    let port = server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"openai/blocked\"\nfallback_models = [\"anthropic/open\"]\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("fallback-remembered", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    // The kickoff: the probe finds the block; the greeting comes from the
    // other family; the first notice is the plain one, and no "so …
    // answers this turn" fallback line appears at all.
    a.send(serde_json::json!({"type": "kickoff", "agent": "root"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    let first_notice = root
        .iter()
        .find(|e| e["kind"] == "notice")
        .unwrap_or_else(|| panic!("{root:#?}"));
    assert!(
        first_notice["text"]
            .as_str()
            .unwrap()
            .starts_with("This key cannot use openai models (openai/blocked was refused by the provider), so anthropic/open answers for now."),
        "{first_notice}"
    );
    assert!(
        !root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("answers this turn")),
        "no fallback happened in the turn itself: {root:#?}"
    );
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "answered by anthropic/open"),
        "{root:#?}"
    );
    // The block is on disk for this host, and the next turn skips the
    // family up front (one sentence, then the answer).
    let blocked =
        std::fs::read_to_string(k.scratch.join("xdg/arbos/runtime/blocked-models.json")).unwrap();
    assert!(blocked.contains("\"openai\""), "{blocked}");
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello again"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .filter(|e| e["kind"] == "turn_complete")
            .count()
            >= 2
    }));
    let root = transcript(&k.place);
    let answers: Vec<&str> = root
        .iter()
        .filter(|e| e["kind"] == "assistant")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert!(
        answers.iter().all(|t| *t == "answered by anthropic/open"),
        "{answers:?}"
    );
    assert!(
        !root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("answers this turn")),
        "the blocked model is never tried again: {root:#?}"
    );
    let _ = k.child.kill();
}

/// A model that never answers is given up in `first_byte_ms`, not the
/// two minutes of `stream_idle_ms`; the fallback takes over and the chat
/// says so plainly.
#[test]
fn a_silent_model_is_given_up_fast_and_the_fallback_answers() {
    let port = server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"slow/silent\"\nfallback_models = [\"anthropic/open\"]\nwindow_tokens = 0\nfirst_byte_ms = 3000\n"
    );
    let mut k = start_kernel_with("fallback-silent", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let started = std::time::Instant::now();
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "given up within first_byte_ms, not stream_idle: {:?}",
        started.elapsed()
    );
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["text"] == "slow/silent did not answer, so anthropic/open answers this turn."),
        "{root:#?}"
    );
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "answered by anthropic/open"),
        "{root:#?}"
    );
    let _ = k.child.kill();
}

/// qal-040: a model that returns nothing, twice, used to end the turn
/// with no words and no notice — a blank chat on a first-time user's
/// kickoff. With a fallback, the next model takes the turn (as after a
/// 403 or a silent first byte); alone, a readable notice says what
/// happened and that the first message starts the project as usual.
#[test]
fn two_empty_replies_go_to_the_fallback_or_end_with_a_readable_notice() {
    let port = server();
    // Alone: the notice.
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"zeta/empty\"\nfallback_models = [\"none\"]\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("empty-alone", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "kickoff", "agent": "root"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    let failed = root
        .iter()
        .find(|e| e["kind"] == "notice" && e["failed"] == true)
        .unwrap_or_else(|| panic!("a readable notice: {root:#?}"));
    let text = failed["text"].as_str().unwrap();
    assert!(
        text.starts_with("zeta/empty returned nothing twice."),
        "{text}"
    );
    assert!(text.contains("Settings › Model"), "{text}");
    assert!(
        text.contains("your first message starts the project as usual"),
        "{text}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "assistant" && !e["text"].as_str().unwrap_or("").is_empty()),
        "{root:#?}"
    );
    // The kickoff is not taken: a second kickoff frame runs another turn.
    a.send(serde_json::json!({"type": "kickoff", "agent": "root"}));
    assert!(
        a.wait(Duration::from_secs(10), |f| f["type"] == "turn"
            && f["agent"] == "root"
            && f["state"] == "running")
            .is_some(),
        "a second kickoff runs"
    );
    let _ = k.child.kill();

    // With a fallback: the next model answers, and the transcript says why.
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"k\"\nmodel = \"zeta/empty\"\nfallback_models = [\"anthropic/open\"]\nwindow_tokens = 0\n"
    );
    let mut k = start_kernel_with("empty-fallback", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(60)));
    assert!(common::wait_for(Duration::from_secs(5), || {
        transcript(&k.place)
            .iter()
            .any(|e| e["kind"] == "turn_complete")
    }));
    let root = transcript(&k.place);
    assert!(
        root.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                == "zeta/empty returned nothing twice, so anthropic/open answers this turn."),
        "{root:#?}"
    );
    assert!(
        root.iter()
            .any(|e| e["kind"] == "assistant" && e["text"] == "answered by anthropic/open"),
        "{root:#?}"
    );
    assert!(
        !root
            .iter()
            .any(|e| e["kind"] == "notice" && e["failed"] == true),
        "{root:#?}"
    );
    let _ = k.child.kill();
}
