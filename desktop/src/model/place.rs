//! A place: a folder on this Mac, or a folder on an ssh host.
//!
//! The one-line form is `host:folder`, or a local path. That is what the
//! opener types, what recents store, and what `state.toml` persists.

use crate::model::settings;
use std::path::{Path, PathBuf};

/// Where a project lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// ssh alias from `~/.ssh/config`. `None` is this Mac.
    pub host: Option<String>,
    /// Folder path. Absolute on this Mac. On a host, as typed — `~` is the
    /// remote home, expanded there, not here.
    pub path: PathBuf,
}

impl Default for Place {
    fn default() -> Self {
        Self {
            host: None,
            path: PathBuf::new(),
        }
    }
}

impl Place {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self {
            host: None,
            path: path.into(),
        }
    }

    pub fn remote(host: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            host: Some(host.into()),
            path: path.into(),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }

    /// The last directory in the path. `~` stays `~`. `~/work` is `work`.
    pub fn folder(&self) -> String {
        let raw = trimmed_path(&self.path);
        if raw.is_empty() || raw == "~" {
            return "~".into();
        }
        raw.rsplit('/').next().unwrap_or(&raw).to_string()
    }

    /// Home (`~`) or filesystem root (`/`), including trailing slashes and
    /// an empty path. A selected folder is anything else.
    fn is_home_or_root(&self) -> bool {
        let raw = trimmed_path(&self.path);
        raw.is_empty() || raw == "~"
    }

    /// Sidebar title. Remote home or `/` is the box name. A selected remote
    /// folder is that folder. Local is the last folder.
    pub fn title(&self) -> String {
        match &self.host {
            Some(host) if self.is_home_or_root() => host.clone(),
            _ => self.folder(),
        }
    }

    /// The one-line form: `host:path`, or a local path with home written `~`.
    pub fn encode(&self) -> String {
        match &self.host {
            Some(host) => format!("{host}:{}", self.path.display()),
            None => abbreviate(&self.path),
        }
    }

    /// Read a typed line. `host:folder` when the left side looks like an ssh
    /// alias; otherwise a local path (`~` expanded here).
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if let Some((host, path)) = raw.split_once(':')
            && looks_like_host(host)
        {
            let path = if path.is_empty() { "~" } else { path };
            return Some(Self::remote(host, path));
        }
        Some(Self::local(expand_local(raw)))
    }

    /// Where this app writes `.arbos` extras for the place. Local is the
    /// folder itself. Remote is a sidecar under the config dir — the remote
    /// path is not a directory on this Mac.
    pub fn store(&self) -> PathBuf {
        match &self.host {
            None => self.path.clone(),
            Some(host) => {
                let root = settings::dir().unwrap_or_else(|_| PathBuf::from("/tmp/arbos-desktop"));
                root.join("remote")
                    .join(safe_component(host))
                    .join(format!("{:x}", fnv(&self.path.to_string_lossy())))
            }
        }
    }
}

/// Concrete `Host` aliases in `~/.ssh/config`, no glob patterns, order as written.
pub fn ssh_hosts() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    parse_ssh_config(&home.join(".ssh").join("config"), 0, &mut |alias| {
        if seen.insert(alias.clone()) {
            out.push(alias);
        }
    });
    out
}

fn parse_ssh_config(file: &Path, depth: u8, emit: &mut dyn FnMut(String)) {
    if depth > 3 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let mut aliases: Vec<String> = Vec::new();
    let mut in_match = false;
    let flush = |aliases: &mut Vec<String>, emit: &mut dyn FnMut(String)| {
        for alias in aliases.drain(..) {
            if !alias.contains('*') && !alias.contains('?') && !alias.starts_with('!') {
                emit(alias);
            }
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<String> = line
            .replace('=', " ")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let Some(key) = parts.first().map(|k| k.to_ascii_lowercase()) else {
            continue;
        };
        let values = &parts[1..];
        match key.as_str() {
            "host" => {
                flush(&mut aliases, emit);
                in_match = false;
                aliases = values.iter().map(|s| s.to_string()).collect();
            }
            "match" => {
                flush(&mut aliases, emit);
                in_match = true;
                aliases.clear();
            }
            "include" if !in_match => {
                for pattern in values {
                    for path in expand_include(pattern) {
                        parse_ssh_config(&path, depth + 1, emit);
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut aliases, emit);
}

fn expand_include(pattern: &str) -> Vec<PathBuf> {
    let mut p = pattern.to_string();
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        p = home.join(rest).to_string_lossy().into_owned();
    }
    let path = PathBuf::from(&p);
    let path = if path.is_absolute() {
        path
    } else if let Some(home) = dirs::home_dir() {
        home.join(".ssh").join(&p)
    } else {
        path
    };
    if path.is_file() {
        return vec![path];
    }
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let needle = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !needle.contains('*') {
        return Vec::new();
    }
    let prefix = needle.split('*').next().unwrap_or("");
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix))
        })
        .collect()
}

fn trimmed_path(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('/').to_string()
}

fn looks_like_host(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('/')
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
}

fn expand_local(path: &str) -> PathBuf {
    // A `/` typed after the picker's own `/` is still one root.
    let collapsed = format!("/{}", path.trim_start_matches('/'));
    let path = if path.starts_with("//") {
        collapsed.as_str()
    } else {
        path
    };
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn abbreviate(path: &Path) -> String {
    let full = path.to_string_lossy();
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if full == home {
            return "~".into();
        }
        if let Some(rest) = full.strip_prefix(home.as_ref())
            && rest.starts_with('/')
        {
            return format!("~{rest}");
        }
    }
    full.into_owned()
}

fn safe_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Stable across launches. `DefaultHasher` is salted per process.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(1099511628211);
    }
    h
}
