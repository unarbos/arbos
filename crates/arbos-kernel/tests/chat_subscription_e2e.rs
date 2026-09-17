//! Process parity, slice 4: Cursor's Slack channel/thread subscriptions.
//! `subscribe kind=chat channel=… [thread] [match]` wakes an agent that is
//! not the door's own when a human message lands in a channel a door
//! polls, as a message from `subscription:N`. Against a local stand-in for
//! Slack; no token is real.

mod common;

use common::{Attach, spawn_with_env};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A Slack stand-in: the first look is empty; the second carries a bot
/// line, a plain human message, a matching human message, and a threaded
/// reply.
fn fake_slack() -> u16 {
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
            let reply = if line.contains("auth.test") {
                r#"{"ok":true,"user_id":"B1"}"#.to_string()
            } else if line.starts_with("GET") {
                looks += 1;
                match looks {
                    1 => r#"{"ok":true,"messages":[]}"#.to_string(),
                    2 => concat!(
                        r#"{"ok":true,"messages":["#,
                        r#"{"ts":"1700000000.000400","user":"U2","text":"and in the thread: deploy is green","thread_ts":"1700000000.000100"},"#,
                        r#"{"ts":"1700000000.000300","user":"U1","text":"please deploy now"},"#,
                        r#"{"ts":"1700000000.000200","bot_id":"B1","user":"B1","text":"deploy beep"},"#,
                        r#"{"ts":"1700000000.000100","user":"U1","text":"hello there"}"#,
                        r#"]}"#
                    )
                    .to_string(),
                    _ => r#"{"ok":true,"messages":[]}"#.to_string(),
                }
            } else {
                r#"{"ok":true,"ts":"1700000001.000000"}"#.to_string()
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

fn transcript(place: &Path, agent: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        place
            .join(".arbos/agents")
            .join(agent)
            .join("transcript.jsonl"),
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| serde_json::from_str(l).ok())
    .collect()
}

fn texts(t: &[serde_json::Value]) -> String {
    t.iter()
        .filter_map(|e| e["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_chat_subscription_wakes_its_agent_on_matching_channel_messages() {
    let port = fake_slack();
    let dir = scratch("chat-sub");
    let place_dir = dir.join("place");
    // The watchers are standing agents, not one-shot workers: keep them.
    std::fs::write(
        place_dir.join(".arbos/project.toml"),
        "schema = 2\n[root]\narchive_children = false\n",
    )
    .unwrap();
    std::fs::write(
        place_dir.join(".arbos/doors.toml"),
        format!(
            "[[door]]\nkind = \"slack\"\ntoken = \"env:DOOR_TOKEN_SRC\"\nchannels = [\"C123\"]\nevery = \"1s\"\napi_base = \"http://127.0.0.1:{port}\"\n"
        ),
    )
    .unwrap();
    // Two agents besides root, authored: one subscribed to deploy talk in
    // the channel, one to the thread only.
    let place = arbos_core::Place::new(&place_dir);
    for (id, sub) in [
        (
            "watch-deploys",
            "id = 1\nkind = \"chat\"\nprompt = \"Someone asked for a deploy.\"\nchannel = \"C123\"\nmatch = \"deploy\"\ndeliver_to = \"agent\"\ncreated = \"2026-09-14T00:00:00Z\"\n",
        ),
        (
            "watch-the-thread",
            "id = 1\nkind = \"chat\"\nchannel = \"slack:C123\"\nthread = \"1700000000.000100\"\ndeliver_to = \"agent\"\ncreated = \"2026-09-14T00:00:00Z\"\n",
        ),
    ] {
        let mut agent = arbos_core::Agent::root(id);
        agent.parent = Some(arbos_core::AgentId::new("root"));
        agent.name = id.replace('-', " ");
        agent.save(&place.agent_dir(id)).unwrap();
        let subs = place.agent_dir(id).join("subscriptions");
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(subs.join("0001-chat.toml"), sub).unwrap();
    }
    std::fs::write(
        dir.join("replies.jsonl"),
        concat!(
            "{\"agent\":\"root\",\"content\":\"ok\"}\n",
            "{\"agent\":\"root\",\"content\":\"ok\"}\n",
            "{\"agent\":\"root\",\"content\":\"ok\"}\n",
            "{\"agent\":\"watch-deploys\",\"content\":\"on the deploy\"}\n",
            "{\"agent\":\"watch-deploys\",\"content\":\"on the deploy again\"}\n",
            "{\"agent\":\"watch-the-thread\",\"content\":\"thread noted\"}\n",
        ),
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
    loop {
        let deploys = transcript(&k.place, "watch-deploys");
        let thread = transcript(&k.place, "watch-the-thread");
        // The two deploy messages may open one turn (the second lands as
        // a steer into the first) or two.
        let heard = texts(&deploys);
        let done = heard.contains("please deploy now")
            && heard.contains("deploy is green")
            && deploys.iter().any(|e| e["kind"] == "turn_complete")
            && thread.iter().any(|e| e["kind"] == "turn_complete");
        if done || start.elapsed() > Duration::from_secs(40) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let log =
        std::fs::read_to_string(k.place.join(".arbos/runtime/kernel.log")).unwrap_or_default();
    let _ = k.child.kill();

    // The deploy watcher heard the two human "deploy" messages (channel and
    // thread), not "hello there", not the bot's line.
    let deploys = texts(&transcript(&k.place, "watch-deploys"));
    assert!(
        deploys.contains("[chat] U1 in slack:C123: please deploy now"),
        "{deploys}\n{log}"
    );
    assert!(
        deploys.contains(
            "[chat] U2 in slack:C123 (thread 1700000000.000100): and in the thread: deploy is green"
        ),
        "{deploys}"
    );
    assert!(
        deploys.contains("Someone asked for a deploy."),
        "the prompt rides along: {deploys}"
    );
    assert!(
        !deploys.contains("hello there"),
        "no match, no wake: {deploys}"
    );
    assert!(
        !deploys.contains("deploy beep"),
        "a bot's line never wakes anyone: {deploys}"
    );
    // The thread watcher heard the thread reply only.
    let thread = texts(&transcript(&k.place, "watch-the-thread"));
    assert!(thread.contains("deploy is green"), "{thread}");
    assert!(
        !thread.contains("please deploy now") && !thread.contains("hello there"),
        "{thread}"
    );
    // The wake came from the subscription, and the file remembers it.
    let deploys_t = transcript(&k.place, "watch-deploys");
    assert!(
        deploys_t
            .iter()
            .any(|e| e["kind"] == "wake" && e["text"].as_str().unwrap_or("").contains("[chat]")),
        "{deploys_t:?}"
    );
    let file = std::fs::read_to_string(
        k.place
            .join(".arbos/agents/watch-deploys/subscriptions/0001-chat.toml"),
    )
    .unwrap();
    assert!(file.contains("last_fired ="), "{file}");
    assert!(file.contains("last = \"U"), "{file}");
    assert!(
        !file.contains("next_due"),
        "a chat subscription has no schedule: {file}"
    );
    // The door's own agent still got every human message as the user's.
    let root = transcript(&k.place, "root");
    assert!(
        root.iter()
            .any(|e| e["kind"] == "user" && e["text"] == "hello there"),
        "{root:?}"
    );
    assert!(log.contains("chat_subscription_fired"), "{log}");
}
