//! `.arbos/GOALS.md`: the one goals file of a place. The main chat owns it;
//! every agent reads it first, every turn (design Phase 5a, moved up:
//! Cursor's `docs/project-context.md`).

use std::path::{Path, PathBuf};

use crate::{Agent, Place};

/// Roughly 4 000 tokens. Over it, the prompt gets the head and a note.
pub const PROMPT_CAP_CHARS: usize = 16_000;

pub const TEMPLATE: &str = r#"+++
owner = "root"
+++
# Goals

## Goal
(what this place is for, in one or two lines)

## Constraints
- 

## Decisions (dated, newest first)
- 

## Current focus
- 
"#;

pub fn path(place: &Place) -> PathBuf {
    place.arbos().join("GOALS.md")
}

/// Write the template when the file is missing. Never touches an existing one.
pub fn ensure(place: &Place) -> std::io::Result<()> {
    let p = path(place);
    if p.exists() {
        return Ok(());
    }
    std::fs::write(p, TEMPLATE)
}

/// Is `candidate` this place's GOALS.md? Both sides canonicalised when
/// they exist, so a symlinked place still matches.
pub fn is_goals_path(place_root: &Path, candidate: &Path) -> bool {
    let goals = place_root.join(".arbos").join("GOALS.md");
    let a = std::fs::canonicalize(&goals).unwrap_or(goals);
    let b = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    a == b
}

/// Only a top-level agent (the main chat) writes GOALS.md. A child proposes.
pub fn may_write(agent: &Agent) -> bool {
    agent.parent.is_none()
}

pub const REFUSAL: &str =
    "GOALS.md is owned by the main chat (root); propose the change with `say to=root`";

/// The prompt segment: the file's text under the cap, or nothing when it
/// is still the untouched template or empty.
pub fn prompt_segment(place: &Place) -> Option<String> {
    let p = path(place);
    let text = std::fs::read_to_string(&p).ok()?;
    if text.trim().is_empty() || text == TEMPLATE {
        return None;
    }
    let body = strip_front_matter(&text);
    let shown: String = if body.chars().count() > PROMPT_CAP_CHARS {
        let head: String = body.chars().take(PROMPT_CAP_CHARS).collect();
        format!(
            "{head}\n[GOALS.md is over {PROMPT_CAP_CHARS} characters; only this much is shown. Root: trim it or move detail to shared/.]"
        )
    } else {
        body.to_string()
    };
    Some(format!(
        "GOALS.md ({}) — the place's goals, constraints, decisions, and current focus. Read it as the standing brief; only the main chat edits it.\n{shown}",
        p.display()
    ))
}

fn strip_front_matter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("+++") else {
        return text;
    };
    match rest.find("\n+++") {
        Some(end) => rest[end + 4..].trim_start_matches('\n'),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-goals-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        Place::new(dir)
    }

    #[test]
    fn the_template_is_not_injected_but_real_goals_are() {
        let p = place("seg");
        ensure(&p).unwrap();
        assert!(prompt_segment(&p).is_none());
        std::fs::write(
            path(&p),
            "+++\nowner = \"root\"\n+++\n# demo\n\n## Goal\nShip it.\n",
        )
        .unwrap();
        let seg = prompt_segment(&p).unwrap();
        assert!(seg.contains("## Goal\nShip it."), "{seg}");
        assert!(!seg.contains("owner ="), "{seg}");
        ensure(&p).unwrap();
        assert!(
            std::fs::read_to_string(path(&p))
                .unwrap()
                .contains("Ship it.")
        );
    }

    #[test]
    fn over_the_cap_the_head_is_shown_with_a_note() {
        let p = place("cap");
        std::fs::write(path(&p), "x".repeat(PROMPT_CAP_CHARS + 50)).unwrap();
        let seg = prompt_segment(&p).unwrap();
        assert!(seg.contains("only this much is shown"));
    }

    #[test]
    fn only_a_top_level_agent_writes() {
        let root = Agent::root("root");
        assert!(may_write(&root));
        let mut child = Agent::root("c");
        child.parent = Some(crate::AgentId::new("root"));
        assert!(!may_write(&child));
        let p = place("path");
        assert!(is_goals_path(&p.path, &path(&p)));
        assert!(!is_goals_path(&p.path, &p.path.join("GOALS.md")));
    }
}
