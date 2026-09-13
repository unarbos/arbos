//! qa-017: the desktop deletes a chat with `rm -rf` on its folder while its
//! turn may still run. The turn used to keep appending, recreating the
//! folder as a ghost: a transcript with no agent.md that nothing lists.

mod common;

use common::{Attach, sigint, start_kernel_with, wait_exit};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

/// Lists one model, never answers a chat request: the turn stays running.
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
                    std::thread::sleep(Duration::from_secs(120));
                }
            });
        }
    });
    port
}

#[test]
fn a_chat_deleted_during_its_turn_stays_deleted() {
    let port = silent_model_server();
    let config = format!(
        "api_base = \"http://127.0.0.1:{port}/v1\"\napi_key = \"test-key\"\nmodel = \"mock\"\nwindow_tokens = 8000\ntrace = false\n"
    );
    let mut k = start_kernel_with("deleted", &config);
    let place = arbos_core::Place::new(&k.place);
    let chat = arbos_core::create_chat(&place).unwrap();
    let id = chat.id.as_str().to_string();
    let dir = place.agent_dir(&id);

    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    a.send(serde_json::json!({"type": "user", "agent": id, "text": "hello"}));
    assert!(a.wait_turn(&id, "running", Duration::from_secs(15)));
    std::thread::sleep(Duration::from_millis(500));

    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!dir.exists());
    std::thread::sleep(Duration::from_secs(1));

    // A graceful stop ends the running turn; ending it must not write.
    sigint(&k.child);
    assert_eq!(wait_exit(&mut k.child, Duration::from_secs(10)), Some(0));
    assert!(
        !dir.exists(),
        "the deleted chat came back as a ghost folder: {:?}",
        std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
    );
}
