//! The board socket (ADR-0042): one WebSocket per project kernel.
//!
//! This window listens for `command` frames from the `board` tool and
//! `show`. It does not post snapshots. The Mac app owns layout; last
//! writer wins on the kernel.

use crate::{agent::acp, kernel, model::place::Place};
use anyhow::{Result, anyhow};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, Utf8Bytes},
};

const RECONNECT: Duration = Duration::from_millis(800);
/// A kernel with no HTTP gateway has no board socket to offer; look again
/// now and then rather than every second.
const NO_GATEWAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub cards: Vec<Card>,
    pub view: View,
    pub viewport: Size,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: String,
    pub key: i32,
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub z: f64,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Command {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default, rename = "match")]
    pub match_text: Option<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub panel: Option<String>,
    #[serde(default)]
    pub count: Option<i64>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub node: Option<i64>,
    #[serde(default)]
    pub terminal_ids: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

pub enum Event {
    Command { owner: String, command: Command },
    Ready,
}

const NO_GATEWAY_ERR: &str = "no HTTP gateway for this kernel";

pub fn websocket_url(base: &str) -> String {
    let base = base
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{base}/api/board/ws")
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

/// Connect and stay connected. Snapshots the caller sends go up; commands
/// come down on the returned channel.
pub fn listen(
    place: Place,
) -> (
    mpsc::UnboundedSender<Snapshot>,
    mpsc::UnboundedReceiver<Event>,
) {
    let (snap_tx, mut snap_rx) = mpsc::unbounded_channel();
    let (tx, rx) = mpsc::unbounded_channel();
    acp::runtime().spawn(async move {
        loop {
            match run(&place, &mut snap_rx, &tx).await {
                Ok(()) => {}
                Err(e) if e.to_string() == NO_GATEWAY_ERR => {
                    tokio::time::sleep(NO_GATEWAY).await;
                }
                Err(_) => {
                    tokio::time::sleep(RECONNECT).await;
                }
            }
            if tx.is_closed() {
                return;
            }
        }
    });
    (snap_tx, rx)
}

async fn run(
    place: &Place,
    snaps: &mut mpsc::UnboundedReceiver<Snapshot>,
    tx: &mpsc::UnboundedSender<Event>,
) -> Result<()> {
    tokio::task::spawn_blocking({
        let place = place.clone();
        move || kernel::attach_or_spawn_place(&place)
    })
    .await
    .map_err(|e| anyhow!("board attach panicked: {e}"))??;
    // The board socket is the gateway's (`web.json`), not the attach
    // port's `tcp://` address, which no WebSocket client can open.
    let base = tokio::task::spawn_blocking({
        let place = place.clone();
        move || kernel::http_base_place(&place)
    })
    .await
    .map_err(|e| anyhow!("board base lookup panicked: {e}"))?
    .ok_or_else(|| anyhow!(NO_GATEWAY_ERR))?;
    let url = websocket_url(&base);
    let (ws, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(&url))
        .await
        .map_err(|_| anyhow!("board websocket timed out"))?
        .map_err(|e| anyhow!("board websocket: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let _ = tx.send(Event::Ready);

    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Ok(());
                };
                let Message::Text(text) = frame.map_err(|e| anyhow!("{e}"))? else {
                    continue;
                };
                if let Some(event) = decode(&text) {
                    let _ = tx.send(event);
                }
            }
            snap = snaps.recv() => {
                let Some(snap) = snap else {
                    return Ok(());
                };
                send_snapshot(&mut sink, snap).await?;
            }
        }
    }
}

async fn send_snapshot(
    sink: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    snap: Snapshot,
) -> Result<()> {
    let mut value = serde_json::to_value(&snap)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("type".into(), json!("snapshot"));
    }
    let text = serde_json::to_string(&value)?;
    sink.send(Message::Text(Utf8Bytes::from(text)))
        .await
        .map_err(|e| anyhow!("board snapshot: {e}"))
}

fn decode(text: &str) -> Option<Event> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.get("type")?.as_str()? {
        "command" => {
            let command: Command = serde_json::from_value(value.get("command")?.clone()).ok()?;
            let owner = value
                .get("owner")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            Some(Event::Command { owner, command })
        }
        _ => None,
    }
}
