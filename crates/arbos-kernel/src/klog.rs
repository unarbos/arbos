//! The kernel's own log: `.arbos/kernel.log`, one JSON object per line.
//!
//! The transcript records what agents did. This records what the kernel
//! did around them: start and stop (pid, version, git sha), every turn's
//! start and end, every frame it refused and why, every error it used to
//! print to stderr and nowhere else. A rollout that has the transcript and
//! this file can say why a prompt did nothing.
//!
//! Lines: `{"ts":<ms>,"level":"info|warn|error","event":"<name>",
//! "agent":"<id>"?, "detail":"<text>"}`. Warnings and errors also go to
//! stderr, as before.

use serde::Serialize;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

/// Rotate when the file passes this size at kernel start.
const ROTATE_AT: u64 = 16 * 1024 * 1024;

static LOG: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

#[derive(Serialize)]
struct Line<'a> {
    ts: i64,
    level: &'a str,
    event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<&'a str>,
    detail: &'a str,
}

/// Where the log goes for this process. Called once by `serve`.
pub fn init(path: PathBuf) {
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > ROTATE_AT {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    let slot = LOG.get_or_init(|| Mutex::new(None));
    *slot.lock().unwrap() = Some(path);
}

pub fn path() -> Option<PathBuf> {
    LOG.get().and_then(|m| m.lock().unwrap().clone())
}

fn write(level: &str, event: &str, agent: Option<&str>, detail: &str) {
    if level != "info" {
        match agent {
            Some(a) => eprintln!("{event} {a}: {detail}"),
            None => eprintln!("{event}: {detail}"),
        }
    }
    let Some(path) = path() else { return };
    let line = Line {
        ts: arbos_core::now_ms(),
        level,
        event,
        agent,
        detail,
    };
    let Ok(mut buf) = serde_json::to_vec(&line) else {
        return;
    };
    buf.push(b'\n');
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(&buf);
    }
}

pub fn info(event: &str, agent: Option<&str>, detail: impl AsRef<str>) {
    write("info", event, agent, detail.as_ref());
}

pub fn warn(event: &str, agent: Option<&str>, detail: impl AsRef<str>) {
    write("warn", event, agent, detail.as_ref());
}

pub fn error(event: &str, agent: Option<&str>, detail: impl AsRef<str>) {
    write("error", event, agent, detail.as_ref());
}

/// Version and git sha of this binary, for `kernel_start` and `kernel.json`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn git_sha() -> &'static str {
    option_env!("ARBOS_GIT_SHA").unwrap_or("unknown")
}

/// When this binary was built (`YYYY-MM-DDTHH:MMZ`), from build.rs.
pub fn build() -> &'static str {
    option_env!("ARBOS_BUILD").unwrap_or("unknown")
}

pub fn log_path_for(arbos_dir: &Path) -> PathBuf {
    arbos_dir.join("runtime").join("kernel.log")
}
