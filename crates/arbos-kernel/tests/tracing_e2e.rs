//! What a rollout needs from the kernel: a timestamped log on disk with
//! start/stop, every turn, and every refused frame; provider traces that
//! name the agent and the transcript line they produced; version and pid
//! in `kernel.json`.

mod common;

use common::{Attach, sigint, start_kernel_with, wait_exit};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

/// An OpenAI-shaped server: lists one model, answers every chat request
/// with a two-chunk stream that says "hello".
fn tiny_model_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read headers, then the declared body.
                let mut content_length = 0usize;
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        for line in head.lines() {
                            if let Some(v) = line
                                .strip_prefix("content-length: ")
                                .or_else(|| line.strip_prefix("Content-Length: "))
                            {
                                content_length = v.trim().parse().unwrap_or(0);
                            }
                        }
                        let have = buf.len() - end - 4;
                        if head.starts_with("GET") || have >= content_length {
                            let (body, ctype) = if head.starts_with("GET") {
                                (
                                    r#"{"data":[{"id":"mock","context_length":8000}]}"#.to_string(),
                                    "application/json",
                                )
                            } else {
                                (concat!(
                                    "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"}}]}\n\n",
                                    "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}\n\n",
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
                }
            });
        }
    });
    port
}

fn log_lines(place: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos").join("kernel.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn has(lines: &[serde_json::Value], event: &str, agent: Option<&str>) -> Option<serde_json::Value> {
    lines
        .iter()
        .find(|l| l["event"] == event && agent.is_none_or(|a| l["agent"] == a))
        .cloned()
}

#[test]
fn the_kernel_log_traces_and_kernel_json_tell_a_rollout_what_happened() {
    let port = tiny_model_server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"mock\"\nwindow_tokens = 8000\ntrace = true\n"
    );
    let mut k = start_kernel_with("tracing", &config);
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );

    // Gap 3: start is on disk with pid, version, git sha; kernel.json says the same.
    let lines = log_lines(&k.place);
    let start = has(&lines, "kernel_start", None).expect("kernel_start line");
    let detail = start["detail"].as_str().unwrap();
    assert!(
        detail.contains(&format!("pid={}", k.child.id())),
        "{detail}"
    );
    assert!(
        detail.contains("version=") && detail.contains("git="),
        "{detail}"
    );
    assert!(start["ts"].is_number());
    let kj: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(k.place.join(".arbos").join("kernel.json")).unwrap(),
    )
    .unwrap();
    assert!(kj["version"].is_string() && kj["git_sha"].is_string() && kj["started"].is_number());
    assert!(kj["log"].as_str().unwrap().ends_with("kernel.log"));

    // Gap 1: a refused frame is answered and logged, with the reason.
    a.send_raw("this is not a frame");
    let err = a
        .wait(Duration::from_secs(5), |f| f["type"] == "error")
        .expect("error frame for garbage");
    assert!(err["detail"].as_str().unwrap().contains("not a frame"));
    a.send(serde_json::json!({"type": "user", "agent": "nobody", "text": "hi"}));
    let err = a
        .wait(Duration::from_secs(5), |f| {
            f["type"] == "error" && f["agent"] == "nobody"
        })
        .expect("error frame for a missing agent");
    assert!(
        err["detail"].as_str().unwrap().contains("no agent"),
        "{err}"
    );
    let lines = log_lines(&k.place);
    assert!(
        lines
            .iter()
            .filter(|l| l["event"] == "frame_rejected")
            .count()
            >= 2,
        "{lines:#?}"
    );

    // Gap 2 and 3: a real turn writes turn_start/turn_end and a trace file
    // that names the agent and the transcript line of its assistant event.
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "say hello"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(300));
    let lines = log_lines(&k.place);
    assert!(
        has(&lines, "turn_start", Some("root")).is_some(),
        "{lines:#?}"
    );
    assert!(
        has(&lines, "turn_end", Some("root")).is_some(),
        "{lines:#?}"
    );

    let root = k.place.join(".arbos").join("agents").join("root");
    let transcript = std::fs::read_to_string(root.join("transcript.jsonl")).unwrap();
    let assistant_line = transcript
        .lines()
        .position(|l| l.contains("\"kind\":\"assistant\""))
        .expect("an assistant line")
        + 1;
    let traces: Vec<_> = std::fs::read_dir(root.join("trace"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(traces.len(), 1, "one model call, one trace file");
    let name = traces[0].file_name().to_string_lossy().to_string();
    assert!(
        name.ends_with(&format!("-L{assistant_line}.json")),
        "{name} vs line {assistant_line}"
    );
    let trace: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(traces[0].path()).unwrap()).unwrap();
    assert_eq!(trace["agent"], "root");
    assert_eq!(trace["purpose"], "turn");
    assert_eq!(trace["transcript_line"], assistant_line);
    assert!(trace["call_ids"].is_array());
    assert_eq!(trace["content"], "hello");

    // Gap 3: the stop is on disk too.
    sigint(&k.child);
    assert_eq!(wait_exit(&mut k.child, Duration::from_secs(8)), Some(0));
    let lines = log_lines(&k.place);
    assert!(has(&lines, "kernel_stop", None).is_some(), "{lines:#?}");
}
