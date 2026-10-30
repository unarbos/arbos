//! Client side of the control socket: one JSON line in, one JSON line out.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::config::Paths;

pub async fn call(paths: &Paths, req: &Value) -> Result<Value> {
    let stream = tokio::time::timeout(
        Duration::from_secs(2),
        UnixStream::connect(paths.socket_file()),
    )
    .await
    .map_err(|_| anyhow!("sidecar not running"))?
    .map_err(|_| anyhow!("sidecar not running"))?;
    let (r, mut w) = stream.into_split();
    let mut line = req.to_string();
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    let mut lines = BufReader::new(r).lines();
    let resp = tokio::time::timeout(Duration::from_secs(90), lines.next_line())
        .await
        .map_err(|_| anyhow!("the sidecar did not answer in time"))??
        .ok_or_else(|| anyhow!("the sidecar closed the connection"))?;
    serde_json::from_str(&resp).context("control reply")
}

/// Call and turn `{ok:false}` into one human sentence.
pub async fn action(paths: &Paths, req: &Value) -> Result<Value> {
    let v = call(paths, req).await.map_err(|e| {
        if e.to_string() == "sidecar not running" {
            anyhow!("The sidecar is not running. Start it with `arbos-sidecar run` or install the service.")
        } else {
            e
        }
    })?;
    if v["ok"] == false {
        let msg = v["message"].as_str().unwrap_or("the sidecar refused");
        return Err(anyhow!("{msg}"));
    }
    Ok(v)
}
