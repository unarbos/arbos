//! One attached client: frames in, frames out, over plain TCP lines or a
//! WebSocket. The kernel decides which by looking at the first bytes: an
//! HTTP `GET` is a WebSocket upgrade (a phone through a Cloudflare tunnel,
//! which carries HTTP only); anything else is the newline-delimited JSON
//! the desktop and the CLI speak.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::mpsc,
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

pub use arbos_core::wire::{Frame, TreeNode};

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
        if n >= 3 && &head[..3] == b"GET" {
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
}

impl Reader {
    /// The next non-empty line, or `None` when the peer is gone.
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            match self {
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
}

impl Writer {
    pub async fn send_line(&mut self, line: &str) -> Result<()> {
        match self {
            Writer::Tcp(w) => {
                w.write_all(line.as_bytes()).await?;
                w.write_all(b"\n").await?;
                Ok(())
            }
            Writer::Ws(sink) => {
                sink.send(Message::Text(line.to_string().into())).await?;
                Ok(())
            }
        }
    }

    pub async fn send(&mut self, frame: &Frame) -> Result<()> {
        let line = serde_json::to_string(frame)?;
        self.send_line(&line).await
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
                let head: String = line.chars().take(80).collect();
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
