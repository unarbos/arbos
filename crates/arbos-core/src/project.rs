//! `.arbos/project.toml`: the place's own settings, and the one role that
//! changes what the main chat may do.
//!
//! Cursor's model (decided 2026-09-13): the agent the user talks to is a
//! coordinator. It reads, plans, spawns, steers, and asks; it does not edit
//! code. `[root] role = "coordinator"` turns that on. New places get it;
//! an existing place keeps its old behaviour until someone adds the line.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{Agent, Place};

pub const COORDINATOR: &str = "coordinator";

/// What a coordinator keeps of the tool set: everything that reads,
/// delegates, or talks, and `write`/`edit` for the project store only
/// (`notes.md`, `docs/`, `internal/`, `media/`, `archived.md`; the write
/// guard in `arbos_engine::PlanCx::resolve_write` refuses the rest).
/// Nothing that runs a command.
pub const COORDINATOR_TOOLS: &[&str] = &[
    "ls",
    "read",
    "find",
    "grep",
    "search",
    "fetch",
    "write",
    "edit",
    "spawn",
    "say",
    "ask",
    "plan",
    "subscribe",
    "browser",
    "remember",
    "changes",
    "jobs",
    "screenshot",
    // `secret list` answers "do we have a key for X" without a value ever
    // showing; `use` arms a worker's bash through the kernel.
    "secret",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub root: RootConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RootConfig {
    /// `"coordinator"`: the main chat delegates and does not edit code.
    /// Absent: the main chat has every tool, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Move a finished worker's folder to `.arbos/archive/agents/<id>/`
    /// once its parent has read its done message. Off by default: a
    /// window showing that worker must know to look in the archive first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archive_children: bool,
}

/// Where finished workers go when `archive_children` is on.
pub fn archive_agents_dir(place: &Place) -> PathBuf {
    place.arbos().join("archive").join("agents")
}

pub fn path(place: &Place) -> PathBuf {
    place.arbos().join("project.toml")
}

/// The file as it is; an absent file is every default. A file that does
/// not parse is reported once by `check`; here it reads as defaults too,
/// so a typo never locks the main chat out of its tools.
pub fn load(place: &Place) -> ProjectConfig {
    let Ok(text) = std::fs::read_to_string(path(place)) else {
        return ProjectConfig::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn save(place: &Place, cfg: &ProjectConfig) -> Result<()> {
    let text = toml::to_string_pretty(cfg).context("serialise project.toml")?;
    let path = path(place);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// The file a new place starts with. Only when there is none: an existing
/// place is never changed under its user.
pub fn write_for_new_place(place: &Place, name: &str) -> Result<()> {
    if path(place).exists() {
        return Ok(());
    }
    let cfg = ProjectConfig {
        schema: Some(2),
        name: Some(name.to_string()),
        root: RootConfig {
            role: Some(COORDINATOR.into()),
            archive_children: false,
        },
    };
    save(place, &cfg)
}

/// Does the main chat of this place coordinate rather than edit?
pub fn root_is_coordinator(place: &Place) -> bool {
    load(place).root.role.as_deref() == Some(COORDINATOR)
}

/// The role as it applies to `agent`, in memory for this turn: a top-level
/// agent of a coordinator place keeps only the coordinator's tools. Not
/// saved — children inherit the parent's allowlist at spawn, and they are
/// the ones that edit.
pub fn apply_role(place: &Place, agent: &mut Agent) {
    if agent.parent.is_some() || !root_is_coordinator(place) {
        return;
    }
    agent.role = Some(COORDINATOR.into());
    agent
        .allowlist
        .retain(|t| COORDINATOR_TOOLS.contains(&t.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        Place::new(dir)
    }

    #[test]
    fn a_new_place_is_coordinated_and_an_old_one_is_not() {
        let p = place("new");
        assert!(!root_is_coordinator(&p));
        write_for_new_place(&p, "demo").unwrap();
        assert!(root_is_coordinator(&p));
        let text = std::fs::read_to_string(path(&p)).unwrap();
        assert!(
            text.contains("[root]") && text.contains("role = \"coordinator\""),
            "{text}"
        );
        // Written once: a later call leaves an edited file alone.
        std::fs::write(path(&p), "[root]\n").unwrap();
        write_for_new_place(&p, "demo").unwrap();
        assert!(!root_is_coordinator(&p));
    }

    #[test]
    fn the_role_narrows_the_top_agent_only_in_memory() {
        let p = place("role");
        write_for_new_place(&p, "demo").unwrap();
        let mut root = Agent::root("root");
        apply_role(&p, &mut root);
        assert_eq!(root.role.as_deref(), Some("coordinator"));
        assert!(root.may("spawn") && root.may("read") && root.may("say"));
        // The coordinator protocol (2026-09-13): root writes notes.md and
        // docs/project-context.md itself, so write and edit stay; the
        // write guard confines them to the store. Commands never.
        assert!(root.may("edit") && root.may("write"));
        assert!(!root.may("bash") && !root.may("terminal") && !root.may("undo"));
        assert!(!root.to_md().contains("coordinator"));
        let mut child = Agent::root("child");
        child.parent = Some(crate::AgentId::new("root"));
        apply_role(&p, &mut child);
        assert!(child.role.is_none() && child.may("edit"));
    }
}
