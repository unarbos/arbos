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

pub fn path(place: &Place, agent: &str) -> PathBuf {
    place.agent_dir(agent).join("notes.md")
}

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

    /// Replace the whole checklist: sections and items as given, prose
    /// dropped. `items` are `(section, text)`; an empty section name puts
    /// the item under the last heading.
    pub fn set(&mut self, items: &[(String, String)]) {
        let mut lines: Vec<String> = Vec::new();
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

    /// Append an item under `section` (created at the end when new).
    pub fn add(&mut self, section: &str, text: &str) -> usize {
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
        match insert_at {
            Some(ix) => self.lines.insert(ix, line),
            None => {
                if !section.is_empty() {
                    if !self.lines.is_empty() {
                        self.lines.push(String::new());
                    }
                    self.lines.push(heading);
                }
                self.lines.push(line);
            }
        }
        self.items().len()
    }

    /// Rewrite item `n`'s text (the status readout is part of it).
    pub fn update(&mut self, n: usize, text: &str) -> Result<()> {
        let ix = self.line_of(n).with_context(|| format!("no item {n}"))?;
        let done = parse_item_line(&self.lines[ix])
            .map(|(d, _)| d)
            .unwrap_or(false);
        self.lines[ix] = format!("- [{}] {}", if done { 'x' } else { ' ' }, text.trim());
        Ok(())
    }

    /// Check (or uncheck) item `n`, with an optional fresh readout. A
    /// checked item sinks to the end of its section; the section keeps
    /// `DONE_KEPT` checked items, older ones go (git has them).
    pub fn check(&mut self, n: usize, done: bool, readout: Option<&str>) -> Result<Item> {
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
        let items = self.items();
        items
            .into_iter()
            .find(|i| i.text == text && i.done == done)
            .context("item lost while moving")
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

/// The `plan set` items argument: `["text", {"section": "..", "text": ".."}]`.
pub fn items_from_json(v: &serde_json::Value) -> Result<Vec<(String, String)>> {
    let arr = v.as_array().context("items must be an array")?;
    let mut out = Vec::new();
    let mut section = String::new();
    for it in arr {
        match it {
            serde_json::Value::String(s) => out.push((section.clone(), s.clone())),
            serde_json::Value::Object(o) => {
                if let Some(s) = o.get("section").and_then(|s| s.as_str()) {
                    section = s.to_string();
                }
                let text = o
                    .get("text")
                    .and_then(|t| t.as_str())
                    .context("item needs text")?;
                out.push((section.clone(), text.to_string()));
            }
            _ => bail!("an item is a string or {{section, text}}"),
        }
    }
    Ok(out)
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
    fn set_add_update_remove_round_trip() {
        let mut n = Notes::default();
        n.set(&[
            ("Plan".into(), "read the issue".into()),
            ("Plan".into(), "fix".into()),
            ("Verify".into(), "run tests".into()),
        ]);
        assert_eq!(n.items().len(), 3);
        assert_eq!(n.add("Plan", "commit"), 4);
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
}
