//! `session.jsonl`: the owner-facing, text-free story of every wake (spec §13.3). Written by the
//! daemon (wake, typing, ack, reply, session.*, error) and by the MCP/CLI process (read); read by
//! `arbos-sidecar log` and `arbos_log`; uploaded to the relay in batches.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{json, Value};

use crate::config::Paths;
use crate::mirror::now_ms;
use crate::redact;

#[derive(Clone)]
pub struct SessionLog {
    path: PathBuf,
    offset_path: PathBuf,
}

/// Keys that may never appear in an entry (belt; the relay drops them too).
const FORBIDDEN: &[&str] = &[
    "text", "prompt", "key", "token", "auth", "secret", "body", "url",
];

impl SessionLog {
    pub fn new(paths: &Paths) -> Self {
        Self {
            path: paths.data_dir.join("session.jsonl"),
            offset_path: paths.data_dir.join("session.uploaded"),
        }
    }

    /// Append one entry. `kind` is required; forbidden keys are dropped; strings are redacted.
    pub fn append(&self, mut entry: Value) -> Result<()> {
        let Some(obj) = entry.as_object_mut() else {
            return Ok(());
        };
        obj.retain(|k, _| {
            let lk = k.to_ascii_lowercase();
            !FORBIDDEN.iter().any(|f| lk.contains(f))
        });
        if !obj.contains_key("at") {
            obj.insert("at".into(), json!(now_ms()));
        }
        for v in obj.values_mut() {
            if let Some(s) = v.as_str() {
                *v = json!(redact::redact(s));
            }
        }
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(serde_json::to_string(&entry)?.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    }

    pub fn entries(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Newest first, filtered.
    pub fn read(&self, chat: Option<&str>, kind: Option<&str>, limit: usize) -> Vec<Value> {
        self.entries()
            .into_iter()
            .rev()
            .filter(|e| chat.is_none_or(|c| e["chat"] == c))
            .filter(|e| {
                kind.is_none_or(|k| {
                    e["kind"]
                        .as_str()
                        .is_some_and(|x| x == k || x.starts_with(&format!("{k}.")))
                })
            })
            .take(limit.max(1))
            .collect()
    }

    /// Lines already uploaded.
    fn uploaded(&self) -> usize {
        std::fs::read_to_string(&self.offset_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// The next batch to upload (≤ `max`, skipping `upload` records) and the offset to record
    /// once the relay stored it.
    pub fn pending_upload(&self, max: usize) -> (Vec<Value>, usize) {
        let all = self.entries();
        let done = self.uploaded().min(all.len());
        let mut batch = Vec::new();
        let mut offset = done;
        for (i, e) in all.iter().enumerate().skip(done) {
            if batch.len() >= max.max(1) {
                break;
            }
            offset = i + 1;
            if e["kind"] != "upload" {
                batch.push(e.clone());
            }
        }
        (batch, offset)
    }

    pub fn mark_uploaded(&self, offset: usize) -> Result<()> {
        std::fs::write(&self.offset_path, offset.to_string())?;
        Ok(())
    }

    /// Per-chat medians for `status` and the card header: `(event_to_reply_ms…)`.
    pub fn summary(&self) -> Value {
        let entries = self.entries();
        let mut replies: Vec<i64> = entries
            .iter()
            .filter(|e| e["kind"] == "reply")
            .filter_map(|e| e["ms"]["event_to_reply"].as_i64())
            .collect();
        let mut acks: Vec<i64> = entries
            .iter()
            .filter(|e| e["kind"] == "ack")
            .filter_map(|e| e["ms"]["event_to_ack"].as_i64())
            .collect();
        replies.sort_unstable();
        acks.sort_unstable();
        let med = |v: &Vec<i64>| v.get(v.len() / 2).copied();
        json!({
            "entries": entries.len(),
            "wakes": entries.iter().filter(|e| e["kind"] == "wake").count(),
            "replies": replies.len(),
            "median_event_to_ack_ms": med(&acks),
            "median_event_to_reply_ms": med(&replies),
            "errors": entries.iter().filter(|e| e["kind"] == "error").count(),
            "last_at": entries.last().and_then(|e| e["at"].as_i64()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> (tempfile::TempDir, SessionLog) {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: d.path().join("c"),
            data_dir: d.path().join("d"),
        };
        paths.ensure().unwrap();
        (d, SessionLog::new(&paths))
    }

    /// Text and secrets never reach the log, whatever a caller passes.
    #[test]
    fn log_is_text_free_and_redacted() {
        let (_d, l) = temp();
        let key = format!("arbk_{}", "e".repeat(40));
        l.append(json!({ "kind": "reply", "chat": "c1", "text": "the answer", "error": format!("boom {key}"),
            "Authorization": "Bearer x", "ms": { "event_to_reply": 9603 } }))
            .unwrap();
        let e = &l.entries()[0];
        assert!(e.get("text").is_none());
        assert!(e.get("Authorization").is_none());
        assert!(!e["error"].as_str().unwrap().contains(&key));
        assert_eq!(e["ms"]["event_to_reply"], 9603);
        assert!(e["at"].is_i64());
    }

    #[test]
    fn read_filters_and_upload_offsets() {
        let (_d, l) = temp();
        l.append(json!({ "kind": "wake", "chat": "c1" })).unwrap();
        l.append(json!({ "kind": "ack", "chat": "c1", "ms": { "event_to_ack": 1200 } }))
            .unwrap();
        l.append(json!({ "kind": "reply", "chat": "c2", "ms": { "event_to_reply": 9000 } }))
            .unwrap();
        l.append(json!({ "kind": "session.resume", "session": "x" }))
            .unwrap();
        assert_eq!(l.read(Some("c1"), None, 10).len(), 2);
        assert_eq!(l.read(None, Some("session"), 10).len(), 1);
        assert_eq!(l.read(None, Some("reply"), 10)[0]["chat"], "c2");
        let (batch, off) = l.pending_upload(100);
        assert_eq!(batch.len(), 4);
        l.mark_uploaded(off).unwrap();
        l.append(json!({ "kind": "upload", "entries": 4 })).unwrap();
        l.append(json!({ "kind": "wake", "chat": "c3" })).unwrap();
        let (batch, off2) = l.pending_upload(100);
        assert_eq!(batch.len(), 1, "upload records are not re-uploaded");
        assert_eq!(batch[0]["chat"], "c3");
        assert!(off2 > off);
        let s = l.summary();
        assert_eq!(s["replies"], 1);
        assert_eq!(s["median_event_to_reply_ms"], 9000);
        assert_eq!(s["median_event_to_ack_ms"], 1200);
    }
}
