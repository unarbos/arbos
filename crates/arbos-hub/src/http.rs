//! Just enough HTTP: read one request head, answer a plain `GET`, or
//! upgrade to a WebSocket. cloudflared (or any reverse proxy) sits in
//! front and terminates TLS; the hub itself speaks plain HTTP on loopback.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

/// Longest request head the hub reads before giving up on a peer.
const HEAD_CAP: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    /// Header names lower-cased.
    pub headers: HashMap<String, String>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn wants_websocket(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
    }
}

/// Read the request head. Bytes after the blank line (a body) are not
/// expected on any route the hub serves and are dropped.
pub async fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await.context("read request")?;
        if n == 0 {
            bail!("peer closed before a full request head");
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > HEAD_CAP {
            bail!("request head over {HEAD_CAP} bytes");
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if method.is_empty() {
        bail!("not an HTTP request");
    }
    Ok(Request {
        method,
        path,
        query,
        headers,
    })
}

/// Finish the WebSocket handshake for a request that asked for one.
pub async fn upgrade(mut stream: TcpStream, req: &Request) -> Result<WebSocketStream<TcpStream>> {
    let key = req
        .header("sec-websocket-key")
        .context("upgrade without Sec-WebSocket-Key")?;
    let accept = derive_accept_key(key.as_bytes());
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .await
        .context("write upgrade response")?;
    Ok(WebSocketStream::from_raw_socket(stream, Role::Server, None).await)
}

/// One plain response, then the connection closes.
pub async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}
