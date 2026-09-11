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

pub async fn read_loop(r: OwnedReadHalf, tx: mpsc::UnboundedSender<Frame>) -> Result<()> {
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(frame) = serde_json::from_str::<Frame>(&line) {
            if tx.send(frame).is_err() {
                break;
            }
        }
    }
    Ok(())
}
