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

/// The role every child gets unless its kind says otherwise: it does its
/// own task and does not delegate. A coordinator's brief that describes
/// several workers must not turn each worker into a coordinator (three
/// workers spawned nine grandchildren from one ask).
pub const WORKER: &str = "worker";

/// What a coordinator keeps of the tool set: everything that reads,
/// delegates, or talks, and `write`/`edit` for the project store only
/// (`notes.md`, `docs/`, `internal/`, `media/`, `archived.md`; the write
/// guard in `arbos_engine::PlanCx::resolve_write` refuses the rest).
/// `bash` for the one quick command the user asks to see run — Cursor's
/// coordinator runs those itself ("Ran 1 command"), and a coordinator
/// without a shell answered "I can't run shell commands" to the same
/// prompt (symmetry loop, cycle 3). The contract keeps it to that; a
/// build, a test run, or an edit is a worker's. `terminal` and `secret`
/// follow `bash` (`Agent::may`): the same quick command in a visible
/// pane, and a key by name for it. No `undo`.
pub const COORDINATOR_TOOLS: &[&str] = &[
    "bash",
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
    "status",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// An agent that opens a pull request follows it: the kernel adds a
    /// `github_pr` and a `github_ci` subscription for it, so a failing
    /// check or a review comment wakes the agent that made the change.
    /// On unless `follow_prs = false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_prs: Option<bool>,
    #[serde(default)]
    pub root: RootConfig,
}

impl ProjectConfig {
    pub fn follows_prs(&self) -> bool {
        self.follow_prs.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RootConfig {
    /// `"coordinator"`: the main chat delegates and does not edit code.
    /// Absent: the main chat has every tool, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Move a finished worker's folder to `.arbos/archive/agents/<id>/`
    /// once its parent has read its done message. On unless the file says
    /// `archive_children = false` (since 2026-09-14: the window closes a
    /// chat whose agent is gone instead of reconnecting to it, #129/#138).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_children: Option<bool>,
}

impl RootConfig {
    /// Whether finished workers are archived: the line, or on.
    pub fn archives_children(&self) -> bool {
        self.archive_children.unwrap_or(true)
    }
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
        follow_prs: None,
        root: RootConfig {
            role: Some(COORDINATOR.into()),
            archive_children: Some(true),
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
    if agent.parent.is_some() {
        // A child minted before roles were saved: kind-less means worker.
        if agent.role.is_none() && agent.kind.is_empty() {
            agent.role = Some(WORKER.into());
        }
        return;
    }
    if !root_is_coordinator(place) {
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
    fn finished_workers_are_archived_unless_the_file_says_no() {
        let p = place("archive");
        // No file, or a file without the line: on.
        assert!(load(&p).root.archives_children());
        write_for_new_place(&p, "demo").unwrap();
        assert!(load(&p).root.archives_children());
        assert!(
            std::fs::read_to_string(path(&p))
                .unwrap()
                .contains("archive_children = true")
        );
        std::fs::write(
            path(&p),
            "schema = 2\nname = \"old\"\n[root]\nrole = \"coordinator\"\n",
        )
        .unwrap();
        assert!(load(&p).root.archives_children());
        // The line off: off.
        std::fs::write(path(&p), "schema = 2\n[root]\narchive_children = false\n").unwrap();
        assert!(!load(&p).root.archives_children());
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
        // write guard confines them to the store. bash for one quick
        // command the user asks to see run (symmetry cycle 3), and the
        // pane that follows it; no undo.
        assert!(root.may("edit") && root.may("write") && root.may("bash"));
        assert!(root.may("terminal") && !root.may("undo"));
        assert!(!root.to_md().contains("coordinator"));
        // A kind-less child is a worker: it keeps every tool and gets the
        // worker line, never the coordinator's.
        let mut child = Agent::root("child");
        child.parent = Some(crate::AgentId::new("root"));
        apply_role(&p, &mut child);
        assert_eq!(child.role.as_deref(), Some(WORKER));
        assert!(child.may("edit") && child.may("bash"));
        let mut kinded = Agent::root("reviewer-1");
        kinded.parent = Some(crate::AgentId::new("root"));
        kinded.kind = "reviewer".into();
        apply_role(&p, &mut kinded);
        assert!(kinded.role.is_none());
    }
}
