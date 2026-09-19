//! `agents/<id>/waiting/`: what an agent waits on a person for, as files.
//!
//! `ask-<id>.toml` is a parked question: the turn ended with it, and the
//! answer — an inbox file of `kind = "answer"` — starts the next turn. A
//! kernel restart changes nothing: the file is still there, the answer is
//! still taken. `approve-<id>.toml` mirrors a blocking allow/deny prompt so
//! a reader of the folder sees why the turn stands still; the decision
//! removes it (design Phase 6, revised to Cursor's shape).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Place;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Waiting {
    /// `ask` or `approve`.
    pub kind: String,
    /// The id an answer must name: the ask tool's call id, or `approve-N`.
    pub id: String,
    /// The question, or `allow <tool>: <command>`.
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// `approve`: the tool asked about.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    pub asked: String,
}

pub fn dir(place: &Place, agent: &str) -> PathBuf {
    place.agent_dir(agent).join("waiting")
}

fn path_for(place: &Place, agent: &str, kind: &str, id: &str) -> PathBuf {
    dir(place, agent).join(format!("{kind}-{}.toml", safe(id)))
}

fn safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn write(place: &Place, agent: &str, w: &Waiting) -> Result<PathBuf> {
    let dir = dir(place, agent);
    std::fs::create_dir_all(&dir)?;
    let path = path_for(place, agent, &w.kind, &w.id);
    let text = toml::to_string_pretty(w).context("serialise waiting file")?;
    let tmp = dir.join(format!(".{}.{}.tmp", safe(&w.id), std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn read(path: &Path) -> Result<Waiting> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Every waiting file of `agent`, oldest first.
pub fn list(place: &Place, agent: &str) -> Vec<Waiting> {
    let Ok(rd) = std::fs::read_dir(dir(place, agent)) else {
        return Vec::new();
    };
    let mut out: Vec<Waiting> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| read(&p).ok())
        .collect();
    out.sort_by(|a, b| a.asked.cmp(&b.asked));
    out
}

/// The parked questions of `agent`.
pub fn asks(place: &Place, agent: &str) -> Vec<Waiting> {
    list(place, agent)
        .into_iter()
        .filter(|w| w.kind == "ask")
        .collect()
}

/// Whether the transcript shows the agent woke after asking `ask_id`:
/// an `ask` line with that id followed by a `wake` — the question's turn
/// is over and another began, so nobody is waiting on it. False when the
/// ask is the latest thing, or is not on the transcript at all (a
/// question parked by a kernel whose transcript write was lost keeps
/// standing rather than be dropped on a guess).
pub fn ask_is_stale(events: &[crate::Event], ask_id: &str) -> bool {
    let Some(at) = events.iter().rposition(
        |e| matches!(&e.kind, crate::EventKind::Ask { call_id: Some(id), .. } if id == ask_id),
    ) else {
        return false;
    };
    events[at + 1..].iter().any(|e| e.is_wake())
}

pub fn remove(place: &Place, agent: &str, kind: &str, id: &str) -> bool {
    std::fs::remove_file(path_for(place, agent, kind, id)).is_ok()
}

/// Drop every approve mirror: the turn they belonged to is gone (a kernel
/// start), and a blocking prompt does not survive it.
pub fn clear_approves(place: &Place, agent: &str) -> usize {
    let mut n = 0;
    for w in list(place, agent) {
        if w.kind == "approve" && remove(place, agent, "approve", &w.id) {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parked_ask_survives_a_listing_and_is_removed_by_id() {
        let dir = std::env::temp_dir().join(format!("arbos-waiting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos/agents/root")).unwrap();
        let p = Place::new(dir);
        let w = Waiting {
            kind: "ask".into(),
            id: "call_ABC/1".into(),
            question: "alpha or beta?".into(),
            options: vec!["alpha".into(), "beta".into()],
            tool: String::new(),
            asked: "2026-09-13T10:00:00Z".into(),
        };
        write(&p, "root", &w).unwrap();
        assert_eq!(asks(&p, "root"), vec![w.clone()]);
        let a = Waiting {
            kind: "approve".into(),
            id: "approve-1".into(),
            question: "allow bash: rm".into(),
            options: vec![],
            tool: "bash".into(),
            asked: "2026-09-13T10:00:01Z".into(),
        };
        write(&p, "root", &a).unwrap();
        assert_eq!(list(&p, "root").len(), 2);
        assert_eq!(clear_approves(&p, "root"), 1);
        assert!(remove(&p, "root", "ask", "call_ABC/1"));
        assert!(list(&p, "root").is_empty());
    }
}
