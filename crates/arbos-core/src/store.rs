//! The project's shared files under `.arbos/`, in Cursor's Agent Store
//! shape (decided 2026-09-13). One folder every agent of the place reads:
//!
//! - `notes.md`: the user-visible status page the main chat (root)
//!   rewrites after every state change. The right panel renders it as
//!   "Project".
//! - `docs/project-context.md`: stable goals, constraints, decisions,
//!   resources, dated. Every agent gets it in its prompt every turn. This
//!   is what `GOALS.md` was; `GOALS.md` stays as a symlink for one release.
//! - `docs/`: deliverables the user asked for. `internal/`: material for
//!   agents (audits, inboxes, handoffs). `media/<topic>/`: screenshots,
//!   recordings.
//! - `archived.md`: where finished or stale items of `notes.md` go, never
//!   deleted.
//!
//! Root owns `notes.md`, `docs/project-context.md`, and `archived.md`; a
//! child proposes a change with `say to=root`.

use std::path::{Path, PathBuf};

use crate::{Agent, Place};

/// Roughly 4 000 tokens. Over it, the prompt gets the head and a note.
pub const PROMPT_CAP_CHARS: usize = 16_000;

pub const NOTES: &str = "notes.md";
pub const ARCHIVED: &str = "archived.md";
pub const DOCS: &str = "docs";
pub const INTERNAL: &str = "internal";
pub const MEDIA: &str = "media";
pub const CONTEXT: &str = "project-context.md";
/// The old name of the context file; an alias for one release.
pub const GOALS_ALIAS: &str = "GOALS.md";

/// `<tldr>` may hold this many bullets.
pub const TLDR_CAP: usize = 4;
/// Checked items a section keeps; older ones move to `archived.md`.
pub const COMPLETED_CAP: usize = 3;

pub const CONTEXT_TEMPLATE: &str = r#"+++
owner = "root"
+++
# Project context

Stable goals, constraints, decisions, and resources. Progress lives in `notes.md`.

## Goal
(what this project is for, in one or two lines)

## Constraints
- 

## Decisions (dated, newest first)
- 

## Resources
(machines, accounts, vault items by name — never a secret's value)
- 
"#;

pub const NOTES_TEMPLATE: &str = r#"+++
owner = "root"
+++
# Notes

Goals, constraints, decisions: [project-context](docs/project-context.md)
"#;

pub const ARCHIVED_TEMPLATE: &str = r#"# Archived

Finished or stale items moved here from notes.md, newest first.
"#;

pub fn notes_path(place: &Place) -> PathBuf {
    place.arbos().join(NOTES)
}

pub fn archived_path(place: &Place) -> PathBuf {
    place.arbos().join(ARCHIVED)
}

pub fn docs_dir(place: &Place) -> PathBuf {
    place.arbos().join(DOCS)
}

pub fn internal_dir(place: &Place) -> PathBuf {
    place.arbos().join(INTERNAL)
}

pub fn media_dir(place: &Place) -> PathBuf {
    place.arbos().join(MEDIA)
}

/// `docs/project-context.md`.
pub fn context_path(place: &Place) -> PathBuf {
    docs_dir(place).join(CONTEXT)
}

/// `GOALS.md`: the alias at the store's root.
pub fn goals_alias_path(place: &Place) -> PathBuf {
    place.arbos().join(GOALS_ALIAS)
}

/// Make the folders and seed the templates. Never touches a file that
/// exists. A place with a real `GOALS.md` (an earlier release) has it
/// moved to `docs/project-context.md`, and `GOALS.md` becomes a symlink
/// to it so old readers keep working.
pub fn ensure(place: &Place) -> std::io::Result<()> {
    for dir in [docs_dir(place), internal_dir(place), media_dir(place)] {
        std::fs::create_dir_all(dir)?;
    }
    let context = context_path(place);
    let alias = goals_alias_path(place);
    if !context.exists() {
        // symlink_metadata: a dangling alias must not count as a file.
        let alias_is_file = std::fs::symlink_metadata(&alias)
            .map(|m| m.is_file())
            .unwrap_or(false);
        if alias_is_file {
            std::fs::rename(&alias, &context)?;
        } else {
            std::fs::write(&context, CONTEXT_TEMPLATE)?;
        }
    }
    if std::fs::symlink_metadata(&alias).is_err() {
        // Best effort: a file system without symlinks has no alias.
        let _ = symlink_alias(&alias);
    }
    for (path, template) in [
        (notes_path(place), NOTES_TEMPLATE),
        (archived_path(place), ARCHIVED_TEMPLATE),
    ] {
        if !path.exists() {
            std::fs::write(path, template)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_alias(alias: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(Path::new(DOCS).join(CONTEXT), alias)
}

#[cfg(not(unix))]
fn symlink_alias(_alias: &Path) -> std::io::Result<()> {
    Ok(())
}

/// The files only root writes: `notes.md`, `docs/project-context.md`
/// (and its `GOALS.md` alias), `archived.md`. Both sides canonicalised
/// when they exist, so a symlinked place still matches.
pub fn is_root_owned(place_root: &Path, candidate: &Path) -> bool {
    let arbos = place_root.join(".arbos");
    let owned = [
        arbos.join(NOTES),
        arbos.join(ARCHIVED),
        arbos.join(DOCS).join(CONTEXT),
        arbos.join(GOALS_ALIAS),
    ];
    let b = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    owned.iter().any(|p| {
        let a = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        a == b || *p == candidate
    })
}

/// Is `candidate` inside the store's shared folders a coordinator may
/// write: `notes.md`, `archived.md`, `docs/`, `internal/`, `media/`.
pub fn is_store_path(place_root: &Path, candidate: &Path) -> bool {
    let arbos = place_root.join(".arbos");
    let real = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    let arbos_real = std::fs::canonicalize(&arbos).unwrap_or_else(|_| arbos.clone());
    let rel = match real
        .strip_prefix(&arbos_real)
        .or_else(|_| candidate.strip_prefix(&arbos))
    {
        Ok(rel) => rel,
        Err(_) => return false,
    };
    let Some(head) = rel.components().next() else {
        return false;
    };
    let head = head.as_os_str().to_string_lossy();
    matches!(
        head.as_ref(),
        NOTES | ARCHIVED | GOALS_ALIAS | DOCS | INTERNAL | MEDIA
    )
}

/// Only root (the main chat) writes the root-owned files. Everyone else,
/// a child or a second top-level chat, proposes.
pub fn may_write(agent: &Agent) -> bool {
    agent.id.as_str() == crate::ROOT_ID
}

pub const REFUSAL: &str = ".arbos/notes.md (the project page), docs/project-context.md, and archived.md are owned by the main chat (root); keep your own checklist with the plan tool and propose the change with `say to=root`";

/// Why a coordinator's write outside the store is refused.
pub const COORDINATOR_REFUSAL: &str = "as coordinator you write only the project store (.arbos/notes.md, docs/, internal/, media/, archived.md); code and other files are a worker's job — spawn one";

/// The prompt segment: the context file's text under the cap, or nothing
/// when it is still the untouched template or empty.
pub fn prompt_segment(place: &Place) -> Option<String> {
    let p = context_path(place);
    let text = std::fs::read_to_string(&p).ok()?;
    if text.trim().is_empty() || text == CONTEXT_TEMPLATE {
        return None;
    }
    let body = strip_front_matter(&text);
    let shown: String = if body.chars().count() > PROMPT_CAP_CHARS {
        let head: String = body.chars().take(PROMPT_CAP_CHARS).collect();
        format!(
            "{head}\n[project-context.md is over {PROMPT_CAP_CHARS} characters; only this much is shown. Root: trim it or move detail to docs/.]"
        )
    } else {
        body.to_string()
    };
    Some(format!(
        "docs/project-context.md ({}) — the project's goals, constraints, decisions, and resources. Read it as the standing brief; only the main chat edits it. Status lives in .arbos/notes.md.\n{shown}",
        p.display()
    ))
}

pub fn strip_front_matter(text: &str) -> &str {
    for fence in ["+++", "---"] {
        if let Some(rest) = text.strip_prefix(fence)
            && let Some(end) = rest.find(&format!("\n{fence}"))
        {
            return rest[end + 1 + fence.len()..].trim_start_matches('\n');
        }
    }
    text
}

/// The kickoff brief a coordinator gives a worker (protocol section 6):
/// six labelled lines, each carrying paths rather than content. Every
/// field but `task` has a default, so a lazy call still yields a whole
/// brief.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kickoff<'a> {
    pub read_first: Option<&'a str>,
    pub task: &'a str,
    pub do_: Option<&'a str>,
    pub rules: Option<&'a str>,
    pub output: Option<&'a str>,
    pub report: Option<&'a str>,
}

pub const KICKOFF_READ_FIRST: &str = ".arbos/docs/project-context.md, then .arbos/notes.md";
pub const KICKOFF_RULES: &str = "Stay on the base branch you are given; never merge. No extra documents beyond what the task needs. Secrets come through `secret` by name, never printed; redact them in captures.";
pub const KICKOFF_OUTPUT: &str = "Deliverables under .arbos/docs/, working notes under .arbos/internal/, captures under .arbos/media/<topic>/. Verify each file exists before you report it.";
pub const KICKOFF_REPORT: &str = "A few lines: the outcome, a link to every file and PR you made, and open questions (at most four).";

impl Kickoff<'_> {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut line = |label: &str, text: &str| {
            let text = text.trim();
            if text.is_empty() {
                return;
            }
            out.push_str(label);
            out.push_str(": ");
            if text.contains('\n') {
                out.push('\n');
                for l in text.lines() {
                    out.push_str("  ");
                    out.push_str(l.trim_end());
                    out.push('\n');
                }
            } else {
                out.push_str(text);
                out.push('\n');
            }
        };
        line("Read first", self.read_first.unwrap_or(KICKOFF_READ_FIRST));
        line("Task", self.task);
        line(
            "Do",
            self.do_
                .unwrap_or("as the task says; number your steps in your first reply"),
        );
        line("Rules", self.rules.unwrap_or(KICKOFF_RULES));
        line("Output", self.output.unwrap_or(KICKOFF_OUTPUT));
        line("Report", self.report.unwrap_or(KICKOFF_REPORT));
        out
    }
}

/// One thing wrong with a `notes.md`, and the 1-based line it sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotesProblem {
    pub line: usize,
    pub what: String,
}

/// Lint the status page's shape (protocol section 5): every item is a
/// checkbox whose text starts with a `[label](target)` link; `<tldr>`
/// holds at most [`TLDR_CAP`] bullets, each with a link; a section keeps
/// at most [`COMPLETED_CAP`] checked items, and they come last.
pub fn lint_notes(text: &str) -> Vec<NotesProblem> {
    let mut out = Vec::new();
    let body = strip_front_matter(text);
    let offset = text.lines().count() - body.lines().count();
    // Section 5: the top line links the context document. Checked on the
    // preamble (everything before the first heading or item).
    let preamble_links_context = body
        .lines()
        .take_while(|l| {
            let t = l.trim_start();
            !t.starts_with("## ") && !t.starts_with("### ") && !t.starts_with("- ")
        })
        .any(|l| l.contains("](docs/project-context.md)") || l.contains("](GOALS.md)"));
    if !preamble_links_context {
        out.push(NotesProblem {
            line: offset + 1,
            what: "no top line linking docs/project-context.md before the first section".into(),
        });
    }
    let mut in_tldr = false;
    let mut tldr_bullets = 0usize;
    let mut in_fence = false;
    // Per section: checked items seen, and whether an open item followed one.
    let mut checked = 0usize;
    let mut open_after_checked = false;
    let mut section_line = 0usize;
    let mut section_name = String::from("(top)");
    let close_section = |out: &mut Vec<NotesProblem>, checked: usize, line: usize, name: &str| {
        if checked > COMPLETED_CAP {
            out.push(NotesProblem {
                    line,
                    what: format!(
                        "section {name:?} keeps {checked} completed items; cap is {COMPLETED_CAP}, move the rest to archived.md"
                    ),
                });
        }
    };
    for (i, raw) in body.lines().enumerate() {
        let n = offset + i + 1;
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if t == "<tldr>" {
            in_tldr = true;
            tldr_bullets = 0;
            continue;
        }
        if t == "</tldr>" {
            in_tldr = false;
            continue;
        }
        if in_tldr {
            if let Some(rest) = t.strip_prefix("- ") {
                tldr_bullets += 1;
                if tldr_bullets == TLDR_CAP + 1 {
                    out.push(NotesProblem {
                        line: n,
                        what: format!("<tldr> has more than {TLDR_CAP} bullets"),
                    });
                }
                if link_label(rest).is_none() {
                    out.push(NotesProblem {
                        line: n,
                        what: "tldr bullet has no [label](target) link".into(),
                    });
                }
            }
            continue;
        }
        if t.starts_with("## ") || t.starts_with("### ") {
            close_section(&mut out, checked, section_line, &section_name);
            checked = 0;
            open_after_checked = false;
            section_line = n;
            section_name = t.trim_start_matches('#').trim().to_string();
            continue;
        }
        // Items: `- [ ]` / `- [x]`, at any indent. A plain `- ` bullet
        // under a section is an item without its checkbox.
        let Some(bullet) = t.strip_prefix("- ") else {
            continue;
        };
        let (done, rest) = if let Some(r) = bullet.strip_prefix("[ ] ") {
            (false, r)
        } else if let Some(r) = bullet
            .strip_prefix("[x] ")
            .or_else(|| bullet.strip_prefix("[X] "))
        {
            (true, r)
        } else {
            out.push(NotesProblem {
                line: n,
                what: "item is not a checkbox (`- [ ] ` or `- [x] `)".into(),
            });
            continue;
        };
        if done {
            checked += 1;
        } else if checked > 0 && t.starts_with("- ") && !raw.starts_with(' ') {
            // Only top-level items order-check; nested children follow
            // their parent.
            if !open_after_checked {
                out.push(NotesProblem {
                    line: n,
                    what:
                        "open item after a completed one; completed items go last in their section"
                            .into(),
                });
            }
            open_after_checked = true;
        }
        match link_label(rest) {
            None => out.push(NotesProblem {
                line: n,
                what: "item does not start with a [label](target) link".into(),
            }),
            Some(label) if label.chars().count() > 60 => out.push(NotesProblem {
                line: n,
                what: "link label is over 60 characters; keep it a short name".into(),
            }),
            Some(_) => {}
        }
    }
    close_section(&mut out, checked, section_line, &section_name);
    out
}

/// The label of a `[label](target)` link at the start of `s`.
fn link_label(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let rest = s.strip_prefix('[')?;
    let close = rest.find("](")?;
    let after = &rest[close + 2..];
    after.find(')')?;
    let label = &rest[..close];
    (!label.trim().is_empty()).then_some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(tag: &str) -> Place {
        let dir = std::env::temp_dir().join(format!("arbos-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".arbos")).unwrap();
        Place::new(dir)
    }

    #[test]
    fn ensure_seeds_the_store_and_aliases_goals() {
        let p = place("seed");
        ensure(&p).unwrap();
        assert!(docs_dir(&p).is_dir() && internal_dir(&p).is_dir() && media_dir(&p).is_dir());
        assert_eq!(
            std::fs::read_to_string(context_path(&p)).unwrap(),
            CONTEXT_TEMPLATE
        );
        assert_eq!(
            std::fs::read_to_string(notes_path(&p)).unwrap(),
            NOTES_TEMPLATE
        );
        assert!(archived_path(&p).is_file());
        #[cfg(unix)]
        {
            let alias = goals_alias_path(&p);
            assert!(std::fs::symlink_metadata(&alias).unwrap().is_symlink());
            assert_eq!(
                std::fs::read_to_string(&alias).unwrap(),
                CONTEXT_TEMPLATE,
                "GOALS.md reads as the context file"
            );
        }
        assert!(prompt_segment(&p).is_none(), "the template is not injected");
    }

    #[test]
    fn an_old_goals_file_moves_into_docs() {
        let p = place("migrate");
        std::fs::write(
            goals_alias_path(&p),
            "+++\nowner = \"root\"\n+++\n# demo\n\n## Goal\nShip it.\n",
        )
        .unwrap();
        ensure(&p).unwrap();
        let text = std::fs::read_to_string(context_path(&p)).unwrap();
        assert!(text.contains("Ship it."));
        let seg = prompt_segment(&p).unwrap();
        assert!(seg.contains("## Goal\nShip it."), "{seg}");
        assert!(!seg.contains("owner ="), "{seg}");
        // A second ensure changes nothing.
        ensure(&p).unwrap();
        assert!(
            std::fs::read_to_string(context_path(&p))
                .unwrap()
                .contains("Ship it.")
        );
    }

    #[test]
    fn over_the_cap_the_head_is_shown_with_a_note() {
        let p = place("cap");
        ensure(&p).unwrap();
        std::fs::write(context_path(&p), "x".repeat(PROMPT_CAP_CHARS + 50)).unwrap();
        let seg = prompt_segment(&p).unwrap();
        assert!(seg.contains("only this much is shown"));
    }

    #[test]
    fn root_owns_the_status_and_context_files() {
        let root = Agent::root("root");
        assert!(may_write(&root));
        let mut child = Agent::root("c");
        child.parent = Some(crate::AgentId::new("root"));
        assert!(!may_write(&child));
        let p = place("owned");
        ensure(&p).unwrap();
        for owned in [
            notes_path(&p),
            context_path(&p),
            archived_path(&p),
            goals_alias_path(&p),
        ] {
            assert!(is_root_owned(&p.path, &owned), "{}", owned.display());
            assert!(is_store_path(&p.path, &owned), "{}", owned.display());
        }
        assert!(!is_root_owned(&p.path, &p.path.join("GOALS.md")));
        assert!(!is_root_owned(&p.path, &docs_dir(&p).join("design.md")));
        assert!(is_store_path(&p.path, &docs_dir(&p).join("design.md")));
        assert!(is_store_path(&p.path, &media_dir(&p).join("layout/a.png")));
        assert!(!is_store_path(&p.path, &p.path.join("src/main.rs")));
        assert!(!is_store_path(
            &p.path,
            &p.arbos().join("agents/root/plan.md")
        ));
    }

    #[test]
    fn a_kickoff_renders_six_lines_with_defaults() {
        let brief = Kickoff {
            task: "Fix the echo gate.",
            do_: Some("1. Read gateway.rs\n2. Add the gate\n3. Test"),
            output: Some(".arbos/docs/echo-gate.md"),
            ..Kickoff::default()
        }
        .render();
        assert!(
            brief.starts_with("Read first: .arbos/docs/project-context.md"),
            "{brief}"
        );
        assert!(brief.contains("Task: Fix the echo gate.\n"), "{brief}");
        assert!(
            brief.contains("Do: \n  1. Read gateway.rs\n  2. Add the gate\n  3. Test\n"),
            "{brief}"
        );
        assert!(brief.contains("Rules: Stay on the base branch"), "{brief}");
        assert!(
            brief.contains("Output: .arbos/docs/echo-gate.md\n"),
            "{brief}"
        );
        assert!(brief.contains("Report: A few lines"), "{brief}");
    }

    #[test]
    fn the_notes_lint_accepts_the_protocol_shape() {
        let good = "+++\nowner = \"root\"\n+++\n# Demo\n\nGoals: [project-context](docs/project-context.md)\n\n<tldr>\n- [Voice PR](https://x/1) — live on the pod\n</tldr>\n\n## Voice\n- [ ] [Voice PR](https://x/1) — live on the pod; DNS is Jacob's\n  - [ ] [Worker](agents/fix-echo) — echo gate landing\n- [x] [Research](docs/r.md) — delivered\n";
        assert_eq!(lint_notes(good), Vec::<NotesProblem>::new());
        assert!(lint_notes(NOTES_TEMPLATE).is_empty());
    }

    #[test]
    fn the_notes_lint_names_each_shape_fault() {
        let bad = "# Demo\n\n<tldr>\n- [a](x) — 1\n- [b](x) — 2\n- [c](x) — 3\n- [d](x) — 4\n- five without link\n</tldr>\n\n## S\n- [x] [done1](x) — a\n- [ ] [open](x) — b\n- plain bullet\n- [ ] no link here\n- [x] [done2](x) — c\n- [x] [done3](x) — d\n- [x] [done4](x) — e\n";
        let found = lint_notes(bad);
        let whats: Vec<&str> = found.iter().map(|p| p.what.as_str()).collect();
        assert!(whats.iter().any(|w| w.contains("no top line linking")), "{whats:?}");
        assert!(
            whats.iter().any(|w| w.contains("more than 4 bullets")),
            "{whats:?}"
        );
        assert!(
            whats.iter().any(|w| w.contains("tldr bullet has no")),
            "{whats:?}"
        );
        assert!(
            whats
                .iter()
                .any(|w| w.contains("open item after a completed")),
            "{whats:?}"
        );
        assert!(
            whats.iter().any(|w| w.contains("not a checkbox")),
            "{whats:?}"
        );
        assert!(
            whats
                .iter()
                .any(|w| w.contains("does not start with a [label]")),
            "{whats:?}"
        );
        assert!(
            whats.iter().any(|w| w.contains("keeps 4 completed items")),
            "{whats:?}"
        );
        assert_eq!(
            found
                .iter()
                .filter(|p| p.what.contains("more than"))
                .count(),
            1
        );
    }
}
