//! What the right-hand panel reads out of a project's `.arbos/`: the
//! project page (`notes.md`, the status page the main chat keeps), where
//! the context document lives, and what the store holds. Read off disk
//! when the project opens and again on every watch knock, never while
//! drawing.
//!
//! The page follows the coordinator protocol's shape: an optional
//! `<tldr>` of a few bullets, `##` sections by topic, and checkbox items
//! of the form `- [ ] [label](target) — readout`. The panel renders it
//! the way Cursor's Projects page does: the label is the link, the
//! readout sits dim under it.

use std::path::{Path, PathBuf};

/// The status page, in the order it is looked for.
const NOTES: &[&str] = &["notes.md", "NOTES.md", "Notes.md"];

/// The context document: `docs/project-context.md`, with the older
/// `GOALS.md` names as the fallback for a store that predates it.
const CONTEXT: &[&str] = &[
    "docs/project-context.md",
    "GOALS.md",
    "goals.md",
    "Goals.md",
];

/// Folders in `.arbos/` the panel counts, with the label it gives one
/// entry and many.
const FOLDERS: &[(&str, &str, &str)] = &[
    ("agents", "agent", "agents"),
    ("skills", "skill", "skills"),
    ("hooks", "hook", "hooks"),
    ("archive", "archived chat", "archived chats"),
];

/// Where an item's link points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `http(s)://…`, `arbos://…`, `mailto:` — the browser's, or the
    /// app's own chat link.
    Url(String),
    /// `agents/<id>` (or `.arbos/agents/<id>`): a worker of this project,
    /// by its kernel id. A click puts its chat in the column.
    Worker(String),
    /// Anything else: a file, resolved against `.arbos/` when relative.
    File(PathBuf),
}

/// One item of the page: a checkbox row, or a `<tldr>` bullet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageItem {
    /// `Some(done)` for a checkbox; `None` for a tldr bullet.
    pub done: Option<bool>,
    /// Nesting, from the item's indent (two spaces or a tab per level).
    pub depth: u8,
    /// The link's label, or the whole text when the item has no link.
    pub label: String,
    pub target: Option<Target>,
    /// What follows the link, after the dash.
    pub readout: String,
}

/// The page, top to bottom: headings and items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageBlock {
    /// `##` is level 2, `###` level 3.
    Heading {
        level: u8,
        text: String,
    },
    Item(PageItem),
}

/// `notes.md`, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectPage {
    pub path: PathBuf,
    pub tldr: Vec<PageItem>,
    pub blocks: Vec<PageBlock>,
}

impl ProjectPage {
    /// Nothing but the template: no tldr, no items.
    pub fn is_empty(&self) -> bool {
        self.tldr.is_empty()
            && !self
                .blocks
                .iter()
                .any(|block| matches!(block, PageBlock::Item(_)))
    }
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
    /// The context document, when one has been written.
    pub context: Option<PathBuf>,
    /// The status page, when one exists (even if still the template).
    pub page: Option<ProjectPage>,
    pub resources: Vec<Resource>,
}

impl StoreView {
    /// Read the store under `project`. A folder with no `.arbos/` yet
    /// reads as empty, which is what the panel's invitation is for.
    pub fn read(project: &Path) -> Self {
        let store = crate::model::project::root(project);
        let context = first_of(&store, CONTEXT);
        let page = first_of(&store, NOTES).map(|path| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            ProjectPage::parse(&path, &store, &text)
        });
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
            context,
            page,
            resources,
        }
    }
}

impl ProjectPage {
    /// Parse a status page. `store` is the `.arbos/` folder relative
    /// links resolve against.
    pub fn parse(path: &Path, store: &Path, text: &str) -> Self {
        let body = strip_front_matter(text);
        let mut page = Self {
            path: path.to_path_buf(),
            ..Self::default()
        };
        let mut in_tldr = false;
        let mut in_fence = false;
        let mut seen_heading = false;
        for raw in body.lines() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence || trimmed.is_empty() {
                continue;
            }
            match trimmed {
                "<tldr>" => {
                    in_tldr = true;
                    continue;
                }
                "</tldr>" => {
                    in_tldr = false;
                    continue;
                }
                _ => {}
            }
            if in_tldr {
                if let Some(rest) = bullet(trimmed) {
                    page.tldr.push(item(None, 0, rest, store));
                }
                continue;
            }
            if trimmed.starts_with("# ") {
                continue;
            }
            if let Some(text) = trimmed.strip_prefix("### ") {
                seen_heading = true;
                page.blocks.push(PageBlock::Heading {
                    level: 3,
                    text: text.trim().to_owned(),
                });
                continue;
            }
            if let Some(text) = trimmed.strip_prefix("## ") {
                seen_heading = true;
                page.blocks.push(PageBlock::Heading {
                    level: 2,
                    text: text.trim().to_owned(),
                });
                continue;
            }
            let Some(rest) = bullet(trimmed) else {
                // Prose above the first heading is the template's link line
                // to the context document, which the Context row already
                // carries. Prose elsewhere is not the page's shape; skipped.
                continue;
            };
            let depth = indent_depth(line);
            let (done, rest) = checkbox(rest);
            // A plain bullet above the first heading is prose; under a
            // heading it is an item that lost its checkbox.
            if done.is_none() && !seen_heading {
                continue;
            }
            page.blocks.push(PageBlock::Item(item(
                Some(done.unwrap_or(false)),
                depth,
                rest,
                store,
            )));
        }
        page
    }
}

fn first_of(store: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| store.join(name))
        .find(|path| path.is_file())
}

fn bullet(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
}

/// `[ ] rest` → `(Some(false), rest)`; `[x] rest` → `(Some(true), rest)`;
/// no box → `(None, rest)`.
fn checkbox(rest: &str) -> (Option<bool>, &str) {
    let r = rest.trim_start();
    if let Some(open) = r.strip_prefix("[ ]") {
        return (Some(false), open.trim_start());
    }
    if let Some(done) = r.strip_prefix("[x]").or_else(|| r.strip_prefix("[X]")) {
        return (Some(true), done.trim_start());
    }
    (None, r)
}

fn indent_depth(line: &str) -> u8 {
    let mut cols = 0usize;
    for c in line.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 2,
            _ => break,
        }
    }
    (cols / 2).min(4) as u8
}

/// `[label](target) — readout` into its parts. Without a leading link the
/// whole text is the label and there is no target.
fn item(done: Option<bool>, depth: u8, text: &str, store: &Path) -> PageItem {
    let text = text.trim();
    let mut label = text.to_owned();
    let mut target = None;
    let mut readout = String::new();
    if let Some((l, t, rest)) = leading_link(text) {
        label = l.to_owned();
        target = Some(classify(t, store));
        readout = rest.to_owned();
    }
    PageItem {
        done,
        depth,
        label: unbold(&label),
        target,
        readout: unbold(&readout),
    }
}

/// `[label](target) rest` at the start of `s`.
fn leading_link(s: &str) -> Option<(&str, &str, &str)> {
    let rest = s.strip_prefix('[')?;
    let close = rest.find("](")?;
    let label = &rest[..close];
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    let target = after[..end].split_whitespace().next().unwrap_or("");
    let tail = after[end + 1..].trim_start();
    let tail = tail
        .strip_prefix("—")
        .or_else(|| tail.strip_prefix("--"))
        .or_else(|| tail.strip_prefix('-'))
        .or_else(|| tail.strip_prefix(':'))
        .map(str::trim_start)
        .unwrap_or(tail);
    (!label.trim().is_empty()).then_some((label.trim(), target, tail))
}

fn classify(target: &str, store: &Path) -> Target {
    let t = target.trim();
    if t.contains("://") || t.starts_with("mailto:") {
        return Target::Url(t.to_owned());
    }
    let rel = t.strip_prefix("./").unwrap_or(t);
    let rel = rel.strip_prefix(".arbos/").unwrap_or(rel);
    if let Some(id) = rel.strip_prefix("agents/") {
        let id = id.trim_end_matches('/');
        let id = id.split('/').next().unwrap_or(id);
        if !id.is_empty() {
            return Target::Worker(id.to_owned());
        }
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Target::File(path.to_owned());
    }
    Target::File(store.join(path))
}

fn unbold(s: &str) -> String {
    s.replace("**", "").replace('`', "")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_protocol_page_parses_into_tldr_sections_and_items() {
        let store = Path::new("/p/.arbos");
        let text = "+++\nowner = \"root\"\n+++\n# Demo\n\nGoals: [project-context](docs/project-context.md)\n\n<tldr>\n- [Voice PR](https://x/1) — live on the pod\n</tldr>\n\n## Voice\n- [ ] [Voice PR](https://x/1) — live on the pod; DNS next\n  - [ ] [Fix echo](agents/fix-echo) — echo gate landing\n- [x] [Research](docs/r.md) — delivered\n### Sub\n- [ ] **no link here**\n";
        let page = ProjectPage::parse(Path::new("/p/.arbos/notes.md"), store, text);
        assert_eq!(page.tldr.len(), 1);
        assert_eq!(page.tldr[0].label, "Voice PR");
        assert_eq!(page.tldr[0].target, Some(Target::Url("https://x/1".into())));
        assert_eq!(page.tldr[0].readout, "live on the pod");
        assert_eq!(
            page.blocks[0],
            PageBlock::Heading {
                level: 2,
                text: "Voice".into()
            }
        );
        let PageBlock::Item(first) = &page.blocks[1] else {
            panic!("item")
        };
        assert_eq!(first.done, Some(false));
        assert_eq!(first.depth, 0);
        assert_eq!(first.readout, "live on the pod; DNS next");
        let PageBlock::Item(child) = &page.blocks[2] else {
            panic!("item")
        };
        assert_eq!(child.depth, 1);
        assert_eq!(child.target, Some(Target::Worker("fix-echo".into())));
        let PageBlock::Item(done) = &page.blocks[3] else {
            panic!("item")
        };
        assert_eq!(done.done, Some(true));
        assert_eq!(
            done.target,
            Some(Target::File(PathBuf::from("/p/.arbos/docs/r.md")))
        );
        let PageBlock::Item(plain) = &page.blocks[5] else {
            panic!("item")
        };
        assert_eq!(plain.label, "no link here");
        assert_eq!(plain.target, None);
        assert!(!page.is_empty());
    }

    #[test]
    fn the_template_reads_as_empty() {
        let text = "+++\nowner = \"root\"\n+++\n# Notes\n\nGoals, constraints, decisions: [project-context](docs/project-context.md)\n";
        let page = ProjectPage::parse(
            Path::new("/p/.arbos/notes.md"),
            Path::new("/p/.arbos"),
            text,
        );
        assert!(page.is_empty(), "{page:?}");
    }

    #[test]
    fn targets_classify() {
        let store = Path::new("/p/.arbos");
        assert_eq!(
            classify(".arbos/agents/w1/", store),
            Target::Worker("w1".into())
        );
        assert_eq!(
            classify("arbos://chat/3?p=x", store),
            Target::Url("arbos://chat/3?p=x".into())
        );
        assert_eq!(
            classify("media/layout/a.png", store),
            Target::File(PathBuf::from("/p/.arbos/media/layout/a.png"))
        );
        assert_eq!(
            classify("/abs/file.md", store),
            Target::File(PathBuf::from("/abs/file.md"))
        );
    }
}
