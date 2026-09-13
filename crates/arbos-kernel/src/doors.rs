//! Extra attaches on the same tree: MCP tools, Telegram, voice, refresh.

use anyhow::Result;
use arbos_core::{Place, Wake, WakeKind};
use serde_json::Value;
use std::{
    process::{Command, Stdio},
    sync::{Arc, mpsc},
};

use crate::{attach::Frame, hooks::KernelHooks};

/// Telegram long-poll: each message is an inbox node on root.
pub async fn telegram_loop(token: String, hooks: Arc<KernelHooks>) {
    let mut offset: i64 = 0;
    let client = reqwest::Client::new();
    loop {
        let url =
            format!("https://api.telegram.org/bot{token}/getUpdates?timeout=20&offset={offset}");
        let Ok(resp) = client.get(&url).send().await else {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        };
        let Ok(v) = resp.json::<Value>().await else {
            continue;
        };
        if let Some(arr) = v.get("result").and_then(|r| r.as_array()) {
            for upd in arr {
                if let Some(id) = upd.get("update_id").and_then(|i| i.as_i64()) {
                    offset = id + 1;
                }
                let Some(text) = upd
                    .pointer("/message/text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let _ = hooks.inbox("root", &text, "user", Vec::new());
            }
        }
    }
}

/// Voice: record to a wav via `rec`/`sox` if present; stop returns a path.
pub fn voice_start() -> Result<String> {
    let path = std::env::temp_dir().join("arbos-voice.wav");
    let _ = Command::new("rec")
        .args([
            path.to_str().unwrap_or("/tmp/arbos-voice.wav"),
            "trim",
            "0",
            "30",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    Ok(path.display().to_string())
}

pub fn voice_stop(hooks: &KernelHooks) -> Result<String> {
    let path = std::env::temp_dir().join("arbos-voice.wav");
    // Best effort: whisper.cpp or `say` invert — if a transcript file exists, wake.
    let txt = path.with_extension("txt");
    let text = if txt.exists() {
        std::fs::read_to_string(&txt)?
    } else {
        format!("voice file {}", path.display())
    };
    let _ = hooks.inbox("root", &text, "user", vec![path.display().to_string()]);
    Ok(text)
}

/// Refresh: wake root with a notice. Same tree, no second product.
pub fn refresh(place: &Place, wakes: &tokio::sync::mpsc::UnboundedSender<Wake>) {
    let _ = wakes.send(Wake::new("root", WakeKind::Serve, Some("refresh".into())));
    let _ = place;
}

pub fn spawn_telegram_if_configured(hooks: Arc<KernelHooks>) {
    let Ok(token) = std::env::var("ARBOS_TELEGRAM_TOKEN") else {
        return;
    };
    if token.is_empty() {
        return;
    }
    tokio::spawn(telegram_loop(token, hooks));
}

pub type DoorTx = mpsc::Sender<Frame>;
