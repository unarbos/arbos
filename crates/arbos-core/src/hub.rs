//! The mesh: every Arbos Jacob runs can reach every other one.
//!
//! One small service, `arbos-hub`, sits at a public address. Kernels and
//! worker daemons connect to it *outbound* over a WebSocket and register a
//! machine name; nothing on a laptop needs an open port. A client (the
//! desktop, the phone, another kernel) attaches to a kernel by machine
//! name, or claims a worker so it starts a kernel in an existing checkout
//! on that machine. This is Cursor's self-hosted-worker model: the worker
//! dials out, the service routes work to it by name and labels.
//!
//! This module holds what the hub, the kernel, and the worker share: the
//! wire ([`HubFrame`]), the roster ([`MachineInfo`]), the client config
//! (`~/.config/arbos/hub.toml`), and the on-disk mirror of the roster
//! (`.arbos/machines/`) that lets an agent discover other machines with
//! `ls`, per the file-system principle.

use crate::Place;
use crate::wire::Frame;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What the hub speaks. Bumped when a frame changes meaning.
pub const HUB_PROTOCOL: u32 = 1;

/// Env var carrying the machine token for the hub, when `hub.toml` names
/// no `token`.
pub const TOKEN_ENV: &str = "ARBOS_HUB_TOKEN";

/// A registrant is a kernel (serves one project) or a worker (starts
/// kernels on demand for the checkouts under its directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistrantKind {
    Kernel,
    Worker,
}

impl RegistrantKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrantKind::Kernel => "kernel",
            RegistrantKind::Worker => "worker",
        }
    }
}

/// Frames between the hub and anything connected to it. `Frame` (the
/// attach wire) rides inside `frame` on a numbered channel, so one
/// registration socket carries any number of attached clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum HubFrame {
    /// Registrant → hub, first frame after the socket opens.
    Register {
        machine: String,
        kind: RegistrantKind,
        #[serde(default)]
        user: String,
        #[serde(default)]
        host: String,
        /// Kernel: the project it serves. Worker: absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<String>,
        /// Worker: checkouts it can start a kernel in.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        projects: Vec<String>,
        /// The face of each project named (the kernel's own, a worker's
        /// checkouts), by project name, from each `project.toml`.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        identities: std::collections::BTreeMap<String, crate::project::ProjectIdentity>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        labels: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
        #[serde(default)]
        version: String,
        protocol: u32,
    },
    /// Hub → registrant: accepted under this name.
    Registered {
        machine: String,
        id: u64,
    },
    /// Hub → every registrant, on every change: the machines it knows.
    Roster {
        machines: Vec<MachineInfo>,
    },
    /// Hub → kernel: a client attached; `chan` names it from now on. `who`
    /// and `role` were verified by the hub.
    Open {
        chan: u64,
        who: String,
        role: String,
    },
    /// Both ways: one attach frame for channel `chan`.
    Frame {
        chan: u64,
        frame: Frame,
    },
    /// Both ways: channel `chan` is gone.
    Close {
        chan: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    /// Client → hub → worker: start a kernel for `project` on the machine.
    /// `isolate` asks for a git worktree of the checkout, so the claimer
    /// never edits the machine's own working copy.
    Claim {
        #[serde(default)]
        id: String,
        project: String,
        #[serde(default)]
        isolate: bool,
        #[serde(default)]
        from: String,
    },
    /// Worker → hub → client: the outcome. `project` is the name the new
    /// kernel registered under (a worktree gets `<project>--<id>`).
    Claimed {
        id: String,
        machine: String,
        project: String,
        #[serde(default)]
        place: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        detail: String,
    },
    Error {
        detail: String,
    },
    /// A frame this build does not know. Skipped, never fatal.
    #[serde(other)]
    Unknown,
}

/// One project a machine can serve: a live kernel, or a checkout a
/// worker can start one in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub place: String,
    /// A kernel serves it now.
    #[serde(default)]
    pub live: bool,
    /// The project's face from its `project.toml` (name, glyph, colour),
    /// as the registering kernel or worker read it; absent when the
    /// folder has no file. A phone draws its list from this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::project::ProjectIdentity>,
    /// `worktree` when this is a worker's git worktree of another project
    /// on the machine (a claim with `isolate`), not a project of the
    /// user's; absent for a project. A client nests or hides it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// For a worktree: the project it was cut from (`demo` for
    /// `demo--c616190-1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// One machine as the hub sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineInfo {
    pub name: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub version: String,
    /// A worker daemon is connected: `spawn host=<name>` can claim it.
    #[serde(default)]
    pub worker: bool,
    #[serde(default)]
    pub projects: Vec<ProjectInfo>,
    /// Unix millis of the first registration still connected.
    #[serde(default)]
    pub since: i64,
}

impl MachineInfo {
    /// One roster line for the prompt and `machines.md`.
    pub fn describe(&self) -> String {
        let mut s = self.name.clone();
        let mut bits: Vec<String> = Vec::new();
        if self.worker {
            bits.push("worker".into());
        }
        let live: Vec<&str> = self
            .projects
            .iter()
            .filter(|p| p.live)
            .map(|p| p.name.as_str())
            .collect();
        if !live.is_empty() {
            bits.push(format!("kernels: {}", live.join(", ")));
        }
        let idle: Vec<&str> = self
            .projects
            .iter()
            .filter(|p| !p.live)
            .map(|p| p.name.as_str())
            .collect();
        if !idle.is_empty() {
            bits.push(format!("checkouts: {}", idle.join(", ")));
        }
        let mut tags = self.labels.clone();
        tags.extend(self.capabilities.iter().cloned());
        if !tags.is_empty() {
            bits.push(tags.join(", "));
        }
        if !bits.is_empty() {
            s.push_str(" (");
            s.push_str(&bits.join("; "));
            s.push(')');
        }
        s
    }
}

/// `~/.config/arbos/hub.toml`: which hub this machine dials, as whom.
///
/// ```toml
/// url = "wss://hub-api.arbos.life"
/// machine = "arboslife"
/// token_env = "ARBOS_HUB_TOKEN"   # or token = "…" (file mode 0600)
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct HubConfig {
    pub url: String,
    pub machine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
}

impl HubConfig {
    pub fn path() -> PathBuf {
        crate::host_dir().join("hub.toml")
    }

    /// The file, or `None` when this machine has no hub configured. A
    /// malformed file is an error: it is the user's file.
    pub fn load() -> Result<Option<Self>> {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let cfg: Self =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        if cfg.url.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(cfg))
    }

    /// The file, with `--hub` and `--machine` flags (as env) on top.
    pub fn resolve(url: Option<&str>, machine: Option<&str>) -> Result<Option<Self>> {
        let mut cfg = Self::load()?.unwrap_or_default();
        if let Some(u) = url.map(str::trim).filter(|u| !u.is_empty()) {
            cfg.url = u.to_string();
        }
        if let Some(m) = machine.map(str::trim).filter(|m| !m.is_empty()) {
            cfg.machine = m.to_string();
        }
        if cfg.url.is_empty() {
            return Ok(None);
        }
        if cfg.machine.is_empty() {
            cfg.machine = default_machine_name();
        }
        Ok(Some(cfg))
    }

    /// The machine token: `token`, else `token_env`, else `ARBOS_HUB_TOKEN`.
    pub fn token(&self) -> Result<String> {
        if let Some(t) = self
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return Ok(t.to_string());
        }
        let var = self.token_env.as_deref().unwrap_or(TOKEN_ENV);
        match std::env::var(var) {
            Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
            _ => bail!(
                "no hub token: set {var} or put token = \"…\" in {}",
                Self::path().display()
            ),
        }
    }

    /// `wss://host/attach/<machine>[/<project>]`.
    pub fn attach_url(&self, machine: &str, project: Option<&str>) -> String {
        let mut u = format!("{}/attach/{machine}", self.url.trim_end_matches('/'));
        if let Some(p) = project.filter(|p| !p.is_empty()) {
            u.push('/');
            u.push_str(p);
        }
        u
    }

    pub fn claim_url(&self, machine: &str) -> String {
        format!("{}/claim/{machine}", self.url.trim_end_matches('/'))
    }

    pub fn register_url(&self) -> String {
        format!("{}/register", self.url.trim_end_matches('/'))
    }
}

/// This machine's short name when none is configured: the hostname's
/// first label, lower-case.
pub fn default_machine_name() -> String {
    let raw = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default();
    let name: String = raw
        .trim()
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        "local".into()
    } else {
        name
    }
}

/// A `to=` target of the form `<machine>/<agent>` or
/// `<machine>/<project>/<agent>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshTarget {
    pub machine: String,
    pub project: Option<String>,
    pub agent: String,
}

impl MeshTarget {
    pub fn parse(to: &str) -> Option<Self> {
        let parts: Vec<&str> = to.trim().split('/').map(str::trim).collect();
        match parts.as_slice() {
            [m, a] if !m.is_empty() && !a.is_empty() => Some(Self {
                machine: m.to_string(),
                project: None,
                agent: a.to_string(),
            }),
            [m, p, a] if !m.is_empty() && !p.is_empty() && !a.is_empty() => Some(Self {
                machine: m.to_string(),
                project: Some(p.to_string()),
                agent: a.to_string(),
            }),
            _ => None,
        }
    }
}

// ── the roster on disk ──────────────────────────────────────────────────

/// `.arbos/machines/`: one TOML file per machine the hub knows.
pub fn machines_dir(place: &Place) -> PathBuf {
    place.arbos().join("machines")
}

/// The file a kernel writes for one machine of the roster.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MachineFile {
    #[serde(flatten)]
    info: MachineInfo,
    hub: String,
    /// Unix millis when this file was written.
    seen: i64,
}

/// Write the roster as `.arbos/machines/<name>.toml` plus a rendered
/// `.arbos/machines.md`. Files of machines no longer listed are removed:
/// the folder is a mirror of the hub, not a history.
pub fn write_roster(place: &Place, hub_url: &str, machines: &[MachineInfo]) -> Result<()> {
    let dir = machines_dir(place);
    std::fs::create_dir_all(&dir)?;
    let now = crate::now_ms();
    let mut keep = std::collections::HashSet::new();
    for m in machines {
        let file = MachineFile {
            info: m.clone(),
            hub: hub_url.to_string(),
            seen: now,
        };
        let name = format!("{}.toml", m.name);
        keep.insert(name.clone());
        let path = dir.join(&name);
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
        std::fs::rename(&tmp, &path)?;
    }
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".toml") && !keep.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let mut md = String::from("# Machines on the hub\n\n");
    md.push_str(&format!("Hub: {hub_url}\n\n"));
    if machines.is_empty() {
        md.push_str("(none registered)\n");
    }
    for m in machines {
        md.push_str("- ");
        md.push_str(&m.describe());
        md.push('\n');
    }
    md.push_str("\n`spawn host=<name>` runs a child on a machine with a worker, in a worktree of its checkout of this project. `say to=<name>/<agent>` (or `<name>/<project>/<agent>`) messages an agent there.\n");
    let md_path = place.arbos().join("machines.md");
    let tmp = place.arbos().join(".machines.md.tmp");
    std::fs::write(&tmp, md)?;
    std::fs::rename(&tmp, &md_path)?;
    Ok(())
}

/// The roster as last written, oldest file first by name.
pub fn read_roster(place: &Place) -> Vec<MachineInfo> {
    let dir = machines_dir(place);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<MachineInfo> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| toml::from_str::<MachineFile>(&t).ok())
        .map(|f| f.info)
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One machine from the on-disk roster, by name (case-insensitive).
pub fn roster_machine(place: &Place, name: &str) -> Option<MachineInfo> {
    read_roster(place)
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(name.trim()))
}

/// The hub roster as one prompt line, or `None` when the folder is empty.
pub fn roster_line(place: &Place) -> Option<String> {
    let machines = read_roster(place);
    if machines.is_empty() {
        return None;
    }
    Some(format!(
        "Hub machines (spawn host=<name>, say to=<name>/<agent>; details in .arbos/machines/): {}",
        machines
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_target_parses_two_and_three_parts() {
        assert_eq!(
            MeshTarget::parse("mac/root"),
            Some(MeshTarget {
                machine: "mac".into(),
                project: None,
                agent: "root".into()
            })
        );
        assert_eq!(
            MeshTarget::parse("mac/arbos/fix-build"),
            Some(MeshTarget {
                machine: "mac".into(),
                project: Some("arbos".into()),
                agent: "fix-build".into()
            })
        );
        assert_eq!(MeshTarget::parse("root"), None);
        assert_eq!(MeshTarget::parse("a//b"), None);
    }

    #[test]
    fn hub_frame_unknown_is_skipped_not_fatal() {
        let f: HubFrame = serde_json::from_str(r#"{"type":"later_thing","x":1}"#).unwrap();
        assert!(matches!(f, HubFrame::Unknown));
    }

    #[test]
    fn roster_round_trips_through_the_folder() {
        let dir = std::env::temp_dir().join(format!("arbos-hub-roster-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        let place = Place::new(dir.clone());
        let m = MachineInfo {
            name: "arboslife".into(),
            user: "const".into(),
            host: "box".into(),
            labels: vec!["linux".into()],
            capabilities: vec!["gpu".into()],
            version: "0.2.0".into(),
            worker: true,
            projects: vec![ProjectInfo {
                identity: Some(crate::project::ProjectIdentity {
                    name: Some("Demo".into()),
                    icon: "terminal".into(),
                    color: "teal".into(),
                }),
                name: "demo".into(),
                place: "/x/demo".into(),
                live: false,
                kind: String::new(),
                parent: None,
            }],
            since: 1,
        };
        write_roster(&place, "wss://hub", &[m.clone()]).unwrap();
        assert_eq!(read_roster(&place), vec![m.clone()]);
        // The face rides in the roster as the phone reads it.
        let json = serde_json::to_value(&m.projects[0]).unwrap();
        assert_eq!(json["identity"]["icon"], "terminal");
        assert_eq!(json["identity"]["name"], "Demo");
        // An older hub without the field still reads.
        let old: ProjectInfo = serde_json::from_str(r#"{"name":"demo","live":true}"#).unwrap();
        assert!(old.identity.is_none());
        // Prompt-size pass (2026-09-13): the roster in the prompt carries
        // names only; the details stay in .arbos/machines/ for `read`.
        let line = roster_line(&place).unwrap();
        assert!(line.contains("arboslife"), "{line}");
        assert!(line.contains(".arbos/machines/"), "{line}");
        assert!(!line.contains("checkouts"), "{line}");
        write_roster(&place, "wss://hub", &[]).unwrap();
        assert!(read_roster(&place).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
