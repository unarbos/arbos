//! Log redaction: no `arbk_…` key and no webhook `Authorization` value ever reaches a log line
//! (Buzz L13; preferences 2026-10-26).

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

/// Extra literal secrets to blank (the webhook auth value, the webhook URL).
static EXTRA: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn extra() -> &'static Mutex<Vec<String>> {
    EXTRA.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a secret value so [`redact`] blanks it wherever it appears.
pub fn protect(value: &str) {
    if value.len() >= 8 {
        extra().lock().unwrap().push(value.to_owned());
    }
}

/// Replace every key-shaped token and every protected value with `[redacted]`.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("arbk_") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 5..];
        let hex_len = tail.chars().take_while(|c| c.is_ascii_hexdigit()).count();
        if hex_len >= 8 {
            out.push_str("[redacted]");
            rest = &tail[hex_len..];
        } else {
            out.push_str("arbk_");
            rest = tail;
        }
    }
    out.push_str(rest);
    for v in extra().lock().unwrap().iter() {
        if out.contains(v.as_str()) {
            out = out.replace(v.as_str(), "[redacted]");
        }
    }
    out
}

/// A `MakeWriter` for `tracing_subscriber::fmt` that redacts each line before writing it to
/// stderr and, when set, to the log file.
#[derive(Clone)]
pub struct RedactingWriter {
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl RedactingWriter {
    pub fn new(file: Option<std::fs::File>) -> Self {
        Self {
            file: file.map(|f| Arc::new(Mutex::new(f))),
        }
    }
}

pub struct RedactingSink {
    file: Option<Arc<Mutex<std::fs::File>>>,
}

impl Write for RedactingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = redact(&String::from_utf8_lossy(buf));
        let mut err = std::io::stderr().lock();
        err.write_all(line.as_bytes())?;
        if let Some(f) = &self.file {
            let _ = f.lock().unwrap().write_all(line.as_bytes());
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RedactingWriter {
    type Writer = RedactingSink;
    fn make_writer(&'a self) -> Self::Writer {
        RedactingSink {
            file: self.file.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Buzz L13: a key or a webhook auth value in a log line never survives.
    #[test]
    fn log_redacts_key_and_webhook_auth() {
        let key = format!("arbk_{}", "c".repeat(40));
        let line = format!("sending Authorization: Bearer {key} to the relay");
        let r = redact(&line);
        assert!(!r.contains(&key));
        assert!(r.contains("[redacted]"));
        protect("Bearer grk_super_secret_value_1234");
        let r = redact("POST webhook with Authorization: Bearer grk_super_secret_value_1234 ok");
        assert!(!r.contains("grk_super_secret"));
        assert_eq!(redact("arbk_ alone is fine"), "arbk_ alone is fine");
    }
}
