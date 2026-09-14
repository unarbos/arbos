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
}

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

/// Is `candidate` this place's project page?
pub fn is_project_page(place_root: &Path, candidate: &Path) -> bool {
    let page = place_root.join(".arbos").join("notes.md");
    let a = std::fs::canonicalize(&page).unwrap_or(page);
    let b = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    a == b
}

pub const PAGE_REFUSAL: &str = ".arbos/notes.md is the project page, written by root only; keep your own checklist with the plan tool and tell root with say to=root";

pub fn load(place: &Place, agent: &str) -> Notes {
    Notes::parse(&std::fs::read_to_string(path(place, agent)).unwrap_or_default())
}

pub fn save(place: &Place, agent: &str, notes: &Notes) -> Result<()> {
    let p = path(place, agent);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension(format!("md.{}.tmp", std::process::id()));
    std::fs::write(&tmp, notes.render()).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &p).with_context(|| format!("replace {}", p.display()))?;
    Ok(())
}

pub fn read_path(path: &Path) -> Notes {
    Notes::parse(&std::fs::read_to_string(path).unwrap_or_default())
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

fn parse_item_line(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("- [")?;
    let (mark, after) = rest.split_at(rest.find(']')?);
    let done = match mark {
        " " | "" => false,
        "x" | "X" => true,
        _ => return None,
    };
    Some((done, after[1..].trim()))
}

impl Notes {
    pub fn parse(text: &str) -> Self {
        Self {
            lines: text.lines().map(str::to_string).collect(),
        }
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
                self.lines.remove(drop);
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

    /// The `<tldr>` bullet that names the same `[label]` as `text` becomes
    /// `text`: a tldr entry is a fresh readout, rewritten on every state
    /// change, and points where the item points (the deliverable once it
    /// exists). Nothing happens without a tldr or without a matching label.
    fn refresh_tldr(&mut self, text: &str) {
        let Some(label) = link_label(text).map(str::to_string) else {
            return;
        };
        let mut in_tldr = false;
        for line in self.lines.iter_mut() {
            let t = line.trim();
            if t == "<tldr>" {
                in_tldr = true;
                continue;
            }
            if t == "</tldr>" {
                break;
            }
            if !in_tldr {
                continue;
            }
            if let Some(rest) = t.strip_prefix("- ")
                && link_label(rest).is_some_and(|l| l.eq_ignore_ascii_case(&label))
            {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                *line = format!("{indent}- {text}");
            }
        }
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
