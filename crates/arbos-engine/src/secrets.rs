//! Secrets an agent may use without ever seeing.
//!
//! `<place>/.arbos/secrets.toml` names each secret and where it comes from
//! — an `op://` reference, an environment variable, a file — never the
//! value. The `secret` tool resolves a source and *grants* it: from then on
//! every `bash` job of this kernel gets it as an environment variable, and
//! every tool result has the value replaced by `[REDACTED:NAME]` before it
//! reaches the transcript. The kernel's own model API key is protected the
//! same way from the start, since bash inherits the kernel's environment.
//!
//! One store per kernel process: grants are not scoped per agent yet.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Values shorter than this are not tracked: redacting "1234" would eat
/// every date in a log.
const MIN_LEN: usize = 8;

#[derive(Default)]
pub struct Store {
    /// name → value, for the job environment and for redaction.
    granted: Mutex<BTreeMap<String, String>>,
    /// name → value, redaction only (the kernel's own key).
    protected: Mutex<BTreeMap<String, String>>,
}

pub fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(Store::default)
}

impl Store {
    pub fn grant(&self, name: &str, value: String) {
        self.granted.lock().unwrap().insert(name.to_string(), value);
    }

    /// Stop providing it to jobs. The value stays tracked for redaction:
    /// a job may have written it somewhere a later read brings back.
    pub fn revoke(&self, name: &str) -> bool {
        let taken = self.granted.lock().unwrap().remove(name);
        match taken {
            Some(value) => {
                self.protect(name, value);
                true
            }
            None => false,
        }
    }

    /// Track a value for redaction without exposing it to jobs.
    pub fn protect(&self, name: &str, value: String) {
        if value.len() >= MIN_LEN {
            self.protected
                .lock()
                .unwrap()
                .insert(name.to_string(), value);
        }
    }

    pub fn granted_names(&self) -> Vec<String> {
        self.granted.lock().unwrap().keys().cloned().collect()
    }

    /// The environment every job gets.
    pub fn env(&self) -> Vec<(String, String)> {
        self.granted
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// `text` with every tracked value replaced by `[REDACTED:NAME]`.
    /// Longest values first, so a value that contains another is replaced
    /// whole. Exact substrings only: a value split, encoded, or reversed
    /// by a command is not caught.
    pub fn redact(&self, text: &str) -> String {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (k, v) in self.granted.lock().unwrap().iter() {
            if v.len() >= MIN_LEN {
                pairs.push((k.clone(), v.clone()));
            }
        }
        for (k, v) in self.protected.lock().unwrap().iter() {
            pairs.push((k.clone(), v.clone()));
        }
        if pairs.is_empty() {
            return text.to_string();
        }
        pairs.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        let mut out = text.to_string();
        for (name, value) in pairs {
            if out.contains(&value) {
                out = out.replace(&value, &format!("[REDACTED:{name}]"));
            }
        }
        out
    }

    pub fn has_any(&self) -> bool {
        !self.granted.lock().unwrap().is_empty() || !self.protected.lock().unwrap().is_empty()
    }
}

/// `[secrets]` in `<place>/.arbos/secrets.toml`: name → source.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub secrets: BTreeMap<String, String>,
}

impl Config {
    pub fn load(place: &Path) -> Result<Self> {
        let path = place.join(".arbos").join("secrets.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// What `list` shows: the name and the kind of source, never the value.
    pub fn describe(&self) -> Vec<(String, String)> {
        self.secrets
            .iter()
            .map(|(k, v)| (k.clone(), source_kind(v).to_string()))
            .collect()
    }
}

/// The kind of a source, for receipts.
pub fn kind_of(source: &str) -> &'static str {
    source_kind(source)
}

fn source_kind(source: &str) -> &'static str {
    if source.starts_with("op://") {
        "1Password"
    } else if source.starts_with("env:") {
        "environment variable"
    } else if source.starts_with("file:") {
        "file"
    } else {
        "unknown source kind"
    }
}

/// Fetch a secret's value from its source. Blocking (`op` is a process).
pub fn resolve(source: &str) -> Result<String> {
    if let Some(var) = source.strip_prefix("env:") {
        let v = std::env::var(var.trim())
            .with_context(|| format!("{var} is not set in the kernel's environment"))?;
        return non_empty(v, source);
    }
    if let Some(path) = source.strip_prefix("file:") {
        let v = std::fs::read_to_string(path.trim()).with_context(|| format!("read {path}"))?;
        return non_empty(v, source);
    }
    if source.starts_with("op://") {
        if std::env::var_os("OP_SERVICE_ACCOUNT_TOKEN").is_none()
            && std::env::var_os("OP_SESSION").is_none()
        {
            bail!(
                "1Password: OP_SERVICE_ACCOUNT_TOKEN is not set in the kernel's environment, so `op read` cannot run"
            );
        }
        let out = Command::new("op")
            .args(["read", "--no-newline", source])
            .stdin(std::process::Stdio::null())
            .output()
            .context("run `op` (is the 1Password CLI installed?)")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!(
                "op read {source}: {}",
                err.lines().last().unwrap_or("failed").trim()
            );
        }
        return non_empty(String::from_utf8_lossy(&out.stdout).into_owned(), source);
    }
    bail!("unknown secret source {source:?}: use op://vault/item/field, env:VAR, or file:/path")
}

fn non_empty(v: String, source: &str) -> Result<String> {
    let v = v.trim_end_matches(['\n', '\r']).to_string();
    if v.trim().is_empty() {
        bail!("{source} resolved to an empty value");
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_every_tracked_value_longest_first() {
        let s = Store::default();
        s.grant("TOKEN", "abcdefghij".into());
        s.grant("LONGER", "abcdefghijklmnop".into());
        s.protect("KEY", "sk-or-v1-0123456789".into());
        s.grant("SHORT", "ab".into());
        let out = s.redact("x abcdefghijklmnop y abcdefghij z sk-or-v1-0123456789 ab");
        assert_eq!(
            out,
            "x [REDACTED:LONGER] y [REDACTED:TOKEN] z [REDACTED:KEY] ab"
        );
    }
}
