//! Room client: a local Loro replica per chat, cursor + snapshot committed together,
//! HELLO{cursor} → STATE → ROWS_REQ → live rows; PUSH with batch ids (crdt-sync §7, §11).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use arbos_core::doc::{decode_vv, encode_vv, ChatDoc};
use arbos_core::ids::ulid;
use arbos_core::widget::Widget;
use arbos_core::wire::{self, Frame, FrameType};

use crate::client::Config;

#[derive(Serialize, Deserialize, Default)]
struct Replica {
    cursor: i64,
    #[serde(with = "b64")]
    snapshot: Vec<u8>,
    #[serde(with = "b64")]
    acked_vv: Vec<u8>,
    /// message id → chars already printed (for incremental printing across restarts).
    #[serde(default)]
    printed: HashMap<String, usize>,
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

fn replica_path(cfg: &Config, project: &str, chat: &str) -> std::path::PathBuf {
    cfg.rooms_dir().join(format!("{project}__{chat}.json"))
}

/// Fresh Loro peer id per run: a client killed between PUSH and persist must not reuse op ids.
fn peer_id(cfg: &Config) -> u64 {
    use sha2::Digest;
    let h = sha2::Sha256::digest(format!("{}:{}", cfg.device_name(), ulid()).as_bytes());
    u64::from_le_bytes(h[..8].try_into().unwrap()) | 1
}

struct Session {
    doc: ChatDoc,
    cursor: i64,
    acked_vv: loro::VersionVector,
    printed: HashMap<String, usize>,
    path: std::path::PathBuf,
    device: String,
    room: String,
}

impl Session {
    fn load(cfg: &Config, project: &str, chat: &str) -> Result<Self> {
        let path = replica_path(cfg, project, chat);
        let rep: Replica = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let doc = if rep.snapshot.is_empty() {
            ChatDoc::new(Some(peer_id(cfg)), false)
        } else {
            ChatDoc::from_snapshot(Some(peer_id(cfg)), false, &rep.snapshot)?
        };
        let acked_vv = decode_vv(&rep.acked_vv).unwrap_or_default();
        Ok(Self {
            doc,
            cursor: rep.cursor,
            acked_vv,
            printed: rep.printed,
            path,
            device: cfg.device_name(),
            room: wire::room_name(project, chat),
        })
    }

    /// Cursor and snapshot in one atomic file write.
    fn persist(&self) -> Result<()> {
        let rep = Replica {
            cursor: self.cursor,
            snapshot: self.doc.snapshot(),
            acked_vv: encode_vv(&self.acked_vv),
            printed: self.printed.clone(),
        };
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let tmp = self
            .path
            .with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string(&rep)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Print new text of assistant messages (and other Projects' messages in a group chat) and
    /// status/tool parts since the last print.
    fn print_new(&mut self) -> bool {
        let mut any_done_new = false;
        for m in self.doc.messages() {
            let is_project = m.role == "project";
            if m.role != "assistant" && !is_project {
                continue;
            }
            let full = m.text();
            let shown = self.printed.get(&m.id).copied().unwrap_or(0);
            let total_chars = full.chars().count();
            if total_chars > shown {
                let new: String = full.chars().skip(shown).collect();
                if shown == 0 {
                    let who = if is_project {
                        m.rest
                            .get("senderName")
                            .and_then(|v| v.as_str())
                            .map(|n| format!("project {n}"))
                            .unwrap_or_else(|| "project".to_owned())
                    } else {
                        "assistant".to_owned()
                    };
                    print!("\n[{who} {}] ", &m.id[m.id.len() - 6..]);
                }
                print!("{new}");
                use std::io::Write;
                std::io::stdout().flush().ok();
                self.printed.insert(m.id.clone(), total_chars);
            }
            if is_project {
                continue;
            }
            // Status events (`deciding`, `queued`, `working`, `step`, `done`, `failed`, …) are the only
            // progress the transcript shows; `hidden` parts (tools, raw subagent output) are skipped.
            let events: Vec<&serde_json::Value> = m
                .parts
                .iter()
                .filter(|p| {
                    matches!(
                        p.get("kind").and_then(|k| k.as_str()),
                        Some("status") | Some("question") | Some("error")
                    ) && p.get("hidden").and_then(|h| h.as_bool()) != Some(true)
                })
                .collect();
            let events_key = format!("{}#events", m.id);
            let shown_events = self.printed.get(&events_key).copied().unwrap_or(0);
            if events.len() > shown_events {
                for p in &events[shown_events..] {
                    if p.get("kind").and_then(|k| k.as_str()) == Some("question") {
                        println!(
                            "\n  [{} · question {} · widget {}]",
                            &m.id[m.id.len() - 6..],
                            p.get("questionId").and_then(|c| c.as_str()).unwrap_or(""),
                            p.get("widget")
                                .map(|w| w.to_string())
                                .unwrap_or_else(|| "none".into())
                        );
                        continue;
                    }
                    println!(
                        "\n  [{} · {}: {}]",
                        &m.id[m.id.len() - 6..],
                        p.get("code").and_then(|c| c.as_str()).unwrap_or(""),
                        p.get("message").and_then(|c| c.as_str()).unwrap_or("")
                    );
                }
                self.printed.insert(events_key, events.len());
            }
            // Approval cards (feature `approve`): print on arrival and again on each state change.
            for p in m
                .parts
                .iter()
                .filter(|p| p.get("kind").and_then(|k| k.as_str()) == Some("approval"))
            {
                let aid = p.get("approvalId").and_then(|v| v.as_str()).unwrap_or("");
                let state = p.get("state").and_then(|v| v.as_str()).unwrap_or("pending");
                let state_code = match state {
                    "pending" => 1,
                    "approved" => 2,
                    "rejected" => 3,
                    "expired" => 4,
                    _ => 5,
                };
                let key = format!("{aid}#approval");
                if self.printed.get(&key).copied().unwrap_or(0) != state_code {
                    println!(
                        "\n  [{} · approval {} · {} · {}]\n    {}\n    id {}{}",
                        &m.id[m.id.len() - 6..],
                        state,
                        p.get("risk").and_then(|v| v.as_str()).unwrap_or(""),
                        p.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        p.get("summary").and_then(|v| v.as_str()).unwrap_or(""),
                        aid,
                        p.get("decidedBy")
                            .and_then(|v| v.as_str())
                            .map(|d| format!(" · by {d}"))
                            .unwrap_or_default()
                    );
                    self.printed.insert(key, state_code);
                }
            }
            let status_key = format!("{}#status", m.id);
            let st = m.status.clone();
            let prev = self.printed.get(&status_key).copied().unwrap_or(0);
            let code = match st.as_str() {
                "streaming" => 1,
                "done" => 2,
                "error" => 3,
                _ => 0,
            };
            if code != prev {
                if code >= 2 {
                    let decision = m
                        .rest
                        .get("decision")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let tools = m
                        .parts
                        .iter()
                        .filter(|p| p.get("kind").and_then(|k| k.as_str()) == Some("tool"))
                        .count();
                    println!(
                        "\n[{} {} route={} tools={}]",
                        &m.id[m.id.len() - 6..],
                        st,
                        decision,
                        tools
                    );
                    any_done_new = true;
                }
                self.printed.insert(status_key, code);
            }
        }
        any_done_new
    }
}

async fn connect(
    cfg: &Config,
    room: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    if cfg.device_token.is_empty() {
        return Err(anyhow!("not paired"));
    }
    let url = format!(
        "{}/room/{}?token={}",
        cfg.relay_url.replacen("http", "ws", 1),
        room,
        cfg.device_token
    );
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    .map_err(|_| anyhow!("dial timeout"))??;
    Ok(ws)
}

enum Mode {
    Dump,
    Stream {
        until_done: bool,
    },
    Send {
        text: String,
        stream: bool,
        /// `answer` instead of `send`: `(question id or "" for the newest open one, value)`.
        answer: Option<(String, serde_json::Value)>,
        /// The message this one replies to (`replyTo` on the row).
        reply_to: Option<String>,
        /// Reply in DM: the quoted message in another chat (`replyRef` on the row).
        reply_ref: Option<Box<arbos_core::doc::ReplyRef>>,
    },
    /// Append one out-of-band command (`stop`, `approve`, `reject`) and exit on its ack.
    Command {
        kind: String,
        payload: Value,
    },
}

async fn run(
    cfg: &mut Config,
    project: &str,
    chat: &str,
    mode: Mode,
    timeout: Duration,
) -> Result<()> {
    let mut s = Session::load(cfg, project, chat)?;
    // People chats: the row names its author like the apps do (`user:<id>`, handle, name) so
    // the agent knows who asked (D72 attribution; a secret's People list, spec §7.7). In a
    // project's own chats the plain `user` sender is the owner.
    let author = if project == "people" && matches!(mode, Mode::Send { .. }) {
        match crate::client::Relay::new(cfg).get("/me").await {
            Ok(me) => {
                let u = me.pointer("/user").unwrap_or(&me);
                let id = u.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                (!id.is_empty()).then(|| {
                    (
                        format!("user:{id}"),
                        u.get("login").and_then(Value::as_str).map(str::to_owned),
                        u.get("name")
                            .and_then(Value::as_str)
                            .filter(|n| !n.is_empty())
                            .map(str::to_owned),
                    )
                })
            }
            Err(_) => None,
        }
    } else {
        None
    };
    let ws = connect(cfg, &s.room).await?;
    let (mut sink, mut stream) = ws.split();
    let hello = Frame::new(
        FrameType::Hello,
        json!({ "cursor": s.cursor, "device": s.device, "room": s.room, "proto": arbos_core::PROTO_VERSION }),
        Bytes::new(),
    );
    sink.send(Message::Binary(hello.encode())).await?;
    let started = Instant::now();
    let mut caught_up = false;
    let mut pending_push: Option<(String, loro::VersionVector)> = None;
    let mut sent_msg_id: Option<String> = None;
    let mut rows_seen = 0usize;
    let mut box_state = String::from("?");

    loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if started.elapsed() > timeout { eprintln!("\n[timeout after {:?}]", timeout); break; }
                continue;
            }
        };
        let Some(msg) = msg else { break };
        let data = match msg? {
            Message::Binary(b) => b,
            Message::Close(_) => break,
            Message::Ping(p) => {
                sink.send(Message::Pong(p)).await?;
                continue;
            }
            _ => continue,
        };
        let Some(f) = Frame::decode(data)? else {
            continue;
        };
        match f.ty {
            FrameType::State => {
                let st: wire::State = serde_json::from_value(f.header)?;
                box_state = format!("{:?}", st.box_state).to_ascii_lowercase();
                let mut after = s.cursor;
                let contained = decode_vv(&f.payload)
                    .map(|vv| s.doc.contains_vv(&vv))
                    .unwrap_or(true);
                if st.checkpoint_seq > 0 && (!contained || s.cursor < st.checkpoint_seq) {
                    let url = format!("{}/room/{}/checkpoint", cfg.relay_url, s.room);
                    if let Ok(r) = reqwest::Client::new()
                        .get(url)
                        .bearer_auth(&cfg.device_token)
                        .send()
                        .await
                    {
                        if r.status().is_success() {
                            let b = r.bytes().await?;
                            s.doc.import(&b)?;
                            s.cursor = s.cursor.max(st.checkpoint_seq);
                            after = s.cursor;
                        }
                    }
                }
                eprintln!(
                    "[room {} head={} cursor={} box={}]",
                    s.room, st.head_seq, s.cursor, box_state
                );
                let req = Frame::new(
                    FrameType::RowsReq,
                    json!({ "after": after, "excludeOwn": true }),
                    Bytes::new(),
                );
                sink.send(Message::Binary(req.encode())).await?;
            }
            FrameType::Row => {
                let rh: wire::RowHeader = serde_json::from_value(f.header)?;
                // Import every row, whatever its seq: a Loro update is idempotent, and a row
                // below the cursor may be one the socket had not delivered yet (the relay now
                // orders them, but a replica must never drop data on an ordering assumption).
                if rh.device != s.device {
                    s.doc.import(&f.payload)?;
                }
                s.cursor = s.cursor.max(rh.seq);
                rows_seen += 1;
                s.persist()?;
                if caught_up || matches!(mode, Mode::Stream { .. }) {
                    let done = s.print_new();
                    if done {
                        s.persist()?;
                        let finished = matches!(mode, Mode::Send { .. })
                            || matches!(mode, Mode::Stream { until_done: true });
                        let reference = sent_msg_id.clone().or_else(|| {
                            s.doc
                                .messages()
                                .iter()
                                .rev()
                                .find(|m| m.role == "user")
                                .map(|m| m.id.clone())
                        });
                        if finished
                            && reference
                                .map(|id| reply_done_for(&s.doc, &id))
                                .unwrap_or(true)
                        {
                            break;
                        }
                    }
                }
            }
            FrameType::RowsDone => {
                caught_up = true;
                let head = f
                    .header
                    .get("headSeq")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                s.cursor = s.cursor.max(head);
                s.persist()?;
                eprintln!("[caught up: {} new rows, head={}]", rows_seen, head);
                match &mode {
                    Mode::Dump => {
                        println!("{}", serde_json::to_string_pretty(&s.doc.to_json())?);
                        break;
                    }
                    Mode::Stream { until_done } => {
                        let done = s.print_new();
                        if *until_done && done {
                            break;
                        }
                    }
                    Mode::Command { kind, payload } => {
                        s.print_new();
                        s.doc.append_command(kind, payload.clone(), &s.device);
                        let bytes = s.doc.export_updates_since(&s.acked_vv);
                        let batch = ulid();
                        pending_push = Some((batch.clone(), s.doc.oplog_vv()));
                        sink.send(Message::Binary(
                            Frame::new(
                                FrameType::Push,
                                json!({ "batchId": batch }),
                                Bytes::from(bytes),
                            )
                            .encode(),
                        ))
                        .await?;
                        eprintln!("[sent {kind}]");
                    }
                    Mode::Send {
                        text,
                        stream: _,
                        answer,
                        reply_to,
                        reply_ref,
                    } => {
                        s.print_new();
                        let msg_id = ulid();
                        match answer {
                            Some((qid, value)) => {
                                let (qid, question, widget) = match s.doc.open_question() {
                                    Some((open_id, q, w)) if qid.is_empty() || *qid == open_id => {
                                        (open_id, q, w)
                                    }
                                    _ if !qid.is_empty() => (qid.clone(), String::new(), None),
                                    _ => return Err(anyhow!("no open question in this chat")),
                                };
                                let rendered = match Widget::parse(widget.as_ref()) {
                                    Ok(Some(w)) => w.answer_text(value),
                                    Ok(None) | Err(_) => format!(
                                        "Answer: {}",
                                        value
                                            .as_str()
                                            .map(str::to_owned)
                                            .unwrap_or(value.to_string())
                                    ),
                                };
                                eprintln!("[answering {qid} ({question:?}): {rendered}]");
                                s.doc
                                    .append_user_answer(&msg_id, &rendered, &qid, value, &s.device);
                            }
                            None => {
                                let mut extra: Vec<(&str, Value)> = Vec::new();
                                if let Some((_, handle, name)) = &author {
                                    if let Some(h) = handle {
                                        extra.push(("senderHandle", json!(h)));
                                    }
                                    if let Some(n) = name {
                                        extra.push(("senderName", json!(n)));
                                    }
                                }
                                s.doc.append_user_message_from(
                                    &msg_id,
                                    text,
                                    &s.device,
                                    author
                                        .as_ref()
                                        .map(|(id, _, _)| id.as_str())
                                        .unwrap_or("user"),
                                    &extra,
                                );
                            }
                        }
                        if let Some(reply_to) = reply_to {
                            s.doc
                                .set_message_fields(&msg_id, &[("replyTo", json!(reply_to))]);
                        }
                        if let Some(r) = reply_ref {
                            s.doc.set_reply_ref(&msg_id, r.as_ref());
                        }
                        let bytes = s.doc.export_updates_since(&s.acked_vv);
                        let batch = ulid();
                        pending_push = Some((batch.clone(), s.doc.oplog_vv()));
                        sent_msg_id = Some(msg_id.clone());
                        sink.send(Message::Binary(
                            Frame::new(
                                FrameType::Push,
                                json!({ "batchId": batch }),
                                Bytes::from(bytes),
                            )
                            .encode(),
                        ))
                        .await?;
                        if box_state != "awake" {
                            sink.send(Message::Binary(
                                Frame::new(
                                    FrameType::Wake,
                                    json!({ "reason": "send" }),
                                    Bytes::new(),
                                )
                                .encode(),
                            ))
                            .await?;
                        }
                        eprintln!("[sent message {msg_id} (box={box_state})]");
                    }
                }
            }
            FrameType::Ack => {
                let ack: wire::Ack = serde_json::from_value(f.header)?;
                if let Some((b, vv)) = &pending_push {
                    if *b == ack.batch_id {
                        s.acked_vv = vv.clone();
                        s.cursor = s.cursor.max(ack.seq);
                        pending_push = None;
                        s.persist()?;
                        eprintln!("[ack seq={} dup={}]", ack.seq, ack.dup);
                        if matches!(
                            mode,
                            Mode::Send { stream: false, .. } | Mode::Command { .. }
                        ) {
                            break;
                        }
                    }
                }
            }
            FrameType::Status => {
                if let Ok(st) = serde_json::from_value::<wire::StatusHeader>(f.header) {
                    box_state = format!("{:?}", st.box_state).to_ascii_lowercase();
                    eprintln!(
                        "[box {}{}]",
                        box_state,
                        st.detail.map(|d| format!(" · {d}")).unwrap_or_default()
                    );
                }
            }
            FrameType::Error => eprintln!("[relay error {}]", f.header),
            FrameType::ProbeOk
            | FrameType::Presence
            | FrameType::Hello
            | FrameType::RowsReq
            | FrameType::Push
            | FrameType::Probe
            | FrameType::Wake => {}
        }
    }
    s.persist()?;
    println!();
    Ok(())
}

/// The reply to `user_msg_id` is finished when an assistant message `inReplyTo` it is done
/// and no later subagent message is still streaming.
fn reply_done_for(doc: &ChatDoc, user_msg_id: &str) -> bool {
    let msgs = doc.messages();
    let replies: Vec<_> = msgs
        .iter()
        .filter(|m| {
            m.role == "assistant"
                && m.rest.get("inReplyTo").and_then(|v| v.as_str()) == Some(user_msg_id)
        })
        .collect();
    if replies.is_empty() {
        return false;
    }
    let any_streaming = replies.iter().any(|m| m.status == "streaming");
    let first = &replies[0];
    let decision = first
        .rest
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let expects_followup = decision == "new"
        || decision == "wake"
        || first
            .parts
            .iter()
            .any(|p| p.get("code").and_then(|c| c.as_str()) == Some("handed_off"));
    // A hand-off is complete once the main agent's prose result turn landed (or a run failed).
    if expects_followup
        && !replies.iter().any(|m| {
            m.rest.pointer("/turn/kind").and_then(|v| v.as_str()) == Some("subagent_result")
                || m.status == "error"
        })
    {
        return false;
    }
    !any_streaming
}

#[allow(clippy::too_many_arguments)]
pub async fn send(
    cfg: &mut Config,
    project: &str,
    chat: &str,
    text: &str,
    reply_to: Option<&str>,
    reply_ref: Option<&arbos_core::doc::ReplyRef>,
    stream: bool,
    timeout_s: u64,
) -> Result<()> {
    run(
        cfg,
        project,
        chat,
        Mode::Send {
            text: text.to_owned(),
            stream,
            answer: None,
            reply_to: reply_to.map(str::to_owned),
            reply_ref: reply_ref.cloned().map(Box::new),
        },
        Duration::from_secs(timeout_s),
    )
    .await
}

/// Answer the open `question` part (or the one with `question_id`) with a structured value;
/// the text the model reads is rendered here with the question's widget.
pub async fn answer(
    cfg: &mut Config,
    project: &str,
    chat: &str,
    question_id: Option<&str>,
    value: serde_json::Value,
    stream: bool,
    timeout_s: u64,
) -> Result<()> {
    run(
        cfg,
        project,
        chat,
        Mode::Send {
            text: String::new(),
            stream,
            answer: Some((question_id.unwrap_or("").to_owned(), value)),
            reply_to: None,
            reply_ref: None,
        },
        Duration::from_secs(timeout_s),
    )
    .await
}

pub async fn stream(
    cfg: &mut Config,
    project: &str,
    chat: &str,
    until_done: bool,
    timeout_s: u64,
) -> Result<()> {
    run(
        cfg,
        project,
        chat,
        Mode::Stream { until_done },
        Duration::from_secs(timeout_s),
    )
    .await
}

pub async fn stop(cfg: &mut Config, project: &str, chat: &str) -> Result<()> {
    command(cfg, project, chat, "stop", json!({})).await
}

/// `approve` / `reject` for an approval card (feature `approve`); the box applies the decision.
pub async fn decide(
    cfg: &mut Config,
    project: &str,
    chat: &str,
    approve: bool,
    approval_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    let mut payload = json!({ "approvalId": approval_id });
    if let Some(r) = reason.filter(|r| !r.trim().is_empty()) {
        payload["reason"] = json!(r);
    }
    command(
        cfg,
        project,
        chat,
        if approve { "approve" } else { "reject" },
        payload,
    )
    .await
}

async fn command(
    cfg: &mut Config,
    project: &str,
    chat: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    run(
        cfg,
        project,
        chat,
        Mode::Command {
            kind: kind.to_owned(),
            payload,
        },
        Duration::from_secs(30),
    )
    .await
}

pub async fn dump(cfg: &mut Config, project: &str, chat: &str) -> Result<()> {
    run(cfg, project, chat, Mode::Dump, Duration::from_secs(60)).await
}

pub fn reset(cfg: &Config, project: &str, chat: &str) -> Result<()> {
    let p = replica_path(cfg, project, chat);
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}
