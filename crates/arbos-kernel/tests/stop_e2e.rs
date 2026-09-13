//! qa-007: Ctrl-C while a turn runs must end that turn the way the stop
//! button does, so the folder does not look like a crash and the next
//! start does not silently resume a turn the user ended.

mod common;

use common::{Attach, sigint, start_kernel_with, wait_exit};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

/// An OpenAI-shaped server that lists one model and then never answers a
/// chat request, so a turn stays "running" until something stops it.
fn silent_model_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut head = [0u8; 4096];
                let n = stream.read(&mut head).unwrap_or(0);
                if head[..n].starts_with(b"GET") {
                    let body = r#"{"data":[{"id":"mock","context_length":8000}]}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                } else {
                    // Hold the request open; the kernel's stop must not wait on us.
                    std::thread::sleep(Duration::from_secs(120));
                }
            });
        }
    });
    port
}

fn kinds(place: &std::path::Path, agent: &str) -> Vec<String> {
    let path = place
        .join(".arbos")
        .join("agents")
        .join(agent)
        .join("transcript.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| v["kind"].as_str().unwrap_or("?").to_string())
        .collect()
}

#[test]
fn ctrl_c_during_a_turn_ends_it_on_the_transcript_and_closes_its_node() {
    let port = silent_model_server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"mock\"\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("stop", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "hello"}));
    assert!(a.wait_turn("root", "running", Duration::from_secs(10)));
    // The turn is now waiting on the model. Give it a moment to be there.
    std::thread::sleep(Duration::from_millis(500));

    sigint(&k.child);
    let code = wait_exit(&mut k.child, Duration::from_secs(8));
    if code.is_none() {
        let _ = k.child.kill();
    }
    assert_eq!(
        code,
        Some(0),
        "clean exit within 8s of Ctrl-C with a turn in flight"
    );

    let ks = kinds(&k.place, "root");
    assert_eq!(
        &ks[ks.len() - 2..],
        ["interrupted", "turn_complete"],
        "the running turn ends on the transcript: {ks:?}"
    );
    let transcript = std::fs::read_to_string(
        k.place
            .join(".arbos")
            .join("agents")
            .join("root")
            .join("transcript.jsonl"),
    )
    .unwrap();
    assert!(
        transcript.contains("kernel stopping"),
        "the interrupted line says why: {transcript}"
    );
    let plan = std::fs::read_to_string(
        k.place
            .join(".arbos")
            .join("agents")
            .join("root")
            .join("plan.jsonl"),
    )
    .unwrap();
    let last = plan.lines().last().unwrap();
    let node: serde_json::Value = serde_json::from_str(last).unwrap();
    assert_ne!(
        node["status"], "active",
        "the node is closed, not left active: {last}"
    );
    assert!(
        node["outcome"]
            .as_str()
            .unwrap_or("")
            .contains("kernel stopping"),
        "the node says it was stopped: {last}"
    );
    let attempts = std::fs::read_to_string(
        k.place
            .join(".arbos")
            .join("agents")
            .join("root")
            .join("attempts.jsonl"),
    )
    .unwrap();
    let last_attempt: serde_json::Value =
        serde_json::from_str(attempts.lines().last().unwrap()).unwrap();
    assert!(
        last_attempt["ended_ms"].is_number(),
        "the attempt ended: {last_attempt}"
    );

    // A restart does not resume the ended turn: the transcript stays put.
    let lines_before = kinds(&k.place, "root").len();
    let mut again = common::restart_kernel(&k, &config);
    std::thread::sleep(Duration::from_secs(2));
    sigint(&again.child);
    assert_eq!(wait_exit(&mut again.child, Duration::from_secs(8)), Some(0));
    assert_eq!(
        kinds(&k.place, "root").len(),
        lines_before,
        "an ended turn is not replayed on the next start"
    );
}
