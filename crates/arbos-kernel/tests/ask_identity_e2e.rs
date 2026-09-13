//! qa-021: the kernel sends every ask twice (the live `ask` frame and the
//! transcript line through the tail). A client that took the second copy for
//! a new question answered the first with "" and the question resolved blank
//! 0.1 s after it was asked. `answer` carried only the agent, so any late or
//! duplicate answer resolved whatever was pending. Now an ask has an id.

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

/// OpenAI-shaped: lists one model; the first chat request returns an `ask`
/// tool call, every later one returns a short text.
fn asking_model_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let posts = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let posts = Arc::clone(&posts);
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let mut content_length = 0usize;
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    for line in head.lines() {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length: ")
                        {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    if head.starts_with("GET") || buf.len() - end - 4 >= content_length {
                        let (body, ctype) = if head.starts_with("GET") {
                            (
                                r#"{"data":[{"id":"mock","context_length":8000}]}"#.to_string(),
                                "application/json",
                            )
                        } else if posts.fetch_add(1, Ordering::SeqCst) == 0 {
                            (concat!(
                                "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_ask_1\",\"type\":\"function\",\"function\":{\"name\":\"ask\",\"arguments\":\"{\\\"question\\\":\\\"Which colour?\\\",\\\"options\\\":[\\\"teal\\\",\\\"red\\\"]}\"}}]}}]}\n\n",
                                "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                                "data: [DONE]\n\n"
                            ).to_string(), "text/event-stream")
                        } else {
                            (concat!(
                                "data: {\"id\":\"y\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"You chose teal.\"}}]}\n\n",
                                "data: {\"id\":\"y\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                                "data: [DONE]\n\n"
                            ).to_string(), "text/event-stream")
                        };
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        return;
                    }
                }
            });
        }
    });
    port
}

fn kinds(place: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(
        place
            .join(".arbos")
            .join("agents")
            .join("root")
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    .map(|v| v["kind"].as_str().unwrap_or("?").to_string())
    .collect()
}

#[test]
fn an_ask_has_an_id_and_only_a_matching_answer_resolves_it() {
    let port = asking_model_server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"mock\"\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("askid", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "ask me something"}));

    let ask = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .expect("an ask frame");
    let id = ask["id"]
        .as_str()
        .expect("the ask frame carries an id")
        .to_string();
    assert_eq!(id, "call_ask_1", "the id is the ask tool's call id");

    // An id this kernel never issued is a client that knows about ids and
    // got it wrong: refused, the question stays pending.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "", "id": "call_ask_1-stale"}));
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "error"
            && f["agent"] == "root")
            .is_some(),
        "an unknown ask id is refused"
    );
    // The real answer, with the id: resolves the question and the turn ends.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "teal", "id": id}));
    assert!(
        a.wait_turn("root", "idle", Duration::from_secs(30)),
        "turn never ended after the answer; transcript: {:?}",
        kinds(&k.place)
    );
    std::thread::sleep(Duration::from_millis(400));
    let text = std::fs::read_to_string(
        k.place
            .join(".arbos")
            .join("agents")
            .join("root")
            .join("transcript.jsonl"),
    )
    .unwrap();
    assert!(
        text.contains("\"call_id\":\"call_ask_1\""),
        "the transcript ask line carries the id"
    );
    assert_eq!(kinds(&k.place).iter().filter(|k| *k == "answer").count(), 1);
    assert!(
        text.contains("You chose teal."),
        "the turn used the real answer"
    );

    // The duplicate / late blank answer (the same ask seen twice): refused
    // with an error frame, and nothing more is recorded.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "", "id": id}));
    let err = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "error" && f["agent"] == "root"
        })
        .expect("a late answer is refused with an error frame");
    assert!(err["detail"].as_str().unwrap().contains("refused"), "{err}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(kinds(&k.place).iter().filter(|k| *k == "answer").count(), 1);
    let _ = k.child.kill();
}

#[test]
fn an_answer_with_nothing_pending_is_refused_and_a_blind_one_is_tolerated_once() {
    let port = asking_model_server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"mock\"\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("askid2", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // Nothing pending: refused, and no `answer` line appears.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "early"}));
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "error"
            && f["agent"] == "root")
            .is_some()
    );
    assert!(!kinds(&k.place).iter().any(|k| k == "answer"));

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "ask me something"}));
    let ask = a
        .wait(Duration::from_secs(30), |f| {
            f["type"] == "ask" && f["agent"] == "root"
        })
        .unwrap();
    let id = ask["id"].as_str().unwrap().to_string();

    // A blind answer (old client) while exactly one question is pending: accepted.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "red"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(kinds(&k.place).iter().filter(|k| *k == "answer").count(), 1);

    // The late answer with the now-resolved id: refused.
    a.send(serde_json::json!({"type": "answer", "agent": "root", "text": "", "id": id}));
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "error")
            .is_some()
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(kinds(&k.place).iter().filter(|k| *k == "answer").count(), 1);
    let _ = k.child.kill();
}
