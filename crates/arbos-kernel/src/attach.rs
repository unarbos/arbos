use anyhow::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::mpsc,
};

pub use arbos_core::wire::{Frame, TreeNode};

pub async fn write_loop(mut w: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<Frame>) {
    while let Some(frame) = rx.recv().await {
        if let Ok(line) = serde_json::to_string(&frame) {
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if w.write_all(b"\n").await.is_err() {
                break;
            }
        }
    }
}

/// Frames from one client. A line that is not a frame is answered with an
/// `error` frame on the same connection and logged; it used to vanish.
pub async fn read_loop(
    r: OwnedReadHalf,
    tx: mpsc::UnboundedSender<Frame>,
    out: mpsc::UnboundedSender<Frame>,
) -> Result<()> {
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Frame>(&line) {
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
