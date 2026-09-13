//! A model that takes no image input never loses the user's picture: a
//! vision-capable fallback puts it into words, the transcript records an
//! `image_described` line (drawn inside the message card), and the turn
//! goes on with the selected model reading the words. No bare notice.

mod common;

use common::{Attach, start_kernel_with};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    time::Duration,
};

/// An OpenAI-shaped server with two models: `textonly` (its list says text
/// only; a chat with an image 404s the way OpenRouter does) and `seer`
/// (takes images; describes them). Every chat body is kept for the test.
fn two_model_server(bodies: Arc<Mutex<Vec<(String, String)>>>, list_modalities: bool) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let bodies = Arc::clone(&bodies);
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read the head, then the body up to Content-Length.
                loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = find(&buf, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if buf.len() >= pos + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                if text.starts_with("GET") {
                    let body = if list_modalities {
                        r#"{"data":[{"id":"textonly","context_length":8000,"architecture":{"input_modalities":["text"]}},{"id":"seer","context_length":8000,"architecture":{"input_modalities":["text","image"]}}]}"#
                    } else {
                        r#"{"data":[{"id":"textonly","context_length":8000},{"id":"seer","context_length":8000}]}"#
                    };
                    respond(&mut stream, 200, "application/json", body);
                    return;
                }
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let model = v["model"].as_str().unwrap_or("").to_string();
                let has_image = body.contains("image_url");
                bodies.lock().unwrap().push((model.clone(), body.clone()));
                if model == "textonly" && has_image {
                    respond(
                        &mut stream,
                        404,
                        "application/json",
                        r#"{"error":{"message":"No endpoints found that support image input","code":404}}"#,
                    );
                    return;
                }
                let reply = if model == "seer" {
                    "Image 1: a small red square on white, 8 by 8 pixels."
                } else if body.contains("described by seer") {
                    "I read the description: a small red square."
                } else {
                    "I saw no image."
                };
                let sse = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"choices":[{"delta":{"role":"assistant","content":reply}}]}),
                    serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"total_tokens":20}})
                );
                respond(&mut stream, 200, "text/event-stream", &sse);
            });
        }
    });
    port
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut std::net::TcpStream, status: u16, ctype: &str, body: &str) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

/// A 1x1 red PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn transcript(place: &std::path::Path) -> Vec<serde_json::Value> {
    let path = place
        .join(".arbos")
        .join("agents")
        .join("root")
        .join("transcript.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn a_text_only_model_gets_the_image_in_words_from_a_seeing_fallback() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let port = two_model_server(Arc::clone(&bodies), true);
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"textonly\"\nfallback_models = [\"seer\"]\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("describe", &config);
    std::fs::write(k.place.join("shot.png"), PNG).unwrap();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({
        "type": "user", "agent": "root", "text": "what is in this picture?",
        "attachments": ["shot.png"]
    }));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));

    let evs = transcript(&k.place);
    let described: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|e| e["kind"] == "image_described")
        .collect();
    assert_eq!(described.len(), 1, "one image_described line: {evs:#?}");
    assert_eq!(described[0]["model"], "seer");
    assert_eq!(described[0]["path"], "shot.png");
    assert!(
        described[0]["text"]
            .as_str()
            .unwrap()
            .contains("small red square")
    );
    // The turn's model answered from the words, and the model did not change.
    let last = evs.iter().rev().find(|e| e["kind"] == "assistant").unwrap();
    assert_eq!(last["text"], "I read the description: a small red square.");
    // Nothing about dropped images as a bare notice.
    assert!(
        !evs.iter().any(|e| e["kind"] == "notice"
            && e["text"]
                .as_str()
                .unwrap_or("")
                .contains("does not accept image input")),
        "{evs:#?}"
    );
    // The list said textonly takes no images, so the first call went to
    // the describer; no doomed call with pixels to textonly.
    let calls = bodies.lock().unwrap().clone();
    assert_eq!(
        calls[0].0,
        "seer",
        "{:?}",
        calls.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
    assert!(calls[0].1.contains("image_url"));
    assert!(
        calls
            .iter()
            .all(|(m, b)| !(m == "textonly" && b.contains("image_url")))
    );

    // A second turn on the same agent: the description stays in the
    // projection (the user line's image renders as its words), no new call
    // to the describer.
    let before = calls.len();
    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "and again?"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let calls = bodies.lock().unwrap().clone();
    let new: Vec<&(String, String)> = calls[before..].iter().collect();
    assert!(
        new.iter().all(|(m, _)| m == "textonly"),
        "{:?}",
        new.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
    assert!(new.iter().any(|(_, b)| b.contains("described by seer")));
    let _ = k.child.kill();
}

/// A per-turn model: the composer's "switch to <vision model> for this
/// turn" sends `model` on the user frame; that turn runs there and the
/// agent's own model is back for the next.
#[test]
fn the_user_frame_can_name_a_model_for_one_turn() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let port = two_model_server(Arc::clone(&bodies), true);
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"textonly\"\nfallback_models = [\"none\"]\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("turnmodel", &config);
    std::fs::write(k.place.join("shot.png"), PNG).unwrap();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({
        "type": "user", "agent": "root", "text": "look", "attachments": ["shot.png"], "model": "seer"
    }));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let calls = bodies.lock().unwrap().clone();
    assert!(!calls.is_empty());
    assert!(
        calls.iter().all(|(m, _)| m == "seer"),
        "{:?}",
        calls.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
    let evs = transcript(&k.place);
    assert!(
        !evs.iter().any(|e| e["kind"] == "image_described"),
        "{evs:#?}"
    );

    a.send(serde_json::json!({"type": "user", "agent": "root", "text": "plain words"}));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let calls = bodies.lock().unwrap().clone();
    assert_eq!(calls.last().unwrap().0, "textonly");
    let _ = k.child.kill();
}

/// A host whose list says nothing about modalities: the first call goes to
/// the selected model with pixels, the provider refuses, the describer
/// speaks, and the same model answers from the words.
#[test]
fn a_provider_that_refuses_images_gets_the_describe_path_after_the_first_call() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let port = two_model_server(Arc::clone(&bodies), false);
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"textonly\"\nvision_model = \"seer\"\nfallback_models = [\"none\"]\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("describe-reactive", &config);
    std::fs::write(k.place.join("shot.png"), PNG).unwrap();
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({
        "type": "user", "agent": "root", "text": "what is this?", "attachments": ["shot.png"]
    }));
    assert!(a.wait_turn("root", "idle", Duration::from_secs(30)));
    let calls = bodies.lock().unwrap().clone();
    let order: Vec<&str> = calls.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(order, vec!["textonly", "seer", "textonly"], "{order:?}");
    let evs = transcript(&k.place);
    assert!(
        evs.iter()
            .any(|e| e["kind"] == "image_described" && e["model"] == "seer")
    );
    let last = evs.iter().rev().find(|e| e["kind"] == "assistant").unwrap();
    assert_eq!(last["text"], "I read the description: a small red square.");
    let _ = k.child.kill();
}
