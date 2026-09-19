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

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

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

/// Folders in `.arbos/` the panel counts under Resources, with the label
/// it gives one entry and many. Agents and the archive are not here: the
/// Agents section above already shows every worker and its "N archived"
/// row, and a second count of the same things read as something else.
const FOLDERS: &[(&str, &str, &str)] = &[("skills", "skill", "skills"), ("hooks", "hook", "hooks")];

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
    /// A line of prose under a heading, not a list entry: drawn without a
    /// glyph, flush with the heading.
    pub prose: bool,
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
    /// A pipe table: its header cells and its rows, the `| --- |` line
    /// read and dropped. Cursor draws a table as a table; the pipes drawn
    /// as prose were F-214.
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

/// `notes.md`, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectPage {
    pub path: PathBuf,
    pub tldr: Vec<PageItem>,
    pub blocks: Vec<PageBlock>,
}

impl ProjectPage {
    /// Nothing but the template — the title and its link line. A heading
    /// or a line of prose the agent wrote is a page, even before the first
    /// checklist item (F-61: "Nothing on the project page yet" beside a
    /// turn that had just written it).
    pub fn is_empty(&self) -> bool {
        self.tldr.is_empty() && self.blocks.is_empty()
    }
}

/// One folder of the store and how many entries it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub label: &'static str,
    pub count: usize,
}

/// A standing subscription one of the agents holds
/// (`agents/<id>/subscriptions/NNNN-slug.toml`): the only scheduler in
/// the Cursor-model kernel. Read off the files, so the panel shows them
/// whether or not that agent's chat is attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The kernel id of the agent that holds it.
    pub agent: String,
    pub id: u32,
    /// `timer` | `shell` | `github_pr` | `github_ci` | `inbox`.
    pub kind: String,
    /// The prompt, command, or pull request, clipped.
    pub label: String,
    /// `every 1h · next 15:04`, `once · next 15:04`, `at 09:00`, or empty.
    pub when: String,
    pub paused: bool,
}

/// What a store file is, for its glyph and its viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Image,
    Other,
}

/// One file of the store the page lists: a document under `docs/`, a
/// picture or clip under `media/`, the archive. Cursor's Projects page
/// lists the shared context this way, each one a click from view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFile {
    pub path: PathBuf,
    /// The file's name, without the folder.
    pub name: String,
    /// Where it sits under `.arbos/`: `docs`, `media/shots`, or empty.
    pub folder: String,
    pub kind: FileKind,
    pub modified: Option<SystemTime>,
    /// `docs/project-context.md`: first in every list, whatever its age.
    pub pinned: bool,
}

/// How many files the store view keeps. The page shows them all; the
/// panel shows the first few and says how many more.
const FILES_CAP: usize = 80;

/// Folders whose files the page lists, and how deep it looks in each.
const FILE_FOLDERS: &[(&str, usize)] = &[("docs", 2), ("media", 2)];

/// Loose files of the store the page lists beside those folders.
const LOOSE_FILES: &[&str] = &["archived.md"];

/// A project's `.arbos/`, as much of it as the panel draws.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoreView {
    /// The context document, when one has been written.
    pub context: Option<PathBuf>,
    /// Its text, for the page's Context section. Small by the protocol's
    /// own rule; a file over the cap is cut with a note.
    pub context_text: Option<String>,
    /// `docs/`, `media/` and the loose files, the context document first,
    /// then newest first.
    pub files: Vec<StoreFile>,
    /// The status page, when one exists (even if still the template).
    pub page: Option<ProjectPage>,
    /// Every agent's subscriptions, root's first, then by agent and id.
    pub standing: Vec<Standing>,
    /// Whether any agent has a `subscriptions/` folder at all: the
    /// Cursor-model kernel. Without one, the panel falls back to what
    /// attached chats report in their plan.
    pub standing_known: bool,
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
        let (standing, standing_known) = read_standing(&store);
        let context_text = context.as_deref().and_then(read_context);
        let files = read_files(&store, context.as_deref());
        Self {
            context,
            context_text,
            files,
            page,
            standing,
            standing_known,
            resources,
        }
    }
}

/// The context document as the page renders it: front matter and the
/// title line gone (the page's own heading says "Context"), cut at the
/// size the kernel's prompt cap uses so a runaway file cannot stall a
/// frame. `None` while the file is still the untouched template — every
/// line a heading, a bare bullet, or a `(hint)`.
fn read_context(path: &Path) -> Option<String> {
    const CAP: usize = 16_000;
    let text = std::fs::read_to_string(path).ok()?;
    let body = strip_front_matter(&text);
    let body = body
        .trim_start()
        .strip_prefix('#')
        .filter(|rest| !rest.starts_with('#'))
        .and_then(|rest| rest.split_once('\n'))
        .map(|(_, rest)| rest)
        .unwrap_or(body)
        .trim_start_matches('\n');
    let body = prune_template(body)?;
    if body.chars().count() <= CAP {
        return Some(body);
    }
    let head: String = body.chars().take(CAP).collect();
    Some(format!("{head}\n\n*… cut at {CAP} characters.*"))
}

/// A line the template wrote, not the author: blank, a heading, an empty
/// bullet, or a parenthesised hint like "(what this project is for)".
fn template_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.starts_with('#')
        || matches!(line, "-" | "*" | "+" | "- [ ]")
        || (line.starts_with('(') && line.ends_with(')'))
}

/// The document without its unfilled template sections: a `##` section
/// whose body is only hints and empty bullets is dropped, and a page whose
/// sections are all like that is no page at all — the preamble alone
/// ("Stable goals, constraints, …") is the template talking.
fn prune_template(body: &str) -> Option<String> {
    let mut sections: Vec<(Option<&str>, Vec<&str>)> = vec![(None, Vec::new())];
    for line in body.lines() {
        if line.starts_with("## ") {
            sections.push((Some(line), Vec::new()));
        } else {
            sections.last_mut().expect("preamble").1.push(line);
        }
    }
    let kept: Vec<String> = sections
        .iter()
        .filter(|(heading, lines)| heading.is_some() && lines.iter().any(|l| !template_line(l)))
        .map(|(heading, lines)| {
            let mut out = heading.unwrap_or_default().to_string();
            out.push('\n');
            out.push_str(lines.join("\n").trim_matches('\n'));
            out
        })
        .collect();
    if kept.is_empty() {
        // No sections at all: a plain document is its own content.
        let plain = sections
            .first()
            .map(|(_, lines)| lines)
            .filter(|_| sections.len() == 1)?;
        if plain.iter().any(|l| !template_line(l)) {
            return Some(plain.join("\n").trim_matches('\n').to_string());
        }
        return None;
    }
    Some(kept.join("\n\n"))
}

/// The files the page lists: `docs/` and `media/` (two levels), the loose
/// files, the context document pinned first, the rest newest first.
fn read_files(store: &Path, context: Option<&Path>) -> Vec<StoreFile> {
    let mut files = Vec::new();
    for (folder, depth) in FILE_FOLDERS {
        walk(&store.join(folder), folder, *depth, &mut files);
    }
    for name in LOOSE_FILES {
        let path = store.join(name);
        if path.is_file() {
            files.push(store_file(path, String::new()));
        }
    }
    for file in &mut files {
        file.pinned = context.is_some_and(|context| context == file.path);
    }
    files.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.name.cmp(&b.name))
    });
    files.truncate(FILES_CAP);
    files
}

fn walk(dir: &Path, folder: &str, depth: usize, out: &mut Vec<StoreFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if depth > 1 {
                walk(&path, &format!("{folder}/{name}"), depth - 1, out);
            }
        } else if path.is_file() {
            out.push(store_file(path, folder.to_string()));
        }
    }
}

fn store_file(path: PathBuf, folder: String) -> StoreFile {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "md" | "markdown" | "txt" => FileKind::Markdown,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" => FileKind::Image,
        _ => FileKind::Other,
    };
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    StoreFile {
        path,
        name,
        folder,
        kind,
        modified,
        pinned: false,
    }
}

/// Every `agents/*/subscriptions/*.toml`, parsed leniently: a file the
/// kernel is mid-write on, or a newer kernel's field, costs one row, not
/// the panel.
fn read_standing(store: &Path) -> (Vec<Standing>, bool) {
    let mut agents: Vec<(String, PathBuf)> = std::fs::read_dir(store.join("agents"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    agents.sort_by(|a, b| {
        (a.0 != "root")
            .cmp(&(b.0 != "root"))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut out = Vec::new();
    let mut known = false;
    for (agent, dir) in agents {
        let Ok(entries) = std::fs::read_dir(dir.join("subscriptions")) else {
            continue;
        };
        known = true;
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(standing) = parse_standing(&agent, &text) {
                out.push(standing);
            }
        }
    }
    (out, known)
}

fn parse_standing(agent: &str, text: &str) -> Option<Standing> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let table = value.as_table()?;
    let str_of = |key: &str| table.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    // A kernel chore (`deliver_to = "none"`: the weekly `git gc`) is not
    // the project's standing work; nobody asked for it.
    if str_of("deliver_to").as_deref() == Some("none") {
        return None;
    }
    let id = table.get("id").and_then(|v| v.as_integer()).unwrap_or(0) as u32;
    let kind = str_of("kind").unwrap_or_default();
    let label = str_of("prompt")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| str_of("cmd"))
        .or_else(|| {
            let repo = str_of("repo")?;
            let pr = table.get("pr").and_then(|v| v.as_integer())?;
            Some(format!("{repo}#{pr}"))
        })
        .or_else(|| str_of("path"))
        .unwrap_or_else(|| kind.clone());
    let label = clip(&label, 60);
    let next = str_of("next_due").and_then(|s| clock_of(&s));
    let mut when = match (
        str_of("every"),
        str_of("at"),
        table.get("once").and_then(|v| v.as_bool()),
    ) {
        (Some(every), _, _) => format!("every {every}"),
        (None, Some(at), _) => format!("at {at}"),
        (None, None, Some(true)) => "once".to_owned(),
        _ => String::new(),
    };
    if let Some(next) = next {
        if !when.is_empty() {
            when.push_str(" · ");
        }
        when.push_str("next ");
        when.push_str(&next);
    }
    Some(Standing {
        agent: agent.to_owned(),
        id,
        kind,
        label,
        when,
        paused: table
            .get("paused")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// `HH:MM` out of an RFC 3339 instant; the date is the kernel's business.
fn clock_of(rfc3339: &str) -> Option<String> {
    let (_, time) = rfc3339.split_once('T')?;
    let hhmm: String = time.chars().take(5).collect();
    (hhmm.len() == 5).then_some(hhmm)
}

fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
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
            // A pipe table's row. Consecutive rows join the same table; the
            // separator row (`| --- | --- |`) is read and dropped.
            if let Some(cells) = table_cells(trimmed) {
                if cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
                    continue;
                }
                match page.blocks.last_mut() {
                    Some(PageBlock::Table { rows, .. }) => rows.push(cells),
                    _ => page.blocks.push(PageBlock::Table {
                        header: cells,
                        rows: Vec::new(),
                    }),
                }
                continue;
            }
            let Some(rest) = bullet(trimmed) else {
                // Prose above the first heading is the template's link line
                // to the context document, which the Context row already
                // carries. Prose under a heading is the agent's — Cursor's
                // page renders the whole document — so it stays, as a line
                // without a checkbox.
                if seen_heading && !trimmed.is_empty() {
                    page.blocks.push(PageBlock::Item(PageItem {
                        prose: true,
                        ..item(None, 0, trimmed, store)
                    }));
                }
                continue;
            };
            let depth = indent_depth(line);
            let (done, rest) = checkbox(rest);
            // `- [ ] ## Goal`: a heading the agent pushed through the plan
            // tool as an item (F-71). It is a heading; a checkbox in front
            // of "## Goal" is not something Cursor's page would show.
            let heading = rest.trim_start();
            if let Some((hashes, text)) = heading.split_once(' ')
                && !hashes.is_empty()
                && hashes.chars().all(|c| c == '#')
                && !text.trim().is_empty()
            {
                page.blocks.push(PageBlock::Heading {
                    level: hashes.len().clamp(2, 3) as u8,
                    text: text.trim().to_string(),
                });
                seen_heading = true;
                continue;
            }
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

/// `| a | b |` → `["a", "b"]`; `None` for a line that is not a table row.
fn table_cells(trimmed: &str) -> Option<Vec<String>> {
    let inner = trimmed.strip_prefix('|')?;
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let cells: Vec<String> = inner.split('|').map(|c| unbold(c.trim())).collect();
    (cells.len() >= 2).then_some(cells)
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
    // A model sometimes hands the plan tool a whole checklist line as the
    // label, and the kernel wraps it as written: `[- [ ] [name](agents/x)]
    // (README.md)`. The row shows the name; the inner link is the better
    // target when the outer one is missing.
    let mut inner = label.trim();
    loop {
        let (_, after_box) = checkbox(bullet(inner).unwrap_or(inner));
        let after_box = after_box.trim();
        if after_box == inner {
            break;
        }
        inner = after_box;
    }
    if let Some((l, t, rest)) = leading_link(inner) {
        if target.is_none() {
            target = Some(classify(t, store));
        }
        if readout.is_empty() {
            readout = rest.to_owned();
        }
        inner = l;
    }
    // `[name](x)](y)` — a link closed twice — leaves `name](x)` as the
    // label; the name is what was meant.
    let label = match inner.find("](") {
        Some(at) if inner.ends_with(')') => inner[..at].trim().to_owned(),
        _ => inner.to_owned(),
    };
    PageItem {
        done,
        depth,
        label: unbold(&label),
        target,
        readout: unbold(&readout),
        prose: false,
    }
}

/// `[label](target) rest` at the start of `s`. The label may itself hold
/// brackets (a nested link): the close is the `](` that balances the first
/// `[`.
fn leading_link(s: &str) -> Option<(&str, &str, &str)> {
    let rest = s.strip_prefix('[')?;
    let close = balanced_close(rest)?;
    let label = &rest[..close];
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    let target = after[..end].split_whitespace().next().unwrap_or("");
    let mut tail = after[end + 1..].trim_start();
    // A link closed twice — `](x)](y) rest` — leaves `](y)` in front of
    // the readout; it is not part of it.
    if let Some(stray) = tail.strip_prefix("](")
        && let Some(close) = stray.find(')')
    {
        tail = stray[close + 1..].trim_start();
    }
    let tail = tail
        .strip_prefix("—")
        .or_else(|| tail.strip_prefix("--"))
        .or_else(|| tail.strip_prefix('-'))
        .or_else(|| tail.strip_prefix(':'))
        .map(str::trim_start)
        .unwrap_or(tail);
    (!label.trim().is_empty()).then_some((label.trim(), target, tail))
}

/// Byte offset in `rest` (the text after an opening `[`) of the `](` that
/// closes it, counting nested brackets.
fn balanced_close(rest: &str) -> Option<usize> {
    let mut depth = 0usize;
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' if depth == 0 => {
                if bytes.get(i + 1) == Some(&b'(') {
                    return Some(i);
                }
            }
            b']' => depth -= 1,
            _ => {}
        }
    }
    None
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
    fn a_checklist_line_wrapped_as_a_label_shows_its_name() {
        let store = Path::new("/p/.arbos");
        let text = "## Tasks\n- [x] [- [ ] [rewrite-readme](.arbos/agents/rewrite-readme)](README.md) — worker reported README.md recreated\n- [ ] [- [ ] plain words](docs/x.md) — pending\n";
        let page = ProjectPage::parse(Path::new("/p/.arbos/notes.md"), store, text);
        let PageBlock::Item(first) = &page.blocks[1] else {
            panic!("item")
        };
        assert_eq!(first.done, Some(true));
        assert_eq!(first.label, "rewrite-readme");
        assert_eq!(first.readout, "worker reported README.md recreated");
        assert_eq!(
            first.target,
            Some(Target::File(PathBuf::from("/p/.arbos/README.md")))
        );
        let PageBlock::Item(second) = &page.blocks[2] else {
            panic!("item")
        };
        assert_eq!(second.label, "plain words");
        assert_eq!(second.readout, "pending");
        let text = "## Math\n- [x] [add mul(a, b)](math_utils.py)](math_utils.py) — added mul\n";
        let page = ProjectPage::parse(Path::new("/p/.arbos/notes.md"), store, text);
        let PageBlock::Item(item) = &page.blocks[1] else {
            panic!("item")
        };
        assert_eq!(item.label, "add mul(a, b)");
        assert_eq!(item.readout, "added mul");
    }

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
    fn a_subscription_file_becomes_a_standing_row() {
        let text = "id = 3\nkind = \"shell\"\ncmd = \"curl -s https://x/btc\"\nevery = \"10m\"\ndeliver_to = \"user\"\nnotify = \"BTC: {output}\"\ncreated = \"2026-09-13T16:00:00Z\"\nnext_due = \"2026-09-13T16:10:00Z\"\n";
        let s = parse_standing("root", text).unwrap();
        assert_eq!(s.id, 3);
        assert_eq!(s.kind, "shell");
        assert_eq!(s.label, "curl -s https://x/btc");
        assert_eq!(s.when, "every 10m · next 16:10");
        assert!(!s.paused);
        let pr = parse_standing(
            "w1",
            "id = 1\nkind = \"github_pr\"\nrepo = \"unarbos/arbos\"\npr = 103\ncreated = \"x\"\n",
        )
        .unwrap();
        assert_eq!(pr.label, "unarbos/arbos#103");
        assert_eq!(pr.when, "");
        assert!(parse_standing("w1", "not toml = = =").is_none());
        assert!(
            parse_standing(
                "root",
                "id = 1\nkind = \"shell\"\ncmd = \"git gc\"\nevery = \"7d\"\ndeliver_to = \"none\"\ncreated = \"x\"\n"
            )
            .is_none(),
            "a kernel chore is not standing work"
        );
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
