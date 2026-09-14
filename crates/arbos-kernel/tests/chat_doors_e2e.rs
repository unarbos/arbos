//! K-04 chat doors: a Discord or Slack channel in and out of an agent,
//! against a local stand-in for each service's API. No token leaves the
//! process; none is real.

mod common;

use common::{Attach, spawn_with_env};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the stand-in saw: every request line, and every POST body.
#[derive(Default)]
struct Seen {
    requests: Vec<String>,
    posts: Vec<String>,
}

/// A tiny HTTP server that plays Discord or Slack: the first history
/// look is empty, later ones carry one human message and one bot line.
fn fake_service(kind: &'static str, seen: Arc<Mutex<Seen>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut looks = 0u32;
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = req.lines().next().unwrap_or("").to_string();
            let body_ix = req.find("\r\n\r\n").map(|i| i + 4).unwrap_or(n);
            let mut body = req[body_ix..].to_string();
            // A POST body may arrive in a second read.
            if line.starts_with("POST")
                && let Some(len) = req
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("Content-Length: ")
                            .or_else(|| l.strip_prefix("content-length: "))
                    })
                    .and_then(|v| v.trim().parse::<usize>().ok())
                && body.len() < len
            {
                let mut rest = vec![0u8; len - body.len()];
                let _ = stream.read_exact(&mut rest);
                body.push_str(&String::from_utf8_lossy(&rest));
            }
            {
                let mut s = seen.lock().unwrap();
                s.requests.push(format!(
                    "{line} || {}",
                    req.lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("authorization"))
                        .unwrap_or("")
                ));
                if line.starts_with("POST") && !line.contains("auth.test") {
                    s.posts.push(body.clone());
                }
            }
            let reply = if line.contains("/users/@me") || line.contains("auth.test") {
                match kind {
                    "discord" => r#"{"id":"bot1","username":"Arbos","bot":true}"#.to_string(),
                    _ => r#"{"ok":true,"user_id":"B1"}"#.to_string(),
                }
            } else if line.starts_with("GET") {
                looks += 1;
                match (kind, looks) {
                    ("discord", 1) => "[]".to_string(),
                    ("discord", 2) => r#"[{"id":"9000000000000000002","content":"beep","author":{"id":"bot1","username":"Arbos","bot":true}},{"id":"9000000000000000001","content":"what is 2+2?","author":{"id":"u1","username":"jacob"}}]"#.to_string(),
                    ("discord", _) => "[]".to_string(),
                    (_, 1) => r#"{"ok":true,"messages":[]}"#.to_string(),
                    (_, 2) => r#"{"ok":true,"messages":[{"ts":"1700000000.000200","bot_id":"B1","user":"B1","text":"beep"},{"ts":"1700000000.000100","user":"U1","text":"what is 2+2?"}]}"#.to_string(),
                    _ => r#"{"ok":true,"messages":[]}"#.to_string(),
                }
            } else {
                match kind {
                    "discord" => r#"{"id":"9000000000000000009"}"#.to_string(),
                    _ => r#"{"ok":true,"ts":"1700000001.000000"}"#.to_string(),
                }
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    port
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arbos-kernel-{name}-{}-{}",
        std::process::id(),
        arbos_core::now_ms()
    ));
    for sub in ["place/.arbos", "xdg/arbos", "home"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("xdg/arbos/config.toml"), "trace = false\n").unwrap();
    dir
}

fn transcript(place: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(place.join(".arbos/agents/root/transcript.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn run(kind: &'static str, channel: &str) -> (Arc<Mutex<Seen>>, Vec<serde_json::Value>, String) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let port = fake_service(kind, Arc::clone(&seen));
    let dir = scratch(&format!("door-{kind}"));
    std::fs::write(
        dir.join("place/.arbos/doors.toml"),
        format!(
            "[[door]]\nkind = \"{kind}\"\ntoken = \"env:DOOR_TOKEN_SRC\"\nchannels = [\"{channel}\"]\nevery = \"1s\"\napi_base = \"http://127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("replies.jsonl"),
        "{\"agent\":\"root\",\"content\":\"four\"}\n",
    )
    .unwrap();
    let replies = dir.join("replies.jsonl").display().to_string();
    let mut k = spawn_with_env(
        dir.clone(),
        &["--provider", "replay", "--replies", &replies],
        &[("DOOR_TOKEN_SRC", "door-token-0123456789abcdef")],
    );
    let mut a = Attach::connect(&k.url);
    assert!(
        a.wait(Duration::from_secs(5), |f| f["type"] == "snapshot")
            .is_some()
    );
    let start = Instant::now();
    let t = loop {
        let t = transcript(&k.place);
        let (posted, moved_on) = {
            let s = seen.lock().unwrap();
            (
                !s.posts.is_empty(),
                // One more look after the message: it asks past it.
                s.requests.iter().any(|r| {
                    r.contains("after=9000000000000000002")
                        || r.contains("oldest=1700000000.000200")
                }),
            )
        };
        if (t.iter().any(|e| e["kind"] == "turn_complete") && posted && moved_on)
            || start.elapsed() > Duration::from_secs(40)
        {
            break t;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    let _ = k.child.kill();
    (seen, t, log)
}

#[test]
fn a_discord_channel_message_wakes_root_and_the_answer_goes_back() {
    let (seen, t, log) = run("discord", "1234");
    let user = t
        .iter()
        .find(|e| e["kind"] == "user" && e["text"] == "what is 2+2?")
        .unwrap_or_else(|| panic!("the channel message became root's prompt: {t:?}\n{log}"));
    assert_eq!(user["channel"], "discord:1234");
    assert_eq!(user["device"], "jacob");
    // The bot's own line did not.
    assert!(
        !t.iter().any(|e| e["kind"] == "user" && e["text"] == "beep"),
        "{t:?}"
    );
    let seen = seen.lock().unwrap();
    assert!(
        seen.requests
            .iter()
            .any(|r| r.starts_with("GET /channels/1234/messages") && r.contains("Bot door-token")),
        "{:?}",
        seen.requests
    );
    // The reply went to the channel with the turn's last words.
    assert!(
        seen.posts
            .iter()
            .any(|p| p.contains("\"content\":\"four\"")),
        "{:?}",
        seen.posts
    );
    // Later looks only ask for what is newer than the last message.
    assert!(
        seen.requests
            .iter()
            .any(|r| r.contains("after=9000000000000000002")),
        "{:?}",
        seen.requests
    );
    assert!(log.contains("door_open"), "{log}");
    assert!(
        !log.contains("door-token-0123"),
        "the token never reaches the log"
    );
}

#[test]
fn a_slack_channel_works_through_the_same_door() {
    let (seen, t, log) = run("slack", "C0AB");
    let user = t
        .iter()
        .find(|e| e["kind"] == "user" && e["text"] == "what is 2+2?")
        .unwrap_or_else(|| panic!("the channel message became root's prompt: {t:?}\n{log}"));
    assert_eq!(user["channel"], "slack:C0AB");
    assert_eq!(user["device"], "U1");
    let seen = seen.lock().unwrap();
    assert!(
        seen.requests
            .iter()
            .any(|r| r.starts_with("GET /conversations.history?channel=C0AB")
                && r.contains("Bearer door-token")),
        "{:?}",
        seen.requests
    );
    assert!(
        seen.posts
            .iter()
            .any(|p| p.contains("\"channel\":\"C0AB\"") && p.contains("\"text\":\"four\"")),
        "{:?}",
        seen.posts
    );
    assert!(
        seen.requests
            .iter()
            .any(|r| r.contains("oldest=1700000000.000200")),
        "{:?}",
        seen.requests
    );
}
