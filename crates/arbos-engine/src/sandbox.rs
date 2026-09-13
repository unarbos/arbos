//! The bash sandbox: writes only into the place (and a few allowed paths),
//! optionally no network. Linux: bubblewrap. macOS: sandbox-exec.
//!
//! ```toml
//! # .arbos/sandbox.toml
//! enabled = true
//! network = false
//! allow_write = ["~/.cache", "~/.cargo/registry"]
//! ```
//!
//! `agent.md` may say `sandbox: on` / `sandbox: off` to override the place
//! for one agent. Enabled with the tool missing fails closed: a command
//! that was promised a sandbox never runs bare.

use anyhow::{Result, bail};
use arbos_core::{Agent, Place};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub network: bool,
    /// Writable beyond the place and the temp dir. Absolute, `~` expanded.
    pub allow_write: Vec<PathBuf>,
    pub place: PathBuf,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    enabled: bool,
    network: Option<bool>,
    allow_write: Option<Vec<String>>,
}

/// Where the config lives.
pub fn config_path(place: &Place) -> PathBuf {
    place.arbos().join("sandbox.toml")
}

/// Toolchains write to these; without them every build fails inside.
const DEFAULT_WRITES: &[&str] = &["~/.cache", "~/.cargo/registry", "~/.cargo/git", "~/.npm"];

/// The sandbox for `agent` in `place`, or `None` when off. Config trouble
/// is reported and treated as off.
pub fn for_agent(place: &Place, agent: &Agent) -> Option<Sandbox> {
    let file = match std::fs::read_to_string(config_path(place)) {
        Ok(text) => match toml::from_str::<File>(&text) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("sandbox.toml: {e}; sandbox off");
                File::default()
            }
        },
        Err(_) => File::default(),
    };
    let enabled = match agent_override(place, agent) {
        Some(on) => on,
        None => file.enabled,
    };
    if !enabled {
        return None;
    }
    let writes = file
        .allow_write
        .unwrap_or_else(|| DEFAULT_WRITES.iter().map(|s| s.to_string()).collect());
    Some(Sandbox {
        network: file.network.unwrap_or(true),
        allow_write: writes.iter().filter_map(|p| expand_home(p)).collect(),
        place: place.path.clone(),
    })
}

/// `sandbox: on|off` in agent.md. The core parser keeps unknown keys out
/// of `Agent`, so the file is read here.
fn agent_override(place: &Place, agent: &Agent) -> Option<bool> {
    let path = arbos_core::Layout::new(place, agent.id.as_str()).agent_md();
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() == "sandbox" {
            return match v.trim().to_ascii_lowercase().as_str() {
                "on" | "true" | "yes" | "1" => Some(true),
                "off" | "false" | "no" | "0" => Some(false),
                _ => None,
            };
        }
    }
    None
}

fn expand_home(p: &str) -> Option<PathBuf> {
    if let Some(rest) = p.strip_prefix("~/") {
        let home = std::env::var_os("HOME")?;
        return Some(PathBuf::from(home).join(rest));
    }
    if p == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    Some(PathBuf::from(p))
}

impl Sandbox {
    /// One line for the instance prompt.
    pub fn describe(&self) -> String {
        if self.network {
            "on (writes only inside the place and /tmp)".into()
        } else {
            "on (writes only inside the place and /tmp; no network)".into()
        }
    }

    /// The program and arguments that run `sh -c <script>` inside the
    /// sandbox. Fails when the platform tool is missing.
    pub fn wrap(&self, script: &str, cwd: &Path) -> Result<(String, Vec<String>)> {
        if cfg!(target_os = "linux") {
            if !on_path("bwrap") {
                bail!(
                    "sandbox is on but bubblewrap is not installed (apt install bubblewrap); the command did not run"
                );
            }
            let mut args: Vec<String> = vec![
                "--ro-bind".into(),
                "/".into(),
                "/".into(),
                "--dev".into(),
                "/dev".into(),
                "--proc".into(),
                "/proc".into(),
                "--tmpfs".into(),
                "/tmp".into(),
            ];
            let mut binds = vec![self.place.clone()];
            binds.extend(self.allow_write.iter().cloned());
            if !cwd.starts_with(&self.place) {
                binds.push(cwd.to_path_buf());
            }
            for p in binds {
                if p.exists() {
                    let s = p.display().to_string();
                    args.extend(["--bind".into(), s.clone(), s]);
                }
            }
            if !self.network {
                args.push("--unshare-net".into());
            }
            args.push("--die-with-parent".into());
            args.extend([
                "--".into(),
                crate::jobs::job_shell().into(),
                "-c".into(),
                script.to_string(),
            ]);
            return Ok(("bwrap".into(), args));
        }
        if cfg!(target_os = "macos") {
            if !on_path("sandbox-exec") {
                bail!("sandbox is on but sandbox-exec is missing; the command did not run");
            }
            let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
            let mut allow = vec![
                self.place.display().to_string(),
                "/tmp".into(),
                "/private/tmp".into(),
                "/dev".into(),
            ];
            if let Some(t) = std::env::var_os("TMPDIR") {
                allow.push(t.to_string_lossy().into_owned());
            }
            allow.extend(self.allow_write.iter().map(|p| p.display().to_string()));
            profile.push_str("(allow file-write*");
            for p in allow {
                profile.push_str(&format!(" (subpath \"{}\")", p.replace('"', "\\\"")));
            }
            profile.push_str(")\n");
            if !self.network {
                profile.push_str("(deny network*)\n");
            }
            return Ok((
                "sandbox-exec".into(),
                vec![
                    "-p".into(),
                    profile,
                    crate::jobs::job_shell().into(),
                    "-c".into(),
                    script.to_string(),
                ],
            ));
        }
        bail!("sandbox is on but this platform has no sandbox backend; the command did not run")
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(program))
                .any(|f| f.is_file())
        })
        .unwrap_or(false)
}
