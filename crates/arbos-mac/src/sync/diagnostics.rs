//! App errors nobody asked for — a background fetch that failed, a list that did not refresh —
//! never surface in the chat (no toast, no bubble, no message). They go to the log, and once per
//! distinct *signature* per week one silent feedback item goes to the relay so the loop sees the
//! bug exactly once, not once per launch (`submitted_by: "app"`, `signature`). The relay dedupes
//! the same signature across devices and users; this side keeps a small on-disk ledger so an
//! offline or restarted Mac does not re-send what it already reported.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use arbos_core::ids::now_ms;

use super::credentials;

/// One report per signature per week; after that a still-failing path is worth a fresh item.
pub const REPORT_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Signatures reported per process at most, so a storm of distinct errors cannot flood the loop.
const MAX_PER_LAUNCH: usize = 20;
const FILE: &str = "mac-diagnostics.json";

pub struct Diagnostics {
    /// Signature → when it was last sent (persisted) or first seen this launch (in memory).
    seen: Mutex<HashMap<String, i64>>,
    sent_this_launch: Mutex<usize>,
    path: PathBuf,
}

impl Diagnostics {
    pub fn load() -> Self {
        let path = credentials::config_dir().join(FILE);
        let seen: HashMap<String, i64> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            seen: Mutex::new(seen),
            sent_this_launch: Mutex::new(0),
            path,
        }
    }

    /// `true` once per signature per window and launch: the caller then posts the item. The
    /// signature is marked at once (one attempt per launch) and persisted by [`Self::sent`].
    pub fn should_report(&self, signature: &str) -> bool {
        let now = now_ms();
        let mut seen = self.seen.lock().unwrap();
        if let Some(at) = seen.get(signature) {
            if now.saturating_sub(*at) < REPORT_WINDOW_MS {
                return false;
            }
        }
        let mut count = self.sent_this_launch.lock().unwrap();
        if *count >= MAX_PER_LAUNCH {
            return false;
        }
        *count += 1;
        seen.insert(signature.to_owned(), now);
        true
    }

    /// The relay took the report: remember it across launches.
    pub fn sent(&self, signature: &str) {
        let now = now_ms();
        let snapshot = {
            let mut seen = self.seen.lock().unwrap();
            seen.retain(|_, at| now.saturating_sub(*at) < REPORT_WINDOW_MS);
            seen.insert(signature.to_owned(), now);
            seen.clone()
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&snapshot) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

/// Brief network failures (timeout, reset, DNS, send, TLS, empty/truncated body, gateway blips)
/// are not product bugs — log them, do not file a weekly feedback item. Real HTTP answers (4xx
/// and non-gateway 5xx such as 500) and unexpected client errors still report. Reqwest's
/// top-level "error sending request for url …" means the request never got an HTTP response
/// (connect/TLS/send); the cause may be omitted when the caller formats with `{err}` instead of
/// `{err:#}`. Explicit rustls decrypt/handshake strings still match when only the cause is
/// present. A 200 with no body (serde "EOF while parsing … column 0") or a cut-off JSON frame is
/// the same class of blip as a closed connection. HTTP 502/503/504 (and "bad gateway" /
/// "gateway timeout" wording) are the proxy in front of the relay, not the Mac app — e.g.
/// `GET /me/unread` during a brief upstream miss (`mac:me-unread:http 502`).
pub fn is_transient(detail: &str) -> bool {
    let d = detail.to_ascii_lowercase();
    d.contains("error sending request")
        || d.contains("timed out")
        || d.contains("timeout")
        || d.contains("connection reset")
        || d.contains("connection refused")
        || d.contains("broken pipe")
        || d.contains("network is unreachable")
        || d.contains("host is down")
        || d.contains("temporarily unavailable")
        || d.contains("dns error")
        || d.contains("failed to lookup address")
        || d.contains("name or service not known")
        || d.contains("nodename nor servname")
        || d.contains("no address associated")
        // rustls mid-handshake / mid-record noise (e.g. GET /connectors over flaky TLS).
        || d.contains("cannot decrypt")
        || d.contains("decrypt peer")
        || d.contains("received fatal alert")
        || d.contains("tls handshake")
        || d.contains("connection closed before message completed")
        // Empty or truncated response body after HTTP success (e.g. GET /chats on a flaky link).
        || d.contains("eof while parsing")
        || d.contains("empty body")
        // Load-balancer / reverse-proxy blips in front of the relay (not app bugs).
        || d.contains("http 502")
        || d.contains("http 503")
        || d.contains("http 504")
        || d.contains("bad gateway")
        || d.contains("gateway timeout")
}

/// `mac:<kind>:<normalized error>` — stable across launches, files and users: ids, hashes,
/// file names, URLs and long numbers collapse to `*`, so "Thumbnail att-01m5…-Photo.jpg: HTTP 403
/// Forbidden" and the same failure for another picture share one signature.
pub fn signature(kind: &str, detail: &str) -> String {
    let mut out = String::new();
    for token in detail.split(|c: char| c.is_whitespace() || c == ':' || c == ',' || c == ';') {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        if token.is_empty() {
            continue;
        }
        let noisy = token.contains("://")
            || token.contains('/')
            || token.contains('.')
            || (token.len() >= 8 && token.chars().any(|c| c.is_ascii_digit()))
            || (token.len() >= 5 && token.chars().all(|c| c.is_ascii_digit()));
        if !out.is_empty() {
            out.push(' ');
        }
        if noisy {
            out.push('*');
        } else {
            out.extend(token.chars().flat_map(char::to_lowercase));
        }
    }
    let mut normalized: String = out.chars().take(120).collect();
    if normalized.is_empty() {
        normalized.push_str("error");
    }
    format!("mac:{kind}:{normalized}")
}
