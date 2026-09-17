//! F-46 (symmetry cycle 13): on a code task the coordinator edited
//! `main.py` and `math_utils.py` itself and met the kernel's refusal,
//! instead of spawning a worker. The contract said "spawn first"; the
//! model chose from the tool list. Now the tool list says it at the point
//! of choice: the coordinator's `write`/`edit`/`apply_patch`/`delete`
//! descriptions open with the project-store-only note, `spawn`'s with
//! "your first call on any request that changes code"; a worker's
//! descriptions are the tools' own. And the refusal, when it still comes,
//! carries the exact spawn call to make.

mod common;

use common::{Attach, start_kernel_with};
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
                                    r#"{"data":[{"id":"mock","context_length":128000}]}"#
                                        .to_string(),
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

fn tools_of_first_trace(agent_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut traces: Vec<_> = std::fs::read_dir(agent_dir.join("trace"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    traces.sort();
    let text = std::fs::read_to_string(&traces[0]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["request"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn description(tools: &[serde_json::Value], name: &str) -> String {
    tools
        .iter()
        .find(|t| t["function"]["name"] == name)
        .map(|t| {
            t["function"]["description"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_else(|| panic!("no tool {name}"))
}

#[test]
fn the_coordinators_tool_list_says_spawn_first_and_a_workers_does_not() {
    let port = tiny_model_server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"mock\"\nwindow_tokens = 0\ntrace = true\n"
    );
    let mut k = start_kernel_with("coordinator-spawn-first", &config);
    // Root is the coordinator by default in a project place.
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "fix the off-by-one in main.py"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let root_dir = k.place.join(".arbos/agents/root");
    assert!(common::wait_for(Duration::from_secs(5), || root_dir
        .join("trace")
        .is_dir()));
    let tools = tools_of_first_trace(&root_dir);
    assert!(!tools.is_empty(), "the request carries the tool list");
    for name in ["write", "edit", "apply_patch"] {
        let d = description(&tools, name);
        assert!(
            d.starts_with("COORDINATOR: project store only"),
            "{name}: {d}"
        );
        assert!(d.contains("call spawn first"), "{name}: {d}");
    }
    let spawn = description(&tools, "spawn");
    assert!(
        spawn.starts_with("COORDINATOR: your first call on any request that changes code"),
        "{spawn}"
    );
    assert!(!description(&tools, "read").contains("COORDINATOR"));

    // A worker (no role) reads the tools' own words.
    let mut w = arbos_core::Agent::root("worker");
    w.parent = Some(arbos_core::AgentId::new("root"));
    w.save(&k.place.join(".arbos/agents/worker")).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    a.send(serde_json::json!({"type": "user", "agent": "worker", "text": "fix the off-by-one in main.py"}));
    assert!(a.wait_turn("worker", "idle", Duration::from_secs(30)));
    let worker_dir = k.place.join(".arbos/agents/worker");
    assert!(common::wait_for(Duration::from_secs(5), || worker_dir
        .join("trace")
        .is_dir()));
    let tools = tools_of_first_trace(&worker_dir);
    for name in ["write", "edit", "apply_patch", "spawn"] {
        let d = description(&tools, name);
        assert!(!d.contains("COORDINATOR"), "{name}: {d}");
    }
    let _ = k.child.kill();
}
