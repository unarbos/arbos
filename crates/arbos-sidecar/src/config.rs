//! Paths, `config.toml` and the key file.
//!
//! The key is read from exactly one place — a file only the user can read — and is never put on
//! argv, in the environment of a child process, or in a log line (`redact.rs`). Lessons from
//! Buzz L13/L14 (spec §12.4).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const KEY_PREFIX: &str = "arbk_";
/// Exit code when the relay rejects the key: the service must not restart-loop on it.
pub const EXIT_KEY_REJECTED: i32 = 78;
/// Exit code when another daemon already holds the control socket.
pub const EXIT_ALREADY_RUNNING: i32 = 75;

/// Where everything lives. `ARBOS_SIDECAR_HOME` puts config and data under one root (tests,
/// several agents on one machine); otherwise XDG config/data dirs.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        if let Ok(root) = std::env::var("ARBOS_SIDECAR_HOME") {
            let root = PathBuf::from(root);
            return Self {
                config_dir: root.join("config"),
                data_dir: root.join("data"),
            };
        }
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("arbos-sidecar");
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("arbos-sidecar");
        Self {
            config_dir,
            data_dir,
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn key_file(&self) -> PathBuf {
        self.config_dir.join("key")
    }
    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
    pub fn chats_file(&self) -> PathBuf {
        self.data_dir.join("chats.json")
    }
    pub fn chat_dir(&self, chat: &str) -> PathBuf {
        self.data_dir.join("chats").join(safe_segment(chat))
    }
    pub fn wakes_file(&self) -> PathBuf {
        self.data_dir.join("wakes.jsonl")
    }
    pub fn log_file(&self) -> PathBuf {
        self.data_dir.join("sidecar.log")
    }
    pub fn socket_file(&self) -> PathBuf {
        self.data_dir.join("control.sock")
    }
    pub fn pid_file(&self) -> PathBuf {
        self.data_dir.join("sidecar.pid")
    }

    /// Create the two roots with private permissions.
    pub fn ensure(&self) -> Result<()> {
        for d in [&self.config_dir, &self.data_dir] {
            fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
            fs::set_permissions(d, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

/// Ids are ULIDs; anything else is not a path segment.
pub fn safe_segment(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WakerKind {
    Webhook,
    Command,
    File,
    None,
}

impl std::str::FromStr for WakerKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "webhook" => Self::Webhook,
            "command" => Self::Command,
            "file" => Self::File,
            "none" => Self::None,
            other => bail!("unknown waker kind {other:?} (webhook | command | file | none)"),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakerConfig {
    pub kind: WakerKind,
    /// Webhook: files holding the URL and the `Authorization` value (mode 600).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_file: Option<String>,
    /// Webhook: inline URL for non-secret endpoints (tests, local catchers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Command: argv with `{prompt}`, `{json}`, `{chat}` placeholders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
    /// Command: environment variables the child may inherit (`ARBOS_*` never).
    #[serde(default = "default_env_allow")]
    pub env_allow: Vec<String>,
    /// File: where nudges are appended (default `wakes.jsonl` in the data dir).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Longest the trailing nudge waits for the agent's "done" signal (the turn window).
    #[serde(default = "default_debounce_s")]
    pub debounce_s: u64,
    /// Shortest gap between two nudges.
    #[serde(default = "default_floor_s")]
    pub floor_s: u64,
    #[serde(default = "default_max_nudges_per_hour")]
    pub max_nudges_per_hour: u32,
    #[serde(default = "default_stale_wake_s")]
    pub stale_wake_s: u64,
}

fn default_timeout_s() -> u64 {
    // A whole agent turn can run this long (Buzz raised its idle cap 300 → 1 500 s, L6).
    1800
}
fn default_env_allow() -> Vec<String> {
    [
        "PATH", "HOME", "LANG", "LC_ALL", "TERM", "USER", "SHELL", "TMPDIR",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_floor_s() -> u64 {
    2
}
fn default_debounce_s() -> u64 {
    5
}
fn default_max_nudges_per_hour() -> u32 {
    60
}
fn default_stale_wake_s() -> u64 {
    6 * 3600
}

impl Default for WakerConfig {
    fn default() -> Self {
        Self {
            kind: WakerKind::None,
            url_file: None,
            auth_file: None,
            url: None,
            command: None,
            cwd: None,
            timeout_s: default_timeout_s(),
            env_allow: default_env_allow(),
            path: None,
            debounce_s: default_debounce_s(),
            floor_s: default_floor_s(),
            max_nudges_per_hour: default_max_nudges_per_hour(),
            stale_wake_s: default_stale_wake_s(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorConfig {
    #[serde(default = "default_backfill")]
    pub backfill: usize,
    #[serde(default = "default_true")]
    pub files: bool,
    /// Rows the relay attaches to each wake event (`context=<n>`); 0 = none.
    #[serde(default = "default_context")]
    pub context: usize,
}

fn default_backfill() -> usize {
    200
}
fn default_true() -> bool {
    true
}
fn default_context() -> usize {
    0
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            backfill: default_backfill(),
            files: true,
            context: default_context(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub relay: String,
    #[serde(default)]
    pub key_file: Option<String>,
    #[serde(default)]
    pub waker: WakerConfig,
    #[serde(default)]
    pub mirror: MirrorConfig,
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Self> {
        let p = paths.config_file();
        let text = fs::read_to_string(&p).with_context(|| {
            format!(
                "no config at {} — run `arbos-sidecar init` first",
                p.display()
            )
        })?;
        let mut cfg: Config = toml::from_str(&text).context("config.toml")?;
        cfg.relay = cfg.relay.trim_end_matches('/').to_owned();
        Ok(cfg)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure()?;
        let text = toml::to_string_pretty(self)?;
        write_private(&paths.config_file(), text.as_bytes())
    }

    pub fn key_path(&self, paths: &Paths) -> PathBuf {
        match &self.key_file {
            Some(p) => expand_home(p),
            None => paths.key_file(),
        }
    }
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

/// Write a file with mode 600, replacing any previous content.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)?;
    Ok(())
}

/// Refuse a secret file that others can read (Buzz L14).
pub fn check_private(path: &Path) -> Result<()> {
    let meta = fs::metadata(path).with_context(|| format!("{} is missing", path.display()))?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "{} is readable by others (mode {:o}); run: chmod 600 {}",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

pub fn looks_like_key(s: &str) -> bool {
    s.len() == KEY_PREFIX.len() + 40
        && s.starts_with(KEY_PREFIX)
        && s[KEY_PREFIX.len()..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Read the key from its file; the only reader in the program.
pub fn read_key(path: &Path) -> Result<String> {
    check_private(path)?;
    let s = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?
        .trim()
        .to_owned();
    if !looks_like_key(&s) {
        bail!(
            "{} does not hold an Arbos agent key (arbk_ + 40 hex characters)",
            path.display()
        );
    }
    Ok(s)
}

/// Read a one-line secret (webhook URL or Authorization value) from a private file.
pub fn read_secret_line(path: &Path) -> Result<String> {
    check_private(path)?;
    let s = fs::read_to_string(path)?;
    let line = s.lines().next().unwrap_or("").trim().to_owned();
    if line.is_empty() {
        bail!("{} is empty", path.display());
    }
    Ok(line)
}

/// Ask the person at the terminal for the key with echo off. Opens `/dev/tty` so it works
/// under `curl … | sh` where stdin is the pipe.
pub fn prompt_key_from_tty() -> Result<String> {
    let mut tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| anyhow!("no terminal to ask for the key on; use --key-stdin"))?;
    tty.write_all(b"Paste the agent key from the Arbos app (it will not be shown): ")?;
    tty.flush()?;
    let _ = std::process::Command::new("sh")
        .args(["-c", "stty -echo < /dev/tty"])
        .status();
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = tty.read(&mut byte)?;
        if n == 0 || byte[0] == b'\n' || byte[0] == b'\r' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 256 {
            break;
        }
    }
    let _ = std::process::Command::new("sh")
        .args(["-c", "stty echo < /dev/tty"])
        .status();
    tty.write_all(b"\n")?;
    Ok(String::from_utf8_lossy(&buf).trim().to_owned())
}

pub fn read_key_from_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s.trim().to_owned())
}

/// Store the key: validate, write 0600, never echo.
pub fn store_key(path: &Path, key: &str) -> Result<()> {
    if !looks_like_key(key) {
        bail!("that is not an Arbos agent key (expected arbk_ followed by 40 hex characters)");
    }
    write_private(path, format!("{key}\n").as_bytes())?;
    check_private(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shape() {
        assert!(looks_like_key(&format!("arbk_{}", "a".repeat(40))));
        assert!(!looks_like_key("arbk_short"));
        assert!(!looks_like_key(&format!("arbx_{}", "a".repeat(40))));
        assert!(!looks_like_key(&format!("arbk_{}", "g".repeat(40))));
    }

    /// Buzz L14: the key file is 0600 and a loose file is refused.
    #[test]
    fn init_writes_0600_and_refuses_loose_perms() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("key");
        let key = format!("arbk_{}", "b".repeat(40));
        store_key(&p, &key).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(read_key(&p).unwrap(), key);

        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_key(&p).unwrap_err().to_string();
        assert!(err.contains("readable by others"), "{err}");
        assert!(store_key(&p, "not-a-key").is_err());
    }

    #[test]
    fn config_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: dir.path().join("c"),
            data_dir: dir.path().join("d"),
        };
        let cfg = Config {
            relay: "https://relay.example/".into(),
            key_file: None,
            waker: WakerConfig {
                kind: WakerKind::Webhook,
                url_file: Some("~/.secrets/url".into()),
                auth_file: Some("~/.secrets/auth".into()),
                ..Default::default()
            },
            mirror: MirrorConfig::default(),
        };
        cfg.save(&paths).unwrap();
        let mode = fs::metadata(paths.config_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let back = Config::load(&paths).unwrap();
        assert_eq!(back.relay, "https://relay.example");
        assert_eq!(back.waker.kind, WakerKind::Webhook);
        assert_eq!(back.waker.debounce_s, 5);
        assert_eq!(back.waker.floor_s, 2);
        assert_eq!(back.waker.max_nudges_per_hour, 60);
        assert_eq!(back.waker.stale_wake_s, 21_600);
        assert_eq!(back.waker.timeout_s, 1800);
        assert_eq!(back.mirror.backfill, 200);
        assert!(!back.waker.env_allow.iter().any(|e| e.starts_with("ARBOS")));
    }

    #[test]
    fn segments_are_safe() {
        assert_eq!(safe_segment("01M6ABC-def_9"), "01M6ABC-def_9");
        assert_eq!(safe_segment("../../etc/passwd"), "etcpasswd");
    }
}
