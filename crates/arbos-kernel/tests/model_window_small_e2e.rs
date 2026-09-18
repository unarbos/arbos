//! A first install on a model whose context is smaller than the place's
//! standing prompt (a small local model behind a compatible endpoint, a
//! cheap tier): every turn met "over budget … nothing old enough to
//! compact", naming a window and not the model. Said once at start, with
//! the model, its context, what the prompt needs, and where to change it.

mod common;

use common::start_kernel_with;
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

/// An OpenAI-shaped `/models` that lists one model with a tiny context.
fn small_model_server(context_length: u64) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut head = [0u8; 4096];
            let n = stream.read(&mut head).unwrap_or(0);
            let body = if head[..n].starts_with(b"GET") {
                format!(r#"{{"data":[{{"id":"tiny","context_length":{context_length}}}]}}"#)
            } else {
                r#"{"error":{"message":"not expected here"}}"#.to_string()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

fn notices(place: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "notice")
        .filter_map(|e| e["text"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_model_whose_context_is_smaller_than_the_standing_prompt_is_named_once_at_start() {
    let port = small_model_server(8_000);
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"tiny\"\ntrace = false\n"
    );
    let mut k = start_kernel_with("model-window-small", &config);
    assert!(
        common::wait_for(Duration::from_secs(10), || {
            notices(&k.place)
                .iter()
                .any(|t| t.starts_with("model tiny has a 8k context"))
        }),
        "said on root's transcript: {:?}",
        notices(&k.place)
    );
    let said: Vec<String> = notices(&k.place)
        .into_iter()
        .filter(|t| t.starts_with("model tiny has a"))
        .collect();
    assert_eq!(said.len(), 1);
    assert!(
        said[0].contains("standing prompt is ~") && said[0].contains("Settings › Model"),
        "{}",
        said[0]
    );
    let log = std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap();
    assert!(log.contains("\"event\":\"model_window_small\""), "{log}");
    let _ = k.child.kill();
    let _ = k.child.wait();

    // A second start on the same model: not said again.
    let _ = std::fs::remove_file(k.place.join(".arbos/runtime/kernel.json"));
    let mut k2 = common::spawn_with(k.scratch.clone(), &[]);
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(
        notices(&k2.place)
            .into_iter()
            .filter(|t| t.starts_with("model tiny has a"))
            .count(),
        1,
        "once per model"
    );
    let _ = k2.child.kill();
}

/// A model with room: nothing said.
#[test]
fn a_model_with_room_is_not_warned_about() {
    let port = small_model_server(200_000);
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"tiny\"\ntrace = false\n"
    );
    let mut k = start_kernel_with("model-window-fine", &config);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        notices(&k.place)
            .iter()
            .all(|t| !t.starts_with("model tiny has a")),
        "{:?}",
        notices(&k.place)
    );
    let _ = k.child.kill();
}
