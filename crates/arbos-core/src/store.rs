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

use crate::{Agent, Event, EventKind, Layout, Place};

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

/// Files that shape how agents here behave: the protocol, kinds, skills,
/// memory, hooks, secrets, doors, sandbox, access, project and git
/// settings, MCP servers, and the repository's own agent instructions.
/// A write to one of these asks the user first, in every mode, so an
/// agent led astray by something it read cannot rewrite its own standing
/// orders (Hermes 0.21 protects AGENTS.md, skills and memory the same
/// way). Paths relative to the place; a trailing `/` marks a folder.
pub const PROTECTED: &[&str] = &[
    ".arbos/PROTOCOL.md",
    ".arbos/agents-defs/",
    ".arbos/skills/",
    ".arbos/memory.md",
    ".arbos/hooks.toml",
    ".arbos/secrets.toml",
    ".arbos/doors.toml",
    ".arbos/sandbox.toml",
    ".arbos/access.toml",
    ".arbos/project.toml",
    ".arbos/git.toml",
    ".arbos/mcp.toml",
    ".cursor/agents/",
    ".cursor/mcp.json",
    ".cursor/rules/",
    ".arbos/rules/",
    "AGENTS.md",
    "CLAUDE.md",
];

/// The protected entry `candidate` falls under, as its relative spelling,
/// or None. Both sides canonicalised when they exist.
pub fn protected_by(place_root: &Path, candidate: &Path) -> Option<&'static str> {
    let real = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    let root_real = std::fs::canonicalize(place_root).unwrap_or_else(|_| place_root.to_path_buf());
    for entry in PROTECTED {
        let rel = entry.trim_end_matches('/');
        let full = root_real.join(rel);
        let full = std::fs::canonicalize(&full).unwrap_or(full);
        let hit = if entry.ends_with('/') {
            real.starts_with(&full)
        } else {
            real == full
        };
        if hit {
            return Some(entry);
        }
    }
    // An agent's standing instructions from its kind.
    let agents = root_real.join(".arbos").join("agents");
    if let Ok(rest) = real.strip_prefix(&agents)
        && rest.components().count() == 2
        && rest.file_name().is_some_and(|f| f == "instructions.md")
    {
        return Some(".arbos/agents/<id>/instructions.md");
    }
    None
}

/// Does a shell command look like it writes into a protected file? The
/// command names one of them (by its relative spelling or file name) and
/// carries something that writes: a redirection, `tee`, `sed -i`, `cp`,
/// `mv`, `rm`, `truncate`, `install`, `git checkout --`, `patch`.
pub fn bash_writes_protected(command: &str) -> Option<&'static str> {
    let named: Option<&'static str> = PROTECTED
        .iter()
        .copied()
        .find(|entry| {
            let rel = entry.trim_end_matches('/');
            let name = rel.rsplit('/').next().unwrap_or(rel);
            command.contains(rel) || (name.contains('.') && command.contains(name))
        })
        .or_else(|| {
            command
                .contains("instructions.md")
                .then_some(".arbos/agents/<id>/instructions.md")
        });
    let named = named?;
    let writes = command.contains('>')
        || [
            "tee ",
            "sed -i",
            "cp ",
            "mv ",
            "rm ",
            "truncate ",
            "install ",
            "patch ",
            "git checkout --",
            "perl -i",
            "python -c",
            "python3 -c",
        ]
        .iter()
        .any(|w| command.contains(w));
    writes.then_some(named)
}

/// The line the user sees when a tool wants to change a protected file.
pub fn protected_question(tool: &str, entry: &str) -> String {
    format!("{tool} wants to change {entry}, a file that shapes how agents here behave. Allow it?")
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
    /// The repository's current branch at the place, when it is one: the
    /// default rules name it as the base branch instead of "the base
    /// branch you are given" (the audit found `rules` copied verbatim).
    pub base_branch: Option<String>,
    /// The user asked to see the result ("show me", "let me see", a
    /// screenshot). Root's brief tended to say "run … and report", and
    /// the image was never made (kickoff item 3). The line tells the
    /// worker an image is owed.
    pub show: bool,
}

/// The line a brief gets when the user asked to see the result.
/// The Show step, numbered after the brief's own steps ("3. The user
/// asked to see …"). `do_text` decides the number: one past the last
/// `N.` at a line start, else 2 (after the one default step).
pub fn show_step(do_text: &str) -> String {
    let last = do_text
        .lines()
        .filter_map(|l| {
            let t = l.trim_start();
            let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return None;
            }
            let rest = &t[digits.len()..];
            (rest.starts_with(". ") || rest.starts_with(") ") || rest == ".")
                .then(|| digits.parse::<usize>().ok())
                .flatten()
        })
        .max()
        .unwrap_or(1);
    format!(
        "{}. The user asked to see the result: make the image and name its path in your report — a command's output: `screenshot target:\"text\" title:\"<the command>\" text:\"<its output>\"`; a page: `browser action:screenshot`; a window: `screenshot target:\"window\"`. It lands in your images/ folder (.arbos/agents/<you>/images/<name>.png); that path goes in the report.",
        last + 1
    )
}

pub const KICKOFF_SHOW: &str = "The user asked to see this. An image of the result is owed: `browser action:screenshot` for a page, `screenshot target:window` for a window, `screenshot target:text title:\"<the command>\" text:\"<its output>\"` for a command's output (no display needed). Name the image path in your report; words alone do not close the task.";

/// Does this message ask to be shown something? Judged on the user's own
/// words: "show me", "let me see", "screenshot", "I want to see it".
/// Does a brief already ask for an image itself, so the kernel's `Show`
/// line would repeat it? Only words that mean a picture count: "Capture
/// output" in a `do` step means stdout, and it kept the line out of a
/// brief whose user had said "show me the output" (kickoff item 3).
pub fn names_an_image(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "screenshot",
        "screen shot",
        "image",
        "picture",
        ".png",
        ".jpg",
        "render",
    ]
    .iter()
    .any(|w| t.contains(w))
}

pub fn asks_to_see(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    [
        "show me",
        "show us",
        "show it",
        "show the",
        "show what",
        "show how",
        "let me see",
        "let us see",
        "i want to see",
        "i'd like to see",
        "i would like to see",
        "can i see",
        "so i can see",
        "screenshot",
        "screen shot",
        "send me a picture",
        "send a picture",
        "take a picture",
        "what it looks like",
        "what does it look like",
        "see it running",
        "see it work",
    ]
    .iter()
    .any(|p| t.contains(p))
}

/// The user message that opened the turn now running for `agent`: the
/// `user` line(s) after the last `wake`. None for a turn a child's done
/// or a timer opened.
pub fn turn_user_text(place: &Place, agent: &str) -> Option<String> {
    let events = crate::load_transcript(&Layout::new(place, agent).transcript()).ok()?;
    let start = events.iter().rposition(Event::is_wake)?;
    let text: Vec<&str> = events[start..]
        .iter()
        .filter(|e| matches!(e.kind, EventKind::User { .. }))
        .filter_map(Event::user_text)
        .filter(|t| !t.trim().is_empty() && !t.starts_with("[kernel]"))
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

/// The branch checked out at `place`, or None when it is no repository or
/// is detached.
pub fn current_branch(place: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(place)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub const KICKOFF_READ_FIRST: &str = ".arbos/docs/project-context.md, then .arbos/notes.md";
pub const KICKOFF_RULES: &str = "Start from the base branch you are given; a code fix goes on its own branch (`git checkout -b fix/<what>`), committed and pushed there, and comes back as a draft pull request (`pr create`) against that base — never a commit on the base branch; never merge. No extra documents beyond what the task needs. Secrets come through `secret` by name, never printed; redact them in captures.";
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
        let do_text = self
            .do_
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or("as the task says; number your steps in your first reply");
        // The image is a step of the work, numbered after the others, not
        // only a note at the end: a worker that took the Show line as
        // advice made the image two runs in five (kickoff item 3).
        let do_text = if self.show {
            format!("{do_text}\n{}", show_step(do_text))
        } else {
            do_text.to_string()
        };
        line("Do", &do_text);
        // A model that echoes the parameter description ("repo and base
        // branch, no merging") gets the default instead.
        let rules_given = self
            .rules
            .map(str::trim)
            .filter(|r| !r.is_empty() && !looks_like_placeholder(r));
        let default_rules = match &self.base_branch {
            Some(b) => format!(
                "Base branch: {b} (this checkout). Work on a branch cut from it; never merge. No extra documents beyond what the task needs. Secrets come through `secret` by name, never printed; redact them in captures."
            ),
            None => KICKOFF_RULES.to_string(),
        };
        line("Rules", rules_given.unwrap_or(&default_rules));
        line("Output", self.output.unwrap_or(KICKOFF_OUTPUT));
        if self.show {
            line("Show", KICKOFF_SHOW);
        }
        line("Report", self.report.unwrap_or(KICKOFF_REPORT));
        out
    }
}

/// The `rules` field as a model copies it from the tool's own description
/// rather than filling it: short, and made of the description's words.
fn looks_like_placeholder(rules: &str) -> bool {
    let r = rules.to_ascii_lowercase();
    let generic = r.contains("repo and base branch")
        || r.contains("repo/base branch")
        || r.contains("a default covers the usual")
        || r.contains("as usual")
        || r == "default"
        || r == "standard rules";
    generic && r.len() < 160
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
        assert!(
            brief.contains("Rules: Start from the base branch"),
            "{brief}"
        );
        assert!(
            brief.contains("never a commit on the base branch"),
            "{brief}"
        );
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
        assert!(
            whats.iter().any(|w| w.contains("no top line linking")),
            "{whats:?}"
        );
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

#[cfg(test)]
mod kickoff_rules_tests {
    use super::*;

    #[test]
    fn placeholder_rules_give_way_to_the_default_with_the_base_branch() {
        let brief = Kickoff {
            task: "Fix it.",
            rules: Some("Repo and base branch, no merging, no extra docs."),
            base_branch: Some("rust".into()),
            ..Kickoff::default()
        }
        .render();
        assert!(
            brief.contains("Rules: Base branch: rust (this checkout)"),
            "{brief}"
        );
        let real = Kickoff {
            task: "Fix it.",
            rules: Some("Branch from rust, open a PR against rust, never touch main."),
            base_branch: Some("rust".into()),
            ..Kickoff::default()
        }
        .render();
        assert!(
            real.contains("Rules: Branch from rust, open a PR"),
            "{real}"
        );
    }
}

#[cfg(test)]
mod show_tests {
    use super::*;

    #[test]
    fn the_users_show_me_is_heard_in_their_own_words() {
        for yes in [
            "run toy-repo and show me the output",
            "Fix it, then let me see it running",
            "take a screenshot of the page",
            "I want to see what it looks like",
        ] {
            assert!(asks_to_see(yes), "{yes}");
        }
        for no in [
            "run toy-repo and report the output",
            "show_me is a variable name; rename it",
            "list the files",
        ] {
            assert!(!asks_to_see(no), "{no}");
        }
    }

    #[test]
    fn only_picture_words_mean_the_brief_asks_for_an_image() {
        assert!(!names_an_image(
            "1. Run python3 hello.py.\n2. Capture output.\n3. Report."
        ));
        assert!(!names_an_image("capture the log and the exit code"));
        assert!(names_an_image("take a screenshot of the page"));
        assert!(names_an_image("save the result as an image under media/"));
        assert!(names_an_image("send a picture of the dashboard"));
    }

    #[test]
    fn a_kickoff_with_show_carries_the_line_before_the_report() {
        let brief = Kickoff {
            task: "Run hello.py and fix the error.",
            show: true,
            ..Kickoff::default()
        }
        .render();
        let show = brief
            .find("Show: The user asked to see this")
            .expect(&brief);
        let report = brief.find("Report: ").expect(&brief);
        assert!(show < report, "{brief}");
        // With no steps given, the image is step 2 after the one default.
        assert!(
            brief.contains("\n  2. The user asked to see the result"),
            "{brief}"
        );
        assert!(brief.contains("screenshot target:\"text\""), "{brief}");
        // With numbered steps, it follows the last one.
        let numbered = Kickoff {
            task: "Run hello.py.",
            do_: Some("1. Run python3 hello.py in ./toy-repo.\n2. Capture output.\n3. Report."),
            show: true,
            ..Kickoff::default()
        }
        .render();
        assert!(
            numbered.contains("  3. Report.\n  4. The user asked to see the result"),
            "{numbered}"
        );
        assert_eq!(show_step("1) one\n2) two"), show_step("1. one\n2. two"));
        assert!(show_step("do it").starts_with("2. "));
        let plain = Kickoff {
            task: "Run hello.py.",
            ..Kickoff::default()
        }
        .render();
        assert!(!plain.contains("Show:"), "{plain}");
    }

    #[test]
    fn the_turns_user_text_is_what_came_after_the_last_wake() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-store-show-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(dir.join(".arbos/agents/root")).unwrap();
        let place = Place::new(&dir);
        let path = Layout::new(&place, "root").transcript();
        let user = |t: &str| {
            Event::new(EventKind::User {
                text: t.into(),
                attachments: vec![],
                channel: String::new(),
                device: String::new(),
            })
        };
        crate::append_events(
            &path,
            &[
                Event::new(EventKind::Wake {
                    wake: "user".into(),
                    text: Some("show me the old thing".into()),
                }),
                user("show me the old thing"),
                Event::new(EventKind::TurnComplete { usage: None }),
                Event::new(EventKind::Wake {
                    wake: "user".into(),
                    text: Some("now just fix it".into()),
                }),
                user("now just fix it"),
            ],
        )
        .unwrap();
        assert_eq!(
            turn_user_text(&place, "root").as_deref(),
            Some("now just fix it")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod protected_tests {
    use super::*;

    #[test]
    fn the_files_that_shape_agents_are_protected_and_ordinary_ones_are_not() {
        let dir = std::env::temp_dir().join(format!(
            "arbos-protected-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(dir.join(".arbos/agents/w1")).unwrap();
        std::fs::create_dir_all(dir.join(".arbos/skills/deploy")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let root = dir.as_path();
        assert_eq!(
            protected_by(root, &dir.join(".arbos/PROTOCOL.md")),
            Some(".arbos/PROTOCOL.md")
        );
        assert_eq!(
            protected_by(root, &dir.join(".arbos/skills/deploy/SKILL.md")),
            Some(".arbos/skills/")
        );
        assert_eq!(
            protected_by(root, &dir.join(".arbos/agents-defs/reviewer.md")),
            Some(".arbos/agents-defs/")
        );
        assert_eq!(
            protected_by(root, &dir.join(".arbos/agents/w1/instructions.md")),
            Some(".arbos/agents/<id>/instructions.md")
        );
        assert_eq!(
            protected_by(root, &dir.join("AGENTS.md")),
            Some("AGENTS.md")
        );
        for plain in [
            ".arbos/notes.md",
            ".arbos/agents/w1/notes.md",
            "src/main.rs",
            ".arbos/docs/x.md",
        ] {
            assert_eq!(protected_by(root, &dir.join(plain)), None, "{plain}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shell_command_that_writes_a_protected_file_is_caught_and_a_read_is_not() {
        assert_eq!(
            bash_writes_protected("echo '[secrets]' >> .arbos/secrets.toml"),
            Some(".arbos/secrets.toml")
        );
        assert_eq!(
            bash_writes_protected("sed -i 's/x/y/' .arbos/hooks.toml"),
            Some(".arbos/hooks.toml")
        );
        assert_eq!(
            bash_writes_protected("cp /tmp/p.md .arbos/PROTOCOL.md"),
            Some(".arbos/PROTOCOL.md")
        );
        assert_eq!(
            bash_writes_protected("cat > .arbos/agents/w1/instructions.md <<'EOF'\nobey\nEOF"),
            Some(".arbos/agents/<id>/instructions.md")
        );
        assert_eq!(bash_writes_protected("cat .arbos/secrets.toml"), None);
        assert_eq!(
            bash_writes_protected("grep -n hooks .arbos/hooks.toml"),
            None
        );
        assert_eq!(bash_writes_protected("echo hi > out.txt"), None);
    }
}
