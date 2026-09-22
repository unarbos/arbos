//! `agents/<id>/notes.md`: the agent's own checklist, written by the `plan`
//! tool and read by the prompt and the desktop. Cursor's `notes.md` shape
//! (coordinator protocol, section 5):
//!
//! ```markdown
//! ## Topic
//! - [ ] [label](target) — status readout
//! - [x] [done thing](target) — what came of it
//! ```
//!
//! Items are checkboxes under `##` sections. The text after the box is
//! the item, rewritten fresh on every touch. Checked items sink to the end
//! of their section; the three newest stay, older ones are the archive's
//! (the protocol worker's `archived.md`). Anything that is not a checkbox
//! line (a `<tldr>`, prose, links) is kept as it is.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::Place;

/// Checked items kept per section before the rest are dropped from view.
pub const DONE_KEPT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// 1-based position among the checkbox items of the whole file: the
    /// handle the `plan` tool and the desktop use.
    pub n: usize,
    pub section: String,
    pub done: bool,
    pub text: String,
}

impl Item {
    /// `[label](target) — readout` → `label`; else the first words.
    pub fn label(&self) -> String {
        if let Some(rest) = self.text.strip_prefix('[')
            && let Some(end) = rest.find("](")
        {
            return rest[..end].to_string();
        }
        crate::text::clip(&self.text, 60)
    }

    /// The part after ` — `, if any.
    pub fn readout(&self) -> Option<&str> {
        self.text.split_once(" — ").map(|(_, r)| r.trim())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// The file's lines. Items are edited in place so prose survives.
    lines: Vec<String>,
    /// Checked items that left the page because their section already
    /// kept `DONE_KEPT`: `(section, item line)`, oldest first. The saver
    /// moves them to `archived.md` (the protocol: move, never delete).
    overflow: Vec<(String, String)>,
    /// Why this page is empty although its file may not be: the read
    /// failed (EIO, EACCES, a partial view on a mount). A page in this
    /// state is shown as empty and is **never written back**: one bad
    /// read of `.arbos/notes.md` and the next `plan` call rewrote the
    /// coordinator's 293-byte page as its one new item (qal-j09). See
    /// `crate::record` for the rule.
    unread: Option<String>,
}

/// `<tldr>` is kept by the tool only once the page is big: this many
/// `##` sections and this many items (Cursor's rule: several sub-projects
/// and six or more items). Above it, at most `TLDR_CAP` bullets, the most
/// recently touched workstream first.
pub const TLDR_SECTIONS: usize = 2;
pub const TLDR_ITEMS: usize = 6;
pub const TLDR_CAP: usize = 4;

/// Root's checklist is the project page, `.arbos/notes.md` (one per
/// project, like Cursor's Agent Store `notes.md`; decided 2026-09-13).
/// Every other agent keeps its own under its folder.
pub fn path(place: &Place, agent: &str) -> PathBuf {
    if agent == crate::ROOT_ID {
        project_page(place)
    } else {
        place.agent_dir(agent).join("notes.md")
    }
}

/// `.arbos/notes.md`: the user-visible status page. Root writes it; the
/// file tools refuse other agents (`tool::PlanCx::resolve_write`).
pub fn project_page(place: &Place) -> PathBuf {
    place.arbos().join("notes.md")
}

/// The page's head is not the model's to lose: its front matter (owner)
/// and the link to `docs/project-context.md` stay whatever a whole-page
/// `write` says. `new` keeps its own front matter when it has one; else
/// `old`'s (or the template's) goes on top. The context link, when the new
/// text lacks it, follows the first `#` title or opens the body. A page
/// written with a `# Title` of its own keeps that title.
pub fn keep_page_head(old: &str, new: &str) -> String {
    fn front_matter(text: &str) -> Option<&str> {
        for fence in ["+++", "---"] {
            if let Some(rest) = text.strip_prefix(fence)
                && let Some(end) = rest.find(&format!("\n{fence}"))
            {
                let len = fence.len() + end + 1 + fence.len();
                let mut head = &text[..len];
                if text[len..].starts_with('\n') {
                    head = &text[..len + 1];
                }
                return Some(head);
            }
        }
        None
    }
    const LINK: &str = "Goals, constraints, decisions: [project-context](docs/project-context.md)";
    if front_matter(new).is_some() && new.contains("docs/project-context.md") {
        return new.to_string();
    }
    let front = front_matter(new)
        .or_else(|| front_matter(old))
        .or_else(|| front_matter(crate::store::NOTES_TEMPLATE))
        .unwrap_or("");
    let body = &new[front_matter(new).map_or(0, str::len)..];
    let mut out = String::from(front);
    if !front.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if body.contains("docs/project-context.md") {
        out.push_str(body);
        return out;
    }
    let mut lines = body.lines();
    match lines.next() {
        Some(first) if first.starts_with("# ") => {
            out.push_str(first);
            out.push_str("\n\n");
            out.push_str(LINK);
            out.push('\n');
            let rest: Vec<&str> = lines.collect();
            let rest = rest.join("\n");
            if !rest.trim().is_empty() {
                if !rest.starts_with('\n') {
                    out.push('\n');
                }
                out.push_str(rest.trim_start_matches('\n'));
            }
        }
        _ => {
            out.push_str("# Notes\n\n");
            out.push_str(LINK);
            out.push_str("\n\n");
            out.push_str(body.trim_start_matches('\n'));
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Is `candidate` this place's project page?
pub fn is_project_page(place_root: &Path, candidate: &Path) -> bool {
    let page = place_root.join(".arbos").join("notes.md");
    let a = std::fs::canonicalize(&page).unwrap_or(page);
    let b = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    a == b
}

pub const PAGE_REFUSAL: &str = ".arbos/notes.md is the project page, written by root only; keep your own checklist with the plan tool and tell root with say to=root";

/// The page as it stands: parsed when read, empty when absent, and an
/// `unread` page — empty, unsaveable — when the read failed.
pub fn load(place: &Place, agent: &str) -> Notes {
    read_path(&path(place, agent))
}

pub fn save(place: &Place, agent: &str, notes: &Notes) -> Result<()> {
    save_path(&path(place, agent), notes)?;
    // The project page's finished items are moved, never dropped: each
    // one lands in `archived.md` under its section.
    if agent == crate::ROOT_ID && !notes.overflow.is_empty() {
        archive_overflow(&crate::store::archived_path(place), &notes.overflow)?;
    }
    Ok(())
}

/// Append `(section, line)` pairs to the archive, each under a `##` of its
/// section (made at the end when new), newest last within the section.
pub fn archive_overflow(archived: &Path, overflow: &[(String, String)]) -> Result<()> {
    let mut lines: Vec<String> = std::fs::read_to_string(archived)
        .unwrap_or_else(|_| crate::store::ARCHIVED_TEMPLATE.to_string())
        .lines()
        .map(str::to_string)
        .collect();
    for (section, line) in overflow {
        let heading = if section.trim().is_empty() {
            "## Archived".to_string()
        } else {
            format!("## {}", section.trim())
        };
        let at = match lines.iter().position(|l| l.trim() == heading) {
            Some(h) => {
                let mut end = h + 1;
                while end < lines.len() && !lines[end].starts_with("## ") {
                    end += 1;
                }
                while end > h + 1 && lines[end - 1].trim().is_empty() {
                    end -= 1;
                }
                end
            }
            None => {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(heading);
                lines.len()
            }
        };
        lines.insert(at, line.clone());
    }
    let mut text = lines.join("\n");
    text.push('\n');
    if let Some(dir) = archived.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = archived.with_extension(format!("md.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, archived).with_context(|| format!("replace {}", archived.display()))?;
    Ok(())
}

/// `agents/<id>/todo.md`: the agent's own working checklist for the thread
/// in hand (Cursor's TodoWrite), shown to the user as a card and never a
/// page. A coordinator's `plan` writes the project page, so this is where
/// its own steps go; a worker has both and may use either.
pub const TODO: &str = "todo.md";

pub fn todo_path(place: &Place, agent: &str) -> PathBuf {
    place.agent_dir(agent).join(TODO)
}

pub fn load_todo(place: &Place, agent: &str) -> Notes {
    read_path(&todo_path(place, agent))
}

pub fn save_todo(place: &Place, agent: &str, notes: &Notes) -> Result<()> {
    save_path(&todo_path(place, agent), notes)
}

fn save_path(p: &Path, notes: &Notes) -> Result<()> {
    // The rule (`crate::record`): a page whose read failed is a question,
    // not a value, and is not written over the file it stands in for.
    if let Some(why) = &notes.unread {
        anyhow::bail!(
            "{why} — {} not written; the page keeps its bytes. Read it again, or fix what blocks the read.",
            p.display()
        );
    }
    crate::record::write_atomic(p, notes.render().as_bytes())
}

pub fn read_path(path: &Path) -> Notes {
    match crate::record::read_text(path) {
        crate::record::Read::Present(text) => Notes::parse(&text),
        crate::record::Read::Absent => Notes::parse(""),
        crate::record::Read::Unknown(why) => Notes::unread(why),
    }
}

/// `[label](target) …` → `label`.
fn link_label(s: &str) -> Option<&str> {
    let rest = s.trim_start().strip_prefix('[')?;
    let close = rest.find("](")?;
    rest[close + 2..].find(')')?;
    let label = &rest[..close];
    (!label.trim().is_empty()).then_some(label)
}

/// `[label](target) …` → `target`.
/// The `(target)` of a leading `[label](target)` in an item's text.
pub fn link_target_of(s: &str) -> Option<&str> {
    link_target(s)
}

fn link_target(s: &str) -> Option<&str> {
    let rest = s.trim_start().strip_prefix('[')?;
    let close = rest.find("](")?;
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    let target = after[..end].trim();
    (!target.is_empty()).then_some(target)
}

/// The label of an item, linked or not, without its readout.
fn item_label(s: &str) -> String {
    if let Some(l) = link_label(s) {
        return l.trim().to_string();
    }
    s.split_once(" — ")
        .map(|(l, _)| l)
        .unwrap_or(s)
        .trim()
        .to_string()
}

/// A target as compared: `./agents/x/` and `.arbos/agents/x` are
/// `agents/x`; a URL loses only a trailing slash.
fn norm_target(t: &str) -> String {
    if t.contains("://") {
        return t.trim_end_matches('/').to_string();
    }
    let mut t = t.trim();
    for prefix in ["./", ".arbos/"] {
        t = t.strip_prefix(prefix).unwrap_or(t);
    }
    t.trim_end_matches('/').to_string()
}

fn same_target(a: &str, b: &str) -> bool {
    norm_target(a) == norm_target(b)
}

/// The worker a target names: `agents/<id>` or `archive/agents/<id>`,
/// with or without a file under it. None for a PR, a doc, a plain path.
pub fn worker_of_target(target: &str) -> Option<String> {
    let t = norm_target(target);
    let rest = t
        .strip_prefix("agents/")
        .or_else(|| t.strip_prefix("archive/agents/"))?;
    let id = rest.split('/').next()?.trim();
    // Root is the main chat, not a worker: its subscription files sit
    // under `agents/root/…` and a standing check's row links them.
    (!id.is_empty() && id != crate::ROOT_ID).then(|| id.to_string())
}

/// Does `target` point into worker `agent`'s folder (`agents/<id>`, or a
/// file under it)?
fn targets_worker(target: &str, agent: &str) -> bool {
    let t = norm_target(target);
    let dir = format!("agents/{agent}");
    t == dir || t.starts_with(&format!("{dir}/"))
}

/// What `add` did: the item's number, and whether it rewrote an item
/// that already named the same target instead of appending one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Added {
    pub n: usize,
    pub replaced: bool,
}

/// `- [ ] text` / `- [x] text` — and a line that starts with its box,
/// no dash, as the plan tool's own listing prints it (`[ ] 1 text`): a
/// model asked to restructure the page pasted that listing back into
/// notes.md and the items vanished from the kernel's list while the
/// desktop drew them raw (F-194, desktop #661). On a dashless line the
/// ordinal the listing puts before the text is dropped; a dashed item
/// keeps a leading number, since a person wrote it.
fn parse_item_line(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start();
    let (dashed, rest) = match t.strip_prefix("- [") {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('[')?),
    };
    let (mark, after) = rest.split_at(rest.find(']')?);
    let done = match mark {
        " " | "" => false,
        "x" | "X" => true,
        _ => return None,
    };
    let text = after[1..].trim();
    if dashed {
        return Some((done, text));
    }
    // A dashless box is only ever the listing's shape: `[ ] 1 text`.
    // Its text is what follows the ordinal, or the whole thing when no
    // ordinal was pasted.
    let digits = text.chars().take_while(|c| c.is_ascii_digit()).count();
    let text = match text[digits..].strip_prefix(' ') {
        Some(rest) if digits > 0 => rest.trim(),
        _ => text,
    };
    // A bare `[` that opens a link (`[label](url)`) is prose, not a box:
    // the mark check above already refused it (`label` is not a mark).
    Some((done, text))
}

impl Notes {
    pub fn parse(text: &str) -> Self {
        Self {
            lines: text.lines().map(str::to_string).collect(),
            overflow: Vec::new(),
            unread: None,
        }
    }

    /// An empty page standing in for one that could not be read. It
    /// displays as empty and refuses to be saved.
    pub fn unread(why: String) -> Self {
        Self {
            lines: Vec::new(),
            overflow: Vec::new(),
            unread: Some(why),
        }
    }

    /// The read this page came from failed: the reason.
    pub fn unread_reason(&self) -> Option<&str> {
        self.unread.as_deref()
    }

    /// Checked items that left the page since the last `take_overflow`,
    /// oldest first, with the section each sat in.
    pub fn take_overflow(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.overflow)
    }

    /// The `##`/`###` heading text above line `ix`, or empty.
    fn section_at(&self, ix: usize) -> String {
        (0..ix.min(self.lines.len()))
            .rev()
            .find_map(|i| {
                let l = &self.lines[i];
                l.strip_prefix("### ")
                    .or_else(|| l.strip_prefix("## "))
                    .map(|h| h.trim().to_string())
            })
            .unwrap_or_default()
    }

    pub fn render(&self) -> String {
        let mut out = self.lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.items().is_empty()
    }

    /// Every checkbox item, in file order, numbered from 1.
    pub fn items(&self) -> Vec<Item> {
        let mut out = Vec::new();
        let mut section = String::new();
        for line in &self.lines {
            if let Some(h) = line.strip_prefix("## ") {
                section = h.trim().to_string();
            } else if let Some(h) = line.strip_prefix("### ") {
                section = h.trim().to_string();
            } else if let Some((done, text)) = parse_item_line(line) {
                out.push(Item {
                    n: out.len() + 1,
                    section: section.clone(),
                    done,
                    text: text.to_string(),
                });
            }
        }
        out
    }

    pub fn open(&self) -> Vec<Item> {
        self.items().into_iter().filter(|i| !i.done).collect()
    }

    /// Line index of the n-th item.
    fn line_of(&self, n: usize) -> Option<usize> {
        let mut k = 0;
        for (ix, line) in self.lines.iter().enumerate() {
            if parse_item_line(line).is_some() {
                k += 1;
                if k == n {
                    return Some(ix);
                }
            }
        }
        None
    }

    /// Replace the checklist: sections and items as given. The preamble —
    /// front matter and every line before the first heading or item (the
    /// page's top link to `docs/project-context.md`, a `<tldr>`) — stays;
    /// the rest is rewritten. `items` are `(section, text)`; an empty
    /// section name puts the item under the last heading.
    pub fn set(&mut self, items: &[(String, String)]) {
        let mut lines: Vec<String> = self.preamble();
        let mut current = String::new();
        for (section, text) in items {
            let section = section.trim();
            if !section.is_empty() && section != current {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!("## {section}"));
                current = section.to_string();
            }
            lines.push(format!("- [ ] {}", text.trim()));
        }
        self.lines = lines;
    }

    /// Front matter plus every line up to the first `##`/`###` heading or
    /// checkbox item, trailing blank lines dropped.
    fn preamble(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut in_front = false;
        for (i, line) in self.lines.iter().enumerate() {
            let t = line.trim_start();
            if i == 0 && (t == "+++" || t == "---") {
                in_front = true;
                out.push(line.clone());
                continue;
            }
            if in_front {
                out.push(line.clone());
                if t == "+++" || t == "---" {
                    in_front = false;
                }
                continue;
            }
            if t.starts_with("## ") || t.starts_with("### ") || parse_item_line(line).is_some() {
                break;
            }
            out.push(line.clone());
        }
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        out
    }

    /// Add an item under `section` (created at the end when new). An
    /// item that already names the same link target — or, with no link
    /// on either side, the same label — is rewritten in place and
    /// reopened instead of appended: a coordinator that retries a spawn
    /// and adds its row again gets one row, not two.
    pub fn add(&mut self, section: &str, text: &str) -> Added {
        let text = text.trim();
        if let Some(existing) = self.same_item(text) {
            let ix = self.line_of(existing).expect("item line exists");
            self.lines[ix] = format!("- [ ] {text}");
            self.refresh_tldr(text);
            return Added {
                n: existing,
                replaced: true,
            };
        }
        Added {
            n: self.append(section, text),
            replaced: false,
        }
    }

    /// The number of the item `text` would duplicate, if any.
    fn same_item(&self, text: &str) -> Option<usize> {
        let target = link_target(text);
        let label = item_label(text);
        self.items()
            .into_iter()
            .find(|i| match (target, link_target(&i.text)) {
                (Some(a), Some(b)) => same_target(a, b),
                (None, None) => item_label(&i.text).eq_ignore_ascii_case(&label),
                _ => false,
            })
            .map(|i| i.n)
    }

    /// Every open item whose link target names the worker `agent` (the
    /// protocol's shape while a worker runs: target `agents/<id>`).
    pub fn worker_items(&self, agent: &str) -> Vec<Item> {
        self.items()
            .into_iter()
            .filter(|i| !i.done)
            .filter(|i| link_target(&i.text).is_some_and(|t| targets_worker(t, agent)))
            .collect()
    }

    /// The item a `check n` or `update n` means, when its number may be
    /// stale. Rows sink when checked, and the kernel checks a worker's
    /// row the moment the worker is archived — so a parent that listed
    /// the page, awaited three workers and then wrote by number found
    /// the numbers moved under it: *Worker B*'s readout and archive link
    /// landed on *Worker A*'s row and A's on B's (desktop cycle 53). When
    /// the request names a worker (`target`, or the link in `text`) and
    /// item `n` links a different worker, the row that links the named
    /// worker is the one meant; `Some((k, why))` says which and why. None
    /// when `n` is the row meant, or nothing names a worker.
    pub fn resolve_worker_row(&self, n: usize, wanted: &str) -> Option<(usize, String)> {
        let want = worker_of_target(wanted)?;
        let items = self.items();
        let at_n = items.iter().find(|i| i.n == n)?;
        let has = link_target(&at_n.text).and_then(worker_of_target);
        if has.as_deref() == Some(want.as_str()) {
            return None;
        }
        has.as_ref()?;
        let row = items.iter().find(|i| {
            link_target(&i.text).and_then(worker_of_target).as_deref() == Some(want.as_str())
        })?;
        Some((
            row.n,
            format!(
                "Item {n} is now «{}»: rows sink when checked and a worker's row is checked the moment it is archived, so the numbers moved since your list. Item {} is the row that links {}; that one was taken.",
                at_n.text,
                row.n,
                wanted.trim()
            ),
        ))
    }

    fn append(&mut self, section: &str, text: &str) -> usize {
        let section = section.trim();
        let heading = format!("## {section}");
        let insert_at = if section.is_empty() {
            None
        } else {
            self.lines
                .iter()
                .position(|l| l.trim() == heading)
                .map(|h| {
                    // After the last item line of this section.
                    let mut end = h + 1;
                    while end < self.lines.len() && !self.lines[end].starts_with("## ") {
                        end += 1;
                    }
                    while end > h + 1 && self.lines[end - 1].trim().is_empty() {
                        end -= 1;
                    }
                    end
                })
        };
        let line = format!("- [ ] {}", text.trim());
        let at = match insert_at {
            Some(ix) => {
                self.lines.insert(ix, line);
                ix
            }
            None => {
                if !section.is_empty() {
                    if !self.lines.is_empty() {
                        self.lines.push(String::new());
                    }
                    self.lines.push(heading);
                }
                self.lines.push(line);
                self.lines.len() - 1
            }
        };
        // The new item's number: items are numbered through the whole
        // file, and a section in the middle puts it before later ones.
        self.lines[..=at]
            .iter()
            .filter(|l| parse_item_line(l).is_some())
            .count()
    }

    /// Rewrite item `n`'s text (the status readout is part of it).
    pub fn update(&mut self, n: usize, text: &str) -> Result<()> {
        let ix = self.line_of(n).with_context(|| format!("no item {n}"))?;
        let done = parse_item_line(&self.lines[ix])
            .map(|(d, _)| d)
            .unwrap_or(false);
        self.lines[ix] = format!("- [{}] {}", if done { 'x' } else { ' ' }, text.trim());
        self.refresh_tldr(text.trim());
        Ok(())
    }

    /// Check (or uncheck) item `n`, with an optional fresh readout. A
    /// checked item sinks to the end of its section; the section keeps
    /// `DONE_KEPT` checked items, older ones go (git has them).
    pub fn check(&mut self, n: usize, done: bool, readout: Option<&str>) -> Result<Item> {
        self.check_with_target(n, done, readout, None)
    }

    /// `check`, and the link's target moves to `target` when given: when
    /// the deliverable exists the item (and its tldr line) point at it,
    /// not at the worker.
    pub fn check_with_target(
        &mut self,
        n: usize,
        done: bool,
        readout: Option<&str>,
        target: Option<&str>,
    ) -> Result<Item> {
        let ix = self.line_of(n).with_context(|| format!("no item {n}"))?;
        let (_, text) = parse_item_line(&self.lines[ix]).context("not an item")?;
        let mut text = text.to_string();
        if let Some(r) = readout.map(str::trim).filter(|r| !r.is_empty()) {
            let label = text
                .split_once(" — ")
                .map(|(l, _)| l)
                .unwrap_or(&text)
                .to_string();
            text = format!("{label} — {r}");
        }
        if let Some(t) = target.map(str::trim).filter(|t| !t.is_empty()) {
            text = match link_label(&text) {
                Some(label) => {
                    let after = text.split_once(')').map(|(_, rest)| rest).unwrap_or("");
                    format!("[{label}]({t}){after}")
                }
                None => {
                    let (head, rest) = text.split_once(" — ").unwrap_or((&text, ""));
                    if rest.is_empty() {
                        format!("[{head}]({t})")
                    } else {
                        format!("[{head}]({t}) — {rest}")
                    }
                }
            };
        }
        let line = format!("- [{}] {text}", if done { 'x' } else { ' ' });
        self.lines.remove(ix);
        // The section's span after removal.
        let start = (0..ix)
            .rev()
            .find(|&i| self.lines[i].starts_with("## ") || self.lines[i].starts_with("### "))
            .map(|h| h + 1)
            .unwrap_or(0);
        let mut end = start;
        while end < self.lines.len()
            && !self.lines[end].starts_with("## ")
            && !self.lines[end].starts_with("### ")
        {
            end += 1;
        }
        while end > start && self.lines[end - 1].trim().is_empty() {
            end -= 1;
        }
        if done {
            self.lines.insert(end, line);
            // Cap checked items in this section, oldest first (they are
            // in check order: the newest is the one just placed last).
            let mut done_ixs: Vec<usize> = (start..=end)
                .filter(|&i| parse_item_line(&self.lines[i]).is_some_and(|(d, _)| d))
                .collect();
            while done_ixs.len() > DONE_KEPT {
                let drop = done_ixs.remove(0);
                let section = self.section_at(drop);
                let gone = self.lines.remove(drop);
                self.overflow.push((section, gone));
                done_ixs.iter_mut().for_each(|i| *i -= 1);
            }
        } else {
            // Back among the open ones: before the first checked item.
            let first_done = (start..end)
                .find(|&i| parse_item_line(&self.lines[i]).is_some_and(|(d, _)| d))
                .unwrap_or(end);
            self.lines.insert(first_done, line);
        }
        self.refresh_tldr(&text);
        let items = self.items();
        items
            .into_iter()
            .find(|i| i.text == text && i.done == done)
            .context("item lost while moving")
    }

    /// The `<tldr>` after a touch of the item `text` (Cursor's rule): the
    /// bullet with the same `[label]` is rewritten fresh and moved to the
    /// top (the most recently touched workstream first), bullets whose
    /// item is no longer on the page are dropped, at most `TLDR_CAP`
    /// bullets stay. A page without a tldr gets one only once it is big
    /// (`TLDR_SECTIONS` sections and `TLDR_ITEMS` items); a tldr the
    /// coordinator wrote by hand on a small page is kept as it is.
    fn refresh_tldr(&mut self, text: &str) {
        let Some(label) = link_label(text).map(str::to_string) else {
            return;
        };
        let (open, close) = match self.tldr_span() {
            Some(span) => span,
            None => {
                if !self.page_is_big() {
                    return;
                }
                let at = self.tldr_insert_at();
                self.lines.insert(at, "<tldr>".to_string());
                self.lines.insert(at + 1, "</tldr>".to_string());
                self.lines.insert(at + 2, String::new());
                (at, at + 1)
            }
        };
        let labels_on_page: Vec<String> = self.items().iter().map(Item::label).collect();
        let mut bullets: Vec<String> = self.lines[open + 1..close]
            .iter()
            .filter_map(|l| l.trim().strip_prefix("- ").map(str::to_string))
            .filter(|b| {
                let same = link_label(b).is_some_and(|l| l.eq_ignore_ascii_case(&label));
                let alive = link_label(b)
                    .is_none_or(|l| labels_on_page.iter().any(|p| p.eq_ignore_ascii_case(l)));
                !same && alive
            })
            .collect();
        bullets.insert(0, text.to_string());
        bullets.truncate(TLDR_CAP);
        let mut fresh: Vec<String> = bullets.into_iter().map(|b| format!("- {b}")).collect();
        self.lines.splice(open + 1..close, fresh.drain(..));
    }

    /// `(open, close)` line indices of `<tldr>` … `</tldr>`.
    fn tldr_span(&self) -> Option<(usize, usize)> {
        let open = self.lines.iter().position(|l| l.trim() == "<tldr>")?;
        let close = self.lines[open..]
            .iter()
            .position(|l| l.trim() == "</tldr>")
            .map(|c| open + c)?;
        Some((open, close))
    }

    /// Cursor's threshold for a tldr: several sub-projects, six or more items.
    pub fn page_is_big(&self) -> bool {
        let sections = self.lines.iter().filter(|l| l.starts_with("## ")).count();
        sections >= TLDR_SECTIONS && self.items().len() >= TLDR_ITEMS
    }

    /// Where a new tldr goes: after the preamble (front matter, title,
    /// the top link line), before the first heading or item.
    fn tldr_insert_at(&self) -> usize {
        let mut ix = 0;
        // Skip front matter.
        if self
            .lines
            .first()
            .is_some_and(|l| l.trim() == "+++" || l.trim() == "---")
        {
            let fence = self.lines[0].trim().to_string();
            if let Some(end) = self.lines[1..].iter().position(|l| l.trim() == fence) {
                ix = end + 2;
            }
        }
        while ix < self.lines.len() {
            let t = self.lines[ix].trim();
            if t.starts_with("## ")
                || t.starts_with("### ")
                || parse_item_line(&self.lines[ix]).is_some()
            {
                break;
            }
            ix += 1;
        }
        // Back over trailing blanks so the block sits under the prose.
        while ix > 0
            && self.lines[ix - 1].trim().is_empty()
            && ix > 1
            && self.lines[ix - 2].trim().is_empty()
        {
            ix -= 1;
        }
        ix
    }

    pub fn remove(&mut self, n: usize) -> Result<Item> {
        let ix = self.line_of(n).with_context(|| format!("no item {n}"))?;
        let item = self.items().into_iter().nth(n - 1).context("no item")?;
        self.lines.remove(ix);
        Ok(item)
    }

    /// `[ ] 1 text` lines for the prompt and the tool's `show`.
    pub fn show(&self) -> String {
        let items = self.items();
        if items.is_empty() {
            return "(no notes)".into();
        }
        let mut out = String::new();
        let mut section = String::new();
        for i in items {
            if i.section != section {
                section = i.section.clone();
                if !section.is_empty() {
                    out.push_str(&format!("## {section}\n"));
                }
            }
            out.push_str(&format!(
                "[{}] {} {}\n",
                if i.done { 'x' } else { ' ' },
                i.n,
                i.text
            ));
        }
        out.trim_end().to_string()
    }
}

/// The item text from one object the model sent: `text` as is, or
/// `[label](target) — readout` from the parts, or a `goal`/`title`/
/// `name`/`description`/`step`/`task` field. None when nothing reads as
/// an item.
fn item_text(o: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let str_of = |k: &str| {
        o.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(t) = str_of("text") {
        return Some(t.to_string());
    }
    let label = str_of("label")
        .or_else(|| str_of("goal"))
        .or_else(|| str_of("title"))
        .or_else(|| str_of("name"))
        .or_else(|| str_of("step"))
        .or_else(|| str_of("task"))
        .or_else(|| str_of("description"))?;
    let readout = str_of("readout")
        .or_else(|| str_of("status"))
        .or_else(|| str_of("outcome"));
    let mut text = match str_of("target")
        .or_else(|| str_of("link"))
        .or_else(|| str_of("url"))
    {
        Some(target) => format!("[{label}]({target})"),
        None => label.to_string(),
    };
    if let Some(r) = readout {
        text.push_str(" — ");
        text.push_str(r);
    }
    Some(text)
}

/// Children of an item object: `children`, `items`, `steps`, `subtasks`,
/// `tasks`, `nodes`, `goals` — the shapes a "plan graph" comes in.
fn children_of(o: &serde_json::Map<String, serde_json::Value>) -> Option<&Vec<serde_json::Value>> {
    [
        "children", "items", "steps", "subtasks", "tasks", "nodes", "goals", "subgoals",
    ]
    .iter()
    .find_map(|k| o.get(*k).and_then(|v| v.as_array()))
    .filter(|a| !a.is_empty())
}

/// The accepted shapes, for an error the model can act on.
pub const SHAPES: &str = "plan takes one of: {\"op\":\"set\",\"items\":[\"text\", {\"section\":\"Phase 1\",\"text\":\"[label](target) — readout\"}, {\"label\":\"…\",\"readout\":\"…\"}]} · {\"text\":\"- [ ] one\\n## Section\\n- [ ] two\"} (a markdown checklist) · nested {\"goals\":[{\"goal\":\"…\",\"children\":[…]}]} (a parent with children becomes a ## section) · {\"op\":\"add\",\"text\":\"…\"} · {\"op\":\"check\",\"n\":1,\"readout\":\"…\"} · {\"op\":\"show\"}";

/// A checklist from a JSON array the model sent: strings, `{section,
/// text}`, `{label, target, readout}`, or nested goal objects whose
/// children become the items of a section named for the parent.
pub fn items_from_json(v: &serde_json::Value) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut section = String::new();
    fn walk(
        arr: &[serde_json::Value],
        section: &mut String,
        out: &mut Vec<(String, String)>,
        depth: usize,
    ) -> Result<()> {
        for it in arr {
            match it {
                serde_json::Value::String(s) if !s.trim().is_empty() => {
                    let parsed = parse_checklist_text(s);
                    if parsed.is_empty() {
                        // One plain string is one item.
                        out.push((section.clone(), s.trim().to_string()));
                    }
                    for (sec, text) in parsed {
                        if !sec.is_empty() {
                            *section = sec;
                        }
                        out.push((section.clone(), text));
                    }
                }
                serde_json::Value::String(_) => {}
                serde_json::Value::Object(o) => {
                    if let Some(s) = o.get("section").and_then(|s| s.as_str()) {
                        *section = s.trim().to_string();
                    }
                    let text = item_text(o);
                    match (children_of(o), text) {
                        // A parent with children is a section; its children
                        // are the lines. Deeper nesting flattens under it.
                        (Some(kids), Some(title)) if depth == 0 => {
                            *section = crate::text::clip(&title, 60);
                            walk(kids, section, out, depth + 1)?;
                        }
                        (Some(kids), Some(title)) => {
                            out.push((section.clone(), title));
                            walk(kids, section, out, depth + 1)?;
                        }
                        (Some(kids), None) => walk(kids, section, out, depth + 1)?,
                        (None, Some(text)) => out.push((section.clone(), text)),
                        (None, None) => {
                            if o.get("section").is_none() {
                                bail!(
                                    "an item needs text (or label, goal, title): got {}",
                                    crate::text::clip(&it.to_string(), 80)
                                );
                            }
                        }
                    }
                }
                serde_json::Value::Array(inner) => walk(inner, section, out, depth)?,
                other => bail!(
                    "an item is a string or an object, not {}",
                    crate::text::clip(&other.to_string(), 40)
                ),
            }
        }
        Ok(())
    }
    let arr = match v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::String(s) => return Ok(parse_checklist_text(s)),
        serde_json::Value::Object(o) => match children_of(o) {
            Some(kids) => kids.clone(),
            None => vec![v.clone()],
        },
        _ => bail!("items must be an array"),
    };
    walk(&arr, &mut section, &mut out, 0)?;
    if out.is_empty() {
        bail!("no items found");
    }
    Ok(out)
}

/// A markdown checklist as text: `## Section` headings, `- [ ] item`,
/// `- [x] item`, plain `- item` / `* item`, and `1. item` lines. Anything
/// else is skipped.
pub fn parse_checklist_text(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut section = String::new();
    for raw in text.lines() {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t
            .strip_prefix("### ")
            .or_else(|| t.strip_prefix("## "))
            .or_else(|| t.strip_prefix("# "))
        {
            section = h.trim().to_string();
            continue;
        }
        if let Some((_, item)) = parse_item_line(t) {
            out.push((section.clone(), item.to_string()));
            continue;
        }
        if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            out.push((section.clone(), rest.trim().to_string()));
            continue;
        }
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty()
            && let Some(rest) = t[digits.len()..]
                .strip_prefix(". ")
                .or_else(|| t[digits.len()..].strip_prefix(") "))
        {
            out.push((section.clone(), rest.trim().to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[project context](docs/project-context.md)\n\n## Kernel\n- [ ] [#96](https://x/96) — harness re-run running\n- [ ] [leash](https://x/97) — review\n- [x] [#95](https://x/95) — merged\n\n## Desktop\n- [ ] panel — waiting on layout\n";

    /// F-194: the plan tool's own listing pasted back into the page —
    /// a box and an ordinal, no dash — reads as items, with the ordinal
    /// dropped; a dashed item keeps a leading number a person wrote; a
    /// link that opens with `[` is still prose; `show` round-trips.
    #[test]
    fn a_line_that_starts_with_its_box_is_an_item_and_the_listings_ordinal_goes() {
        let pasted = "# Page

[project context](docs/project-context.md)

## Goal
[ ] 1 [Kickoff](docs/project-context.md) — ready; waiting for the first ask
[x] 2 [OCI Layout](archive/agents/a) — worker finished
- [x] [Word Counts](docs/word_counts.md) — ready
- [ ] 3 files to review
[ ] no ordinal here
";
        let n = Notes::parse(pasted);
        let items = n.items();
        assert_eq!(items.len(), 5, "{items:#?}");
        assert_eq!(items[0].label(), "Kickoff");
        assert!(!items[0].done);
        assert_eq!(items[0].readout(), Some("ready; waiting for the first ask"));
        assert_eq!(items[1].label(), "OCI Layout");
        assert!(items[1].done);
        assert_eq!(items[2].label(), "Word Counts");
        assert_eq!(
            items[3].text, "3 files to review",
            "a dashed item keeps its number"
        );
        assert_eq!(items[4].text, "no ordinal here");
        assert!(
            n.render().contains("[ ] 1 [Kickoff"),
            "reading does not rewrite the page"
        );
        // The listing reads back as the same items.
        let shown = n.show();
        let again = Notes::parse(&shown).items();
        assert_eq!(
            again.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
            items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
            "{shown}"
        );
    }

    #[test]
    fn items_are_numbered_across_sections_and_prose_survives() {
        let n = Notes::parse(SAMPLE);
        let items = n.items();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].section, "Kernel");
        assert_eq!(items[0].label(), "#96");
        assert_eq!(items[0].readout(), Some("harness re-run running"));
        assert_eq!(items[3].section, "Desktop");
        assert!(items[2].done);
        assert!(n.render().starts_with("[project context]"));
    }

    #[test]
    fn checking_sinks_the_item_and_keeps_three_done() {
        let mut n = Notes::parse(SAMPLE);
        let it = n.check(1, true, Some("solved 1 of 3")).unwrap();
        assert!(it.done && it.text.ends_with("— solved 1 of 3"));
        let items = n.items();
        // Open leash first, then the two done ones, #95 before #96.
        assert_eq!(items[0].label(), "leash");
        assert!(items[1].done && items[1].label() == "#95");
        assert!(items[2].done && items[2].label() == "#96");
        for k in 0..3 {
            n.add("Kernel", &format!("t{k}"));
            let last = n
                .items()
                .into_iter()
                .filter(|i| i.section == "Kernel" && !i.done)
                .last()
                .unwrap();
            n.check(last.n, true, None).unwrap();
        }
        let done: Vec<_> = n
            .items()
            .into_iter()
            .filter(|i| i.section == "Kernel" && i.done)
            .collect();
        assert_eq!(done.len(), DONE_KEPT);
        assert_eq!(done.last().unwrap().text, "t2");
        assert!(
            n.items().iter().any(|i| i.section == "Desktop"),
            "{}",
            n.render()
        );
    }

    #[test]
    fn a_fourth_checked_item_leaves_the_section_as_overflow_not_thin_air() {
        let mut n = Notes::parse(
            "## Work\n- [ ] [a](x) — 1\n- [ ] [b](x) — 2\n- [ ] [c](x) — 3\n- [ ] [d](x) — 4\n",
        );
        for _ in 0..4 {
            n.check(1, true, Some("done")).unwrap();
        }
        let text = n.render();
        assert!(
            !text.contains("[a](x)"),
            "the oldest checked item left: {text}"
        );
        assert!(text.contains("[b](x)") && text.contains("[d](x)"), "{text}");
        let gone = n.take_overflow();
        assert_eq!(
            gone,
            vec![("Work".to_string(), "- [x] [a](x) — done".to_string())]
        );
        assert!(n.take_overflow().is_empty(), "drained once");

        let dir = std::env::temp_dir().join(format!("arbos-notes-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let archived = dir.join("archived.md");
        archive_overflow(&archived, &gone).unwrap();
        archive_overflow(
            &archived,
            &[
                ("Work".to_string(), "- [x] [b](x) — done".to_string()),
                ("Other".to_string(), "- [x] [z](x) — done".to_string()),
            ],
        )
        .unwrap();
        let text = std::fs::read_to_string(&archived).unwrap();
        assert!(text.starts_with("# Archived"), "template head kept: {text}");
        let work = text.find("## Work").unwrap();
        let other = text.find("## Other").unwrap();
        assert!(work < other);
        let a = text.find("[a](x)").unwrap();
        let b = text.find("[b](x)").unwrap();
        assert!(
            work < a && a < b && b < other,
            "newest last within its section: {text}"
        );
    }

    #[test]
    fn a_tldr_is_made_once_the_page_is_big_and_keeps_the_freshest_four() {
        let small = "[project context](docs/project-context.md)\n\n## Work\n- [ ] [a](agents/a) — running\n- [ ] [b](agents/b) — running\n";
        let mut n = Notes::parse(small);
        n.check(1, true, Some("landed")).unwrap();
        assert!(
            !n.render().contains("<tldr>"),
            "a small page gets no tldr: {}",
            n.render()
        );

        let big = "+++\nowner = \"root\"\n+++\n# Notes\n\n[project context](docs/project-context.md)\n\n## Voice\n- [ ] [a](agents/a) — 1\n- [ ] [b](agents/b) — 2\n- [ ] [c](agents/c) — 3\n\n## Loops\n- [ ] [d](agents/d) — 4\n- [ ] [e](agents/e) — 5\n- [ ] [f](agents/f) — 6\n";
        let mut n = Notes::parse(big);
        assert!(n.page_is_big());
        n.update(4, "[d](agents/d) — d moved").unwrap();
        let text = n.render();
        let tldr_at = text.find("<tldr>").unwrap();
        assert!(
            tldr_at > text.find("project-context").unwrap()
                && tldr_at < text.find("## Voice").unwrap(),
            "after the preamble, before the first section: {text}"
        );
        assert!(
            text.contains("<tldr>\n- [d](agents/d) — d moved\n</tldr>"),
            "{text}"
        );
        for (k, label) in [(1, "a"), (2, "b"), (3, "c"), (5, "e")] {
            n.update(k, &format!("[{label}](agents/{label}) — {label} moved"))
                .unwrap();
        }
        let text = n.render();
        let open = text.find("<tldr>").unwrap();
        let close = text.find("</tldr>").unwrap();
        let block = &text[open..close];
        let bullets: Vec<&str> = block.lines().filter(|l| l.starts_with("- ")).collect();
        assert_eq!(bullets.len(), TLDR_CAP, "{block}");
        assert!(bullets[0].starts_with("- [e]"), "freshest first: {block}");
        assert!(!block.contains("[d]"), "the oldest touch fell off: {block}");
        // Checking with a target rewrites the bullet and moves it up; a
        // bullet whose item is gone from the page is dropped.
        n.check_with_target(1, true, Some("landed"), Some("docs/a.md"))
            .unwrap();
        let text = n.render();
        assert!(
            text.contains("<tldr>\n- [a](docs/a.md) — landed\n"),
            "{text}"
        );
        n.remove(n.items().iter().find(|i| i.label() == "e").unwrap().n)
            .unwrap();
        n.update(1, "[b](agents/b) — b again").unwrap();
        assert!(
            !n.render()[n.render().find("<tldr>").unwrap()..n.render().find("</tldr>").unwrap()]
                .contains("[e]"),
            "{}",
            n.render()
        );
    }

    #[test]
    fn a_check_with_a_readout_rewrites_the_matching_tldr_line() {
        let page = "[project context](docs/project-context.md)\n\n<tldr>\n- [River poem](agents/river) — worker pending\n- [Colour table](agents/colour) — worker pending\n</tldr>\n\n## Work\n- [ ] [River poem](agents/river) — worker pending\n- [ ] [Colour table](agents/colour) — worker pending\n";
        let mut n = Notes::parse(page);
        n.check_with_target(
            1,
            true,
            Some("landed, 12 lines"),
            Some("docs/river-poem.md"),
        )
        .unwrap();
        let text = n.render();
        assert!(text.contains("<tldr>\n- [River poem](docs/river-poem.md) — landed, 12 lines\n- [Colour table](agents/colour) — worker pending\n</tldr>"), "{text}");
        assert!(
            text.contains("- [x] [River poem](docs/river-poem.md) — landed, 12 lines"),
            "{text}"
        );
    }

    #[test]
    fn set_keeps_the_page_preamble() {
        let mut n = Notes::parse(
            "+++\nowner = \"root\"\n+++\n# Notes\n\nGoals: [project-context](docs/project-context.md)\n\n## Old\n- [ ] gone\n",
        );
        n.set(&[("New".into(), "kept".into())]);
        let text = n.render();
        assert!(text.starts_with("+++\nowner = \"root\"\n+++\n# Notes\n\nGoals: [project-context](docs/project-context.md)\n\n## New\n- [ ] kept\n"), "{text}");
        assert!(!text.contains("gone"));
    }

    #[test]
    fn the_common_shapes_all_read_as_items() {
        let v: serde_json::Value = serde_json::from_str(r#"["a", {"section":"S","text":"b"}, {"label":"PR 1","target":"https://x/1","readout":"open"}]"#).unwrap();
        let items = items_from_json(&v).unwrap();
        assert_eq!(
            items,
            vec![
                ("".to_string(), "a".to_string()),
                ("S".to_string(), "b".to_string()),
                ("S".to_string(), "[PR 1](https://x/1) — open".to_string())
            ]
        );
        let graph: serde_json::Value = serde_json::from_str(r#"{"goals":[{"goal":"Profile the sort","children":[{"goal":"time it","status":"todo"},{"title":"count comparisons"}]},{"goal":"Ship","steps":["write the test"]}]}"#).unwrap();
        let items = items_from_json(&graph).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(
            items[0],
            ("Profile the sort".to_string(), "time it — todo".to_string())
        );
        assert_eq!(items[2], ("Ship".to_string(), "write the test".to_string()));
        let md = items_from_json(&serde_json::Value::String(
            "## Phase 1\n- [ ] read\n- [x] plan\n1. run tests\n".into(),
        ))
        .unwrap();
        assert_eq!(md.len(), 3);
        assert_eq!(md[2], ("Phase 1".to_string(), "run tests".to_string()));
        let err = items_from_json(&serde_json::json!([{"when": {"every": "1h"}}])).unwrap_err();
        assert!(format!("{err:#}").contains("needs text"), "{err:#}");
    }

    #[test]
    fn set_add_update_remove_round_trip() {
        let mut n = Notes::default();
        n.set(&[
            ("Plan".into(), "read the issue".into()),
            ("Plan".into(), "fix".into()),
            ("Verify".into(), "run tests".into()),
        ]);
        assert_eq!(n.items().len(), 3);
        // Its number is its place in the file (Plan comes before Verify),
        // not the count of items.
        assert_eq!(n.add("Plan", "commit").n, 3);
        assert_eq!(n.items()[2].text, "commit");
        n.update(1, "read the issue — done reading").unwrap();
        assert_eq!(n.items()[0].readout(), Some("done reading"));
        n.remove(2).unwrap();
        assert_eq!(n.items().len(), 3);
        assert!(
            n.show().contains("## Verify\n[ ] 3 run tests"),
            "{}",
            n.show()
        );
        assert!(n.render().contains("## Plan\n- [ ] read the issue — done reading\n- [ ] commit\n\n## Verify\n- [ ] run tests\n"), "{}", n.render());
    }

    #[test]
    fn a_whole_page_write_keeps_the_front_matter_and_the_context_link() {
        let old = crate::store::NOTES_TEMPLATE;
        // The kickoff coordinator's write: its own title, no head.
        let new = "# Project Status\n\n- [ ] Speak in full duplex — pending\n";
        let kept = keep_page_head(old, new);
        assert!(
            kept.starts_with("+++\nowner = \"root\"\n+++\n# Project Status\n\n"),
            "{kept}"
        );
        assert!(
            kept.contains("[project-context](docs/project-context.md)"),
            "{kept}"
        );
        assert!(
            kept.ends_with("- [ ] Speak in full duplex — pending\n"),
            "{kept}"
        );
        assert_eq!(kept.matches("# Project Status").count(), 1);
        // No title at all: the page's own.
        let kept = keep_page_head(old, "- [ ] one\n- [ ] two\n");
        assert!(
            kept.starts_with("+++\nowner = \"root\"\n+++\n# Notes\n\nGoals, constraints"),
            "{kept}"
        );
        assert!(kept.ends_with("- [ ] one\n- [ ] two\n"), "{kept}");
        // A write that already has both is left exactly as written.
        let full = "+++\nowner = \"root\"\n+++\n# Notes\n\n[project context](docs/project-context.md)\n\n## Work\n- [ ] x\n";
        assert_eq!(keep_page_head(old, full), full);
        // Front matter of its own but no link: the link is added, the
        // front matter is the new one.
        let kept = keep_page_head(
            old,
            "---\nowner = \"root\"\nextra = 1\n---\n# Page\n\n- [ ] x\n",
        );
        assert!(
            kept.starts_with("---\nowner = \"root\"\nextra = 1\n---\n# Page\n\nGoals"),
            "{kept}"
        );
        let items = Notes::parse(&kept).items();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn adding_the_same_target_again_rewrites_the_row() {
        let mut n = Notes::parse(SAMPLE);
        let again = n.add(
            "Kernel",
            "[Edge review](agents/math-edge-review) — worker running",
        );
        assert!(!again.replaced);
        let retry = n.add(
            "Kernel",
            "[Edge review](.arbos/agents/math-edge-review/) — pending retry with a fresh branch",
        );
        assert!(retry.replaced);
        assert_eq!(retry.n, again.n);
        let rows: Vec<_> = n
            .items()
            .into_iter()
            .filter(|i| i.label() == "Edge review")
            .collect();
        assert_eq!(rows.len(), 1, "{}", n.render());
        assert_eq!(rows[0].readout(), Some("pending retry with a fresh branch"));
        // Same label, no link on either side: one row too.
        let a = n.add("Desktop", "panel — waiting on layout");
        assert!(a.replaced, "{}", n.render());
        // A different worker with a like name is its own row.
        let other = n.add(
            "Kernel",
            "[Edge review 2](agents/math-edge-review-2) — running",
        );
        assert!(!other.replaced);
    }

    #[test]
    fn worker_rows_are_found_by_their_agents_target() {
        let mut n = Notes::parse(SAMPLE);
        n.add(
            "Kernel",
            "[Docstrings](agents/math-docstrings) — worker running",
        );
        n.add(
            "Kernel",
            "[Notes](agents/math-docstrings/notes.md) — the worker's list",
        );
        n.add("Kernel", "[Other](agents/math-docstrings-2) — running");
        let mine = n.worker_items("math-docstrings");
        assert_eq!(mine.len(), 2, "{mine:?}");
        assert!(n.worker_items("nobody").is_empty());
    }
}

#[cfg(test)]
mod stale_number_tests {
    use super::*;

    /// Desktop cycle 53: root listed the page (1 A, 2 B, 3 C), awaited
    /// three workers, and wrote by number; meanwhile the kernel had
    /// checked A's row as A was archived, and A sank — so `check 2` hit C
    /// and `check 1` hit B, and the page read *Worker B — A's report*.
    /// The worker the request names decides the row.
    #[test]
    fn a_stale_number_resolves_to_the_row_that_links_the_named_worker() {
        let mut page = Notes::parse(
            "## Running Workers\n- [ ] [Worker A](agents/worker-a) — running\n- [ ] [Worker B](agents/worker-b) — running\n- [ ] [Worker C](agents/worker-c) — running\n",
        );
        // The kernel retires A: checked, sinks to the end of the section.
        let a = page.worker_items("worker-a")[0].n;
        page.check_with_target(
            a,
            true,
            Some("worker finished: the table"),
            Some("archive/agents/worker-a"),
        )
        .unwrap();
        let texts: Vec<String> = page.items().iter().map(|i| i.text.clone()).collect();
        assert!(texts[0].starts_with("[Worker B]"), "{texts:?}");
        assert!(texts[2].starts_with("[Worker A]"), "{texts:?}");
        // Root's `check 2 target:agents/worker-b`, 2 being B in its list.
        // Item 2 is now C: the row that links worker-b is taken instead.
        let (row, why) = page
            .resolve_worker_row(2, "agents/worker-b")
            .expect("redirected");
        assert_eq!(row, 1, "{why}");
        assert!(why.contains("Item 2 is now «[Worker C]"), "{why}");
        assert!(
            why.contains("Item 1 is the row that links agents/worker-b"),
            "{why}"
        );
        page.check_with_target(
            row,
            true,
            Some("the ls error"),
            Some("archive/agents/worker-b"),
        )
        .unwrap();
        // `check 1 target:agents/worker-a` from the stale list: 1 is C now;
        // A's row is done already — still the one that links A.
        let (row, _) = page
            .resolve_worker_row(1, "agents/worker-a")
            .expect("redirected");
        let items = page.items();
        assert!(items[row - 1].text.starts_with("[Worker A]"), "{items:?}");
        // No redirect when the number is right, when the request names
        // no worker (a PR, a doc), or when item n is not a worker row.
        assert!(page.resolve_worker_row(row, "agents/worker-a").is_none());
        assert!(page.resolve_worker_row(1, "docs/x.md").is_none());
        assert!(
            page.resolve_worker_row(1, "https://github.com/o/r/pull/1")
                .is_none()
        );
        let mut page2 =
            Notes::parse("## S\n- [ ] [Plain](docs/a.md) — x\n- [ ] [W](agents/w) — running\n");
        assert!(
            page2.resolve_worker_row(1, "agents/w").is_none(),
            "item 1 is a doc row: the model may repoint it"
        );
        page2.update(1, "[Plain](docs/a.md) — y").unwrap();
        // Every label sits with its own worker's readout.
        let final_texts: Vec<String> = page.items().iter().map(|i| i.text.clone()).collect();
        assert!(
            final_texts.iter().any(|t| t
                .starts_with("[Worker A](archive/agents/worker-a) — worker finished: the table")),
            "{final_texts:?}"
        );
        assert!(
            final_texts
                .iter()
                .any(|t| t.starts_with("[Worker B](archive/agents/worker-b) — the ls error")),
            "{final_texts:?}"
        );
        assert_eq!(
            worker_of_target(".arbos/archive/agents/x/transcript.jsonl").as_deref(),
            Some("x")
        );
        assert_eq!(
            worker_of_target("agents/root/subscriptions/0001.toml"),
            None,
            "root is not a worker"
        );
        assert_eq!(worker_of_target("docs/agents/plan.md"), None);
    }
}
