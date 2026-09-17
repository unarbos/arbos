//! One attached client: frames in, frames out, over plain TCP lines or a
//! WebSocket. The kernel decides which by looking at the first bytes: an
//! HTTP `GET` is a WebSocket upgrade (a phone through a Cloudflare tunnel,
//! which carries HTTP only); anything else is the newline-delimited JSON
//! the desktop and the CLI speak.

use anyhow::{Context, Result, bail};
use arbos_core::hub::HubFrame;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncReadExt;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::mpsc,
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

pub use arbos_core::wire::{Frame, TreeNode};

/// One client that reached this kernel through the hub: a numbered
/// channel on the kernel's own outbound socket. The hub verified who it
/// is. `open` goes false when the hub closes the channel, so the writer
/// fails and the client's send loop ends like a dropped socket.
pub struct HubChannel {
    pub chan: u64,
    pub to_hub: mpsc::UnboundedSender<HubFrame>,
    pub open: Arc<AtomicBool>,
}

impl HubChannel {
    /// The reader and writer for one hub channel; the JSON lines the hub
    /// delivers for it go into `from_hub`, parsed here like any other
    /// transport's (so an unknown type or a new field is this kernel's
    /// call, never the hub's).
    pub fn split(self, from_hub: mpsc::UnboundedReceiver<String>) -> (Reader, Writer) {
        (Reader::Chan(from_hub), Writer::Chan(self))
    }
}

/// How long to wait for a client's first bytes before taking it for a
/// plain TCP client that is waiting on us.
const PEEK_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// What the peer sent in its upgrade request, when it is a WebSocket.
#[derive(Debug, Default, Clone)]
pub struct Upgrade {
    pub uri: String,
    pub authorization: Option<String>,
}

/// A connection after the transport is known.
pub enum Conn {
    Tcp(TcpStream),
    Ws(WebSocketStream<TcpStream>, Upgrade),
    /// An HTTP `POST`: a webhook. Answered and closed by the door, never
    /// admitted as a client.
    Hook(HookRequest),
    /// A plain HTTP `GET` with no WebSocket upgrade — a tunnel's health
    /// probe, a browser, `curl`. Answered with a small reply and closed
    /// (qa-036: dropping it read as 502 through cloudflared).
    Http {
        stream: TcpStream,
        path: String,
    },
}

/// One HTTP POST as the webhook door reads it: the request line's path,
/// the `Authorization` header, the body, and the stream to answer on.
pub struct HookRequest {
    pub uri: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub stream: TcpStream,
}

/// Bodies beyond this are refused with 413: a hook carries a sentence,
/// not a file.
pub const HOOK_MAX_BODY: usize = 256 * 1024;

impl HookRequest {
    /// Write an HTTP/1.1 response and close.
    pub async fn respond(mut self, status: u16, reason: &str, body: &str) {
        let text = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = self.stream.write_all(text.as_bytes()).await;
        let _ = self.stream.shutdown().await;
    }
}

/// Read one HTTP request (headers, then `Content-Length` bytes of body).
async fn read_http_post(mut stream: TcpStream) -> Result<HookRequest> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            anyhow::bail!("webhook: headers over 64 KB");
        }
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .context("webhook: headers took over 10 s")??;
        if n == 0 {
            anyhow::bail!("webhook: connection closed before the headers ended");
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let uri = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    let mut authorization = None;
    let mut content_type = None;
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => authorization = Some(value.to_string()),
            "content-type" => content_type = Some(value.to_string()),
            "content-length" => content_length = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    if content_length > HOOK_MAX_BODY {
        anyhow::bail!(
            "webhook: body of {content_length} bytes is over the {HOOK_MAX_BODY} byte cap"
        );
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .context("webhook: body took over 10 s")??;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(HookRequest {
        uri,
        authorization,
        content_type,
        body,
        stream,
    })
}

impl Conn {
    /// Look at the first bytes without taking them; upgrade when they are
    /// an HTTP request. The desktop and the CLI send nothing until the
    /// kernel's hello, so silence for a moment means plain TCP.
    pub async fn detect(stream: TcpStream) -> Result<Self> {
        let mut head = [0u8; 4];
        let n = match tokio::time::timeout(PEEK_WAIT, stream.peek(&mut head)).await {
            Ok(n) => n.context("peek")?,
            Err(_) => 0,
        };
        if n >= 4 && &head[..4] == b"POST" {
            return Ok(Conn::Hook(read_http_post(stream).await?));
        }
        if n >= 3 && &head[..3] == b"GET" {
            // The request head, as much as has arrived: a GET without an
            // upgrade is not a client and gets an HTTP answer instead of a
            // failed handshake and a closed socket.
            let mut buf = vec![0u8; 8192];
            let m = tokio::time::timeout(PEEK_WAIT, stream.peek(&mut buf))
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or(n);
            let head_text = String::from_utf8_lossy(&buf[..m]).to_string();
            let lower = head_text.to_ascii_lowercase();
            let upgrades = lower
                .lines()
                .any(|l| l.trim_start().starts_with("upgrade:") && l.contains("websocket"));
            if !upgrades {
                let path = head_text
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                return Ok(Conn::Http { stream, path });
            }
            let mut upgrade = Upgrade::default();
            let seen = &mut upgrade;
            let ws = tokio_tungstenite::accept_hdr_async(
                stream,
                |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    seen.uri = req.uri().to_string();
                    seen.authorization = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    Ok(resp)
                },
            )
            .await
            .context("websocket handshake")?;
            Ok(Conn::Ws(ws, upgrade))
        } else {
            Ok(Conn::Tcp(stream))
        }
    }

    /// Split into a reader of frames and a writer of frames.
    pub fn split(self) -> (Reader, Writer) {
        match self {
            Conn::Tcp(stream) => {
                let (r, w) = stream.into_split();
                (Reader::Tcp(BufReader::new(r).lines()), Writer::Tcp(w))
            }
            Conn::Ws(ws, _) => {
                let (sink, source) = ws.split();
                (Reader::Ws(source, Vec::new()), Writer::Ws(sink))
            }
            // The door answers a hook before anyone calls split; a hook
            // that reaches here reads as a closed peer.
            Conn::Hook(req) => {
                let (r, w) = req.stream.into_split();
                (Reader::Tcp(BufReader::new(r).lines()), Writer::Tcp(w))
            }
            // Same for a plain GET: answered by the door before this.
            Conn::Http { stream, .. } => {
                let (r, w) = stream.into_split();
                (Reader::Tcp(BufReader::new(r).lines()), Writer::Tcp(w))
            }
        }
    }
}

pub enum Reader {
    Tcp(tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>),
    /// The source, plus lines left over from a message that carried
    /// several frames.
    Ws(
        futures_util::stream::SplitStream<WebSocketStream<TcpStream>>,
        Vec<String>,
    ),
    /// JSON lines relayed by the hub for one channel; ends when the hub
    /// closes it or the hub link drops.
    Chan(mpsc::UnboundedReceiver<String>),
}

impl Reader {
    /// The next non-empty line, or `None` when the peer is gone.
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            match self {
                Reader::Chan(rx) => {
                    let line = rx.recv().await?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    return Some(line);
                }
                Reader::Tcp(lines) => match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => continue,
                    Ok(Some(line)) => return Some(line),
                    _ => return None,
                },
                Reader::Ws(source, pending) => {
                    if let Some(line) = pending.pop() {
                        return Some(line);
                    }
                    let text = match source.next().await? {
                        Ok(Message::Text(t)) => t.to_string(),
                        Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).to_string(),
                        Ok(Message::Close(_)) | Err(_) => return None,
                        Ok(_) => continue,
                    };
                    // One frame per message is the norm; a client that
                    // pastes a whole batch gets every line honoured.
                    let mut lines: Vec<String> = text
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_string)
                        .collect();
                    lines.reverse();
                    *pending = lines;
                }
            }
        }
    }
}

pub enum Writer {
    Tcp(tokio::net::tcp::OwnedWriteHalf),
    Ws(futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>),
    Chan(HubChannel),
}

impl Writer {
    pub async fn send(&mut self, frame: &Frame) -> Result<()> {
        match self {
            Writer::Tcp(w) => {
                let line = serde_json::to_string(frame)?;
                w.write_all(line.as_bytes()).await?;
                w.write_all(b"\n").await?;
                Ok(())
            }
            Writer::Ws(sink) => {
                let line = serde_json::to_string(frame)?;
                sink.send(Message::Text(line.into())).await?;
                Ok(())
            }
            Writer::Chan(ch) => {
                if !ch.open.load(Ordering::Relaxed) {
                    bail!("hub channel {} closed", ch.chan);
                }
                ch.to_hub
                    .send(HubFrame::Frame {
                        chan: ch.chan,
                        frame: serde_json::to_value(frame)?,
                    })
                    .map_err(|_| anyhow::anyhow!("hub link closed"))
            }
        }
    }
}

pub async fn write_loop(mut w: Writer, mut rx: mpsc::UnboundedReceiver<Frame>) {
    while let Some(frame) = rx.recv().await {
        if w.send(&frame).await.is_err() {
            break;
        }
    }
}

/// Frames from one client. A line that is not a frame is answered with an
/// `error` frame on the same connection and logged; it used to vanish. A
/// frame the client's role does not allow is answered the same way and
/// never reaches the kernel.
pub async fn read_loop(
    mut r: Reader,
    role: crate::access::Role,
    tx: mpsc::UnboundedSender<Frame>,
    out: mpsc::UnboundedSender<Frame>,
) -> Result<()> {
    while let Some(line) = r.next_line().await {
        match serde_json::from_str::<Frame>(&line) {
            // A type this kernel does not know: say so, keep the connection.
            Ok(Frame::Unknown) => {
                let kind = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
                    .unwrap_or_default();
                let detail = format!("unknown frame type {kind:?}");
                crate::klog::warn("frame_rejected", None, &detail);
                let _ = out.send(Frame::Error {
                    agent: None,
                    detail,
                });
            }
            Ok(frame) if !role.allows(&frame) => {
                let _ = out.send(Frame::Error {
                    agent: None,
                    detail: format!(
                        "a {} client may not send {}",
                        role.as_str(),
                        frame_name(&line)
                    ),
                });
            }
            // A second login on an open connection is nothing.
            Ok(Frame::Auth { .. }) => {}
            Ok(frame) => {
                if tx.send(frame).is_err() {
                    break;
                }
            }
            Err(e) => {
                // A broken `configure` line carries a key: never into the log.
                let head: String = if line.contains("api_key") {
                    "(a configure frame; not logged)".to_string()
                } else {
                    line.chars().take(80).collect()
                };
                let detail = format!("not a frame: {e} — {head:?}");
                crate::klog::warn("frame_rejected", None, &detail);
                let _ = out.send(Frame::Error {
                    agent: None,
                    detail,
                });
            }
        }
    }
    Ok(())
}

/// The `type` of a frame line, for an error message.
fn frame_name(line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "that frame".into())
}

/// The reply to a plain GET on the attach port: `/` and `/healthz` get
/// `200` with what this is — the kernel's version, that attaching is a
/// WebSocket, how it authenticates — so a tunnel, a load balancer, or a
/// browser sees a healthy origin; any other path gets `426 Upgrade
/// Required`. The request head is drained first so the peer never sees a
/// reset before the reply.
pub async fn answer_http(
    mut stream: TcpStream,
    path: &str,
    kernel: &str,
    protocol: u32,
    auth: &str,
    update_gate: serde_json::Value,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sink = [0u8; 8192];
    let _ = tokio::time::timeout(PEEK_WAIT, stream.read(&mut sink)).await;
    let path_only = path.split('?').next().unwrap_or("/");
    let (status, body) = if path_only == "/" || path_only == "/healthz" {
        // `update_gate` is what the self-updater would decide this
        // second: `{"verdict":"busy","reason":…}` or `{"verdict":"idle"}`
        // — so a run on a real machine reads the refusal from outside.
        (
            "200 OK",
            format!(
                "{{\"kernel\":\"{kernel}\",\"git_sha\":\"{}\",\"built_at\":\"{}\",\"binary_gone\":{},\"protocol\":{protocol},\"attach\":\"websocket\",\"auth\":\"{auth}\",\"update_gate\":{update_gate}}}\n",
                crate::klog::git_sha(),
                crate::klog::built_at(),
                arbos_core::binary_gone()
            ),
        )
    } else {
        (
            "426 Upgrade Required",
            "{\"error\":\"this port speaks the arbos attach protocol over WebSocket; GET / or /healthz for a health reply\"}\n".to_string(),
        )
    };
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n{}\r\n{body}",
        body.len(),
        if status.starts_with("426") {
            "Upgrade: websocket\r\n"
        } else {
            ""
        }
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}
