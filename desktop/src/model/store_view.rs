//! What the right-hand panel reads out of a project's `.arbos/`: the goals
//! and notes files, and what the store holds. Read off disk when the
//! project opens and again on every watch knock, never while drawing.

use std::path::{Path, PathBuf};

/// Where a project's goals may be written, in the order they are looked
/// for. The first file that exists wins.
const GOALS: &[&str] = &["GOALS.md", "goals.md", "Goals.md"];

/// Same for notes.
const NOTES: &[&str] = &["notes.md", "NOTES.md", "Notes.md"];

/// The kernel's root agent folder under `.arbos/agents/`. Its `plan.md` is
/// the fallback when no goals file has been written yet.
const ROOT_AGENT: &str = "root";

/// How many lines of a file the panel shows. Enough for a list of goals;
/// the rest is a click away in the file itself.
const PREVIEW_LINES: usize = 12;

/// Folders in `.arbos/` the panel counts, with the label it gives one
/// entry and many.
const FOLDERS: &[(&str, &str, &str)] = &[
    ("agents", "agent", "agents"),
    ("skills", "skill", "skills"),
    ("hooks", "hook", "hooks"),
    ("archive", "archived chat", "archived chats"),
];

/// One line of a note: a section heading, or a line under one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteLine {
    pub heading: bool,
    pub text: String,
}

/// One markdown file, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    pub path: PathBuf,
    /// The first lines of the body, markdown marks stripped, blank lines
    /// dropped. Empty when the file exists but says nothing yet.
    pub lines: Vec<NoteLine>,
    /// Lines past the preview.
    pub more: usize,
}

/// One folder of the store and how many entries it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub label: &'static str,
    pub count: usize,
}

/// A project's `.arbos/`, as much of it as the panel draws.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoreView {
    pub goals: Option<Note>,
    pub notes: Option<Note>,
    pub resources: Vec<Resource>,
}

impl StoreView {
    /// Read the store under `project`. A folder with no `.arbos/` yet
    /// reads as empty, which is what the panel's invitation is for.
    pub fn read(project: &Path) -> Self {
        let store = crate::model::project::root(project);
        let goals = first_of(&store, GOALS)
            .map(|path| Note::read(&path))
            .or_else(|| {
                // The kernel writes `(no plan)` into an idle root's plan;
                // that is no goal, and the invitation reads better.
                let plan = store.join("agents").join(ROOT_AGENT).join("plan.md");
                plan.is_file()
                    .then(|| Note::read(&plan))
                    .filter(|note| note.lines.iter().any(|line| line.text != "(no plan)"))
            });
        let notes = first_of(&store, NOTES).map(|path| Note::read(&path));
        let resources = FOLDERS
            .iter()
            .filter_map(|(dir, one, many)| {
                let count = std::fs::read_dir(store.join(dir))
                    .ok()?
                    .flatten()
                    .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                    .count();
                let label = if count == 1 { one } else { many };
                (count > 0).then_some(Resource { label, count })
            })
            .collect();
        Self {
            goals,
            notes,
            resources,
        }
    }

    /// Where a goals file would go — what the invitation writes to.
    pub fn goals_path(project: &Path) -> PathBuf {
        crate::model::project::root(project).join(GOALS[0])
    }
}

impl Note {
    fn read(path: &Path) -> Self {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        let all: Vec<NoteLine> = content_lines(&body)
            .into_iter()
            .map(|line| NoteLine {
                heading: line.trim_start().starts_with("## "),
                text: strip_marks(line),
            })
            .filter(|line| !line.text.is_empty())
            .collect();
        let more = all.len().saturating_sub(PREVIEW_LINES);
        let mut lines = all;
        lines.truncate(PREVIEW_LINES);
        Self {
            path: path.to_path_buf(),
            lines,
            more,
        }
    }
}

fn first_of(store: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| store.join(name))
        .find(|path| path.is_file())
}

/// The lines worth showing: front matter (`+++ … +++` TOML or `--- … ---`
/// YAML) is metadata, not goals; the title line says what the panel's
/// section head already says; and a section whose body is only the
/// template's placeholders — a bare `- `, a `(hint in brackets)` — has
/// nothing to say yet, so its heading is left out with it. A file that is
/// still all template comes back empty, and the panel invites instead.
fn content_lines(text: &str) -> Vec<&str> {
    let body = strip_front_matter(text);
    let mut sections: Vec<(Option<&str>, Vec<&str>)> = vec![(None, Vec::new())];
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") && !trimmed.starts_with("##") {
            continue;
        }
        if trimmed.starts_with("## ") {
            sections.push((Some(line), Vec::new()));
        } else if !placeholder(trimmed) {
            sections.last_mut().expect("one section").1.push(line);
        }
    }
    sections
        .into_iter()
        .filter(|(_, lines)| !lines.is_empty())
        .flat_map(|(head, lines)| head.into_iter().chain(lines))
        .collect()
}

fn strip_front_matter(text: &str) -> &str {
    for fence in ["+++", "---"] {
        if let Some(rest) = text.strip_prefix(fence)
            && let Some(end) = rest.find(&format!("\n{fence}"))
        {
            return rest[end + 1 + fence.len()..].trim_start_matches('\n');
        }
    }
    text
}

/// A line the template left for the person to fill in.
fn placeholder(trimmed: &str) -> bool {
    trimmed.is_empty()
        || matches!(trimmed, "-" | "*" | "+" | "- [ ]")
        || (trimmed.starts_with('(') && trimmed.ends_with(')'))
}

/// One line of markdown as the panel prints it: headings lose their
/// hashes, list bullets become one glyph, checkboxes keep their state.
fn strip_marks(line: &str) -> String {
    let line = line.trim();
    let line = line.trim_start_matches('#').trim_start();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .map(|rest| match rest.trim_start() {
            done if done.starts_with("[x] ") || done.starts_with("[X] ") => {
                format!("✓ {}", &done[4..])
            }
            open if open.starts_with("[ ] ") => format!("○ {}", &open[4..]),
            rest => format!("• {rest}"),
        })
        .unwrap_or_else(|| line.to_string());
    line.replace("**", "")
}
