//! Standing voice brief: glances the kernel keeps so Live can read them.
//!
//! Jev ranks slices on a kernel turn. The gateway only reads the file.
//! Never called from `session.start`. Never called from the gateway.

use anyhow::Result;
use arbos_core::{
    EventKind, Place, list_agents, load_transcript, notes, record, text, waiting,
};
use serde::Serialize;
use std::path::PathBuf;

/// Markdown the gateway injects.
pub const BRIEF_MD: &str = "voice-brief.md";
/// Same sections as data, so a client can parse without scraping.
pub const BRIEF_JSON: &str = "voice-brief.json";
/// About 2,000 tokens. One start inject always fits.
const BUDGET_TOKENS: u64 = 2_000;
/// One glance. Not a file body.
const GLANCE_CHARS: usize = 240;
/// Fill order when Jev did not name `keep`.
const DEFAULT_ORDER: &[&str] = &["tldr", "working", "last", "ask", "branch"];

/// Ranking from the controller object. Empty `keep` means default order.
#[derive(Debug, Clone, Default)]
pub struct Ranking {
    pub keep: Vec<String>,
    pub pointers: Vec<String>,
}

/// One existing line the brief may hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    pub id: String,
    pub kind: String,
    pub body: String,
    pub pointer: String,
}

/// What a read of the brief file found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefRead {
    Present(String),
    Absent,
    Unknown(String),
}

#[derive(Debug, Clone, Serialize)]
struct BriefFile {
    sections: Vec<BriefSection>,
    overflow: Vec<BriefPointer>,
}

#[derive(Debug, Clone, Serialize)]
struct BriefSection {
    id: String,
    kind: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct BriefPointer {
    id: String,
    pointer: String,
}

/// Paths under `.arbos/`.
pub fn paths(place: &Place) -> (PathBuf, PathBuf) {
    let dir = place.arbos();
    (dir.join(BRIEF_MD), dir.join(BRIEF_JSON))
}

/// Delete both files so a stale pack cannot ride after `jev = false`.
pub fn delete(place: &Place) {
    let (md, json) = paths(place);
    let _ = std::fs::remove_file(md);
    let _ = std::fs::remove_file(json);
}

/// Gather existing glances, pack, write. Identity is not in the file.
pub fn refresh(place: &Place, ranking: &Ranking) -> Result<()> {
    let slices = gather(place);
    let packed = pack(&slices, ranking);
    write_packed(place, &packed)
}

/// Existing glances only. No vault values. No file bodies.
pub fn gather(place: &Place) -> Vec<Slice> {
    let mut out = Vec::new();
    if let Some(s) = tldr_slice(place) {
        out.push(s);
    }
    if let Some(s) = working_slice(place) {
        out.push(s);
    }
    if let Some(s) = last_slice(place) {
        out.push(s);
    }
    if let Some(s) = ask_slice(place) {
        out.push(s);
    }
    if let Some(s) = branch_slice(place) {
        out.push(s);
    }
    out
}

/// Fill the budget. A slice in `pointers` is an address only. Keep that
/// will not fit becomes a pointer, not a second page.
pub fn pack(slices: &[Slice], ranking: &Ranking) -> Packed {
    let by_id: std::collections::HashMap<&str, &Slice> =
        slices.iter().map(|s| (s.id.as_str(), s)).collect();
    let pointer_set: std::collections::HashSet<&str> =
        ranking.pointers.iter().map(String::as_str).collect();
    let keep_ids: Vec<String> = if ranking.keep.is_empty() {
        DEFAULT_ORDER
            .iter()
            .filter(|id| by_id.contains_key(**id))
            .map(|id| (*id).to_string())
            .collect()
    } else {
        ranking
            .keep
            .iter()
            .filter(|id| by_id.contains_key(id.as_str()))
            .cloned()
            .collect()
    };

    let mut sections = Vec::new();
    let mut overflow = Vec::new();
    let mut used = 0u64;
    for id in &keep_ids {
        let Some(slice) = by_id.get(id.as_str()) else {
            continue;
        };
        if pointer_set.contains(id.as_str()) {
            overflow.push((*slice).clone());
            continue;
        }
        let line = format!("{}: {}", slice.id, slice.body);
        let cost = crate::evict::estimate_tokens(&line);
        if used.saturating_add(cost) > BUDGET_TOKENS && !sections.is_empty() {
            overflow.push((*slice).clone());
            continue;
        }
        used = used.saturating_add(cost);
        sections.push((*slice).clone());
    }
    for id in &ranking.pointers {
        if keep_ids.iter().any(|k| k == id) {
            continue;
        }
        if let Some(slice) = by_id.get(id.as_str())
            && !overflow.iter().any(|s| s.id == slice.id)
        {
            overflow.push((*slice).clone());
        }
    }
    Packed { sections, overflow }
}

#[derive(Debug, Clone)]
pub struct Packed {
    pub sections: Vec<Slice>,
    pub overflow: Vec<Slice>,
}

impl Packed {
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        for s in &self.sections {
            out.push_str(&s.id);
            out.push_str(": ");
            out.push_str(&s.body);
            out.push('\n');
        }
        if !self.overflow.is_empty() {
            out.push_str("overflow:\n");
            for s in &self.overflow {
                out.push_str("- ");
                out.push_str(&s.id);
                out.push_str(" → ");
                out.push_str(&s.pointer);
                out.push('\n');
            }
        }
        out
    }
}

/// Menu line for the situation card. Fast is the first cheap-looking slug.
pub fn menu_line(models: &[String]) -> String {
    let powerful = models.first().map(String::as_str).unwrap_or("default");
    let fast = models
        .iter()
        .find(|m| looks_cheap(m))
        .or_else(|| models.get(1))
        .map(String::as_str)
        .unwrap_or(powerful);
    format!("fast={fast}  powerful={powerful}")
}

pub fn looks_cheap(model: &str) -> bool {
    let l = model.to_ascii_lowercase();
    ["flash", "mini", "haiku", "lite", "nano", "small"]
        .iter()
        .any(|k| l.contains(k))
}

/// Read the markdown file. Absent and unknown are not the same.
pub fn read_status(place: &Place) -> BriefRead {
    match record::read_text(&paths(place).0) {
        record::Read::Present(text) => BriefRead::Present(text),
        record::Read::Absent => BriefRead::Absent,
        record::Read::Unknown(why) => BriefRead::Unknown(why),
    }
}

fn write_packed(place: &Place, packed: &Packed) -> Result<()> {
    let (md, json) = paths(place);
    let markdown = packed.markdown();
    record::write_atomic(&md, markdown.as_bytes())?;
    let file = BriefFile {
        sections: packed
            .sections
            .iter()
            .map(|s| BriefSection {
                id: s.id.clone(),
                kind: s.kind.clone(),
                text: s.body.clone(),
            })
            .collect(),
        overflow: packed
            .overflow
            .iter()
            .map(|s| BriefPointer {
                id: s.id.clone(),
                pointer: s.pointer.clone(),
            })
            .collect(),
    };
    let body = serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".into());
    record::write_atomic(&json, body.as_bytes())?;
    Ok(())
}

fn tldr_slice(place: &Place) -> Option<Slice> {
    let page = notes::load(place, "root");
    if page.unread_reason().is_some() {
        return None;
    }
    let rendered = page.render();
    let body = tldr_glance(&rendered).or_else(|| {
        let open = page.open();
        if open.is_empty() {
            return None;
        }
        Some(
            open.iter()
                .take(2)
                .map(|i| glance(&i.text))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    if body.is_empty() {
        return None;
    }
    Some(Slice {
        id: "tldr".into(),
        kind: "notes".into(),
        body,
        pointer: "notes.md#tldr".into(),
    })
}

fn tldr_glance(text: &str) -> Option<String> {
    let start = text.find("<tldr>")?;
    let rest = &text[start + "<tldr>".len()..];
    let end = rest.find("</tldr>")?;
    let inner = rest[..end].trim();
    if inner.is_empty() {
        return None;
    }
    let line = inner
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let g = glance(&line);
    (!g.is_empty()).then_some(g)
}

fn working_slice(place: &Place) -> Option<Slice> {
    let agents = list_agents(place).ok()?;
    let kids: Vec<String> = agents
        .iter()
        .filter(|a| a.id.as_str() != "root" && !a.paused)
        .map(|a| {
            let title = if a.title.trim().is_empty() {
                a.name.clone()
            } else {
                a.title.clone()
            };
            glance(&format!("{} · {}", a.id.as_str(), title))
        })
        .filter(|s| !s.is_empty())
        .collect();
    if kids.is_empty() {
        return None;
    }
    let first = agents.iter().find(|a| a.id.as_str() != "root")?;
    Some(Slice {
        id: "working".into(),
        kind: "child".into(),
        body: kids.join("; "),
        pointer: format!("agents/{}", first.id.as_str()),
    })
}

fn last_slice(place: &Place) -> Option<Slice> {
    let path = place.agent_dir("root").join("transcript.jsonl");
    let events = load_transcript(&path).ok()?;
    let text = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::Assistant { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
        _ => None,
    })?;
    let body = glance(text);
    if body.is_empty() {
        return None;
    }
    Some(Slice {
        id: "last".into(),
        kind: "say".into(),
        body,
        pointer: "agents/root/transcript.jsonl".into(),
    })
}

fn ask_slice(place: &Place) -> Option<Slice> {
    let asks = waiting::asks(place, "root");
    let first = asks.first()?;
    let body = glance(&first.question);
    if body.is_empty() {
        return None;
    }
    Some(Slice {
        id: "ask".into(),
        kind: "wait".into(),
        body,
        pointer: "agents/root/waiting".into(),
    })
}

fn branch_slice(place: &Place) -> Option<Slice> {
    let out = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(place.path())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if b.is_empty() {
        return None;
    }
    Some(Slice {
        id: "branch".into(),
        kind: "git".into(),
        body: glance(&b),
        pointer: "HEAD".into(),
    })
}

fn glance(s: &str) -> String {
    if s.contains("op://") || s.contains("sk-") {
        return "(redacted)".into();
    }
    text::clip(s, GLANCE_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbos_core::{Agent, Event, append_event};

    fn place() -> (tempfile::TempDir, Place) {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::new(dir.path());
        std::fs::create_dir_all(place.agent_dir("root")).unwrap();
        (dir, place)
    }

    #[test]
    fn pack_overflows_a_slice_that_will_not_fit() {
        let huge = Slice {
            id: "tldr".into(),
            kind: "notes".into(),
            body: "x".repeat(10_000),
            pointer: "notes.md#tldr".into(),
        };
        let ask = Slice {
            id: "ask".into(),
            kind: "wait".into(),
            body: "Allow cargo publish?".into(),
            pointer: "agents/root/waiting".into(),
        };
        let packed = pack(
            &[huge, ask],
            &Ranking {
                keep: vec!["tldr".into(), "ask".into()],
                pointers: vec![],
            },
        );
        assert_eq!(packed.sections.len(), 1);
        assert_eq!(packed.sections[0].id, "tldr");
        assert_eq!(packed.overflow.len(), 1);
        assert_eq!(packed.overflow[0].id, "ask");
        let md = packed.markdown();
        assert!(md.contains("overflow:"));
        assert!(md.contains("ask → agents/root/waiting"));
        assert!(!md.contains("Allow cargo publish?"));
    }

    #[test]
    fn named_pointer_is_an_address_not_a_body() {
        let tldr = Slice {
            id: "tldr".into(),
            kind: "notes".into(),
            body: "voice live on the pod".into(),
            pointer: "notes.md#tldr".into(),
        };
        let ask = Slice {
            id: "ask".into(),
            kind: "wait".into(),
            body: "Allow cargo publish?".into(),
            pointer: "agents/root/waiting".into(),
        };
        let packed = pack(
            &[tldr, ask],
            &Ranking {
                keep: vec!["tldr".into(), "ask".into()],
                pointers: vec!["ask".into()],
            },
        );
        assert_eq!(packed.sections.len(), 1);
        assert_eq!(packed.overflow[0].id, "ask");
    }

    #[test]
    fn jev_false_deletes_the_brief() {
        let (_dir, place) = place();
        let packed = pack(
            &[Slice {
                id: "tldr".into(),
                kind: "notes".into(),
                body: "hello".into(),
                pointer: "notes.md#tldr".into(),
            }],
            &Ranking::default(),
        );
        write_packed(&place, &packed).unwrap();
        let (md, json) = paths(&place);
        assert!(md.exists());
        assert!(json.exists());
        delete(&place);
        assert!(!md.exists());
        assert!(!json.exists());
        assert_eq!(read_status(&place), BriefRead::Absent);
    }

    #[test]
    fn unknown_read_is_not_empty() {
        let (_dir, place) = place();
        assert_eq!(read_status(&place), BriefRead::Absent);
        match BriefRead::Unknown("eio".into()) {
            BriefRead::Unknown(_) => {}
            BriefRead::Present(_) | BriefRead::Absent => {
                panic!("unknown is not empty and not absent")
            }
        }
    }

    #[test]
    fn gather_takes_glances_and_redacts_vault() {
        let (_dir, place) = place();
        std::fs::write(
            place.arbos().join("notes.md"),
            "# Notes\n\n<tldr>\n- [Voice](docs/x) — live on the pod\n</tldr>\n\n- [ ] work\n",
        )
        .unwrap();
        std::fs::write(
            place.arbos().join("secrets.md"),
            "op://vault/item/password\n",
        )
        .unwrap();
        let mut child = Agent::root("run-tests");
        child.id = arbos_core::AgentId::new("run-tests");
        child.name = "run-tests".into();
        child.title = "cargo test".into();
        child.parent = Some(arbos_core::AgentId::new("root"));
        child
            .save(&place.agent_dir("run-tests"))
            .unwrap();
        append_event(
            &place.agent_dir("root").join("transcript.jsonl"),
            &Event::new(EventKind::Assistant {
                text: "All thirty tests pass.".into(),
                step: 1,
                reasoning_details: None,
            }),
        )
        .unwrap();
        waiting::write(
            &place,
            "root",
            &waiting::Waiting {
                kind: "ask".into(),
                id: "c1".into(),
                question: "Allow cargo publish?".into(),
                options: vec![],
                tool: String::new(),
                asked: "2026-01-01T00:00:00Z".into(),
            },
        )
        .unwrap();

        let slices = gather(&place);
        let ids: Vec<&str> = slices.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"tldr"), "{ids:?}");
        assert!(ids.contains(&"working"), "{ids:?}");
        assert!(ids.contains(&"last"), "{ids:?}");
        assert!(ids.contains(&"ask"), "{ids:?}");
        let tldr = slices.iter().find(|s| s.id == "tldr").unwrap();
        assert!(tldr.body.contains("Voice"), "{}", tldr.body);
        assert!(!tldr.body.contains("op://"));
        let last = slices.iter().find(|s| s.id == "last").unwrap();
        assert!(last.body.contains("thirty"));
        let joined = slices
            .iter()
            .map(|s| s.body.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains("op://vault"));
        assert!(
            !joined.contains(&"x".repeat(400)),
            "file bodies must not ride"
        );

        refresh(&place, &Ranking::default()).unwrap();
        let md = std::fs::read_to_string(paths(&place).0).unwrap();
        assert!(!md.contains("PROJECT IDENTITY"));
        assert!(md.contains("tldr:"));
    }

    #[test]
    fn vault_glance_is_redacted() {
        assert_eq!(glance("op://vault/item/password"), "(redacted)");
        assert_eq!(glance("sk-abc123"), "(redacted)");
        assert_eq!(glance("hello"), "hello");
    }

    #[test]
    fn menu_line_names_fast_and_powerful() {
        let line = menu_line(&[
            "anthropic/claude-opus-5".into(),
            "google/gemini-3.8-flash".into(),
        ]);
        assert!(line.contains("fast=google/gemini-3.8-flash"));
        assert!(line.contains("powerful=anthropic/claude-opus-5"));
    }
}
