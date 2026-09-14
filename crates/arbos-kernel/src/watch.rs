//! `changed` frames: the kernel tells attached clients which files under
//! `.arbos/` moved, so a phone that holds a small mirror (a plan, the
//! open transcript segment, `focus`) asks for exactly that with `tail` or
//! `read` instead of polling everything. The design's "stream" half of
//! remote reads.
//!
//! No inotify: the kernel already walks its agents every tick; once a
//! second it stats a fixed, small set of files per agent (its `agent.md`,
//! `notes.md` checklist, transcript) and the place's own (the project
//! store's `notes.md` page and context file among them), and reports size
//! changes and appearances/disappearances. A file
//! that changes and changes back within a second is missed; that is fine
//! for a view. Nothing is sent when nobody is attached.

use std::collections::HashMap;
use std::path::Path;

use arbos_core::wire::Frame;
use arbos_core::{Place, list_agents};

/// Per agent, relative to `agents/<id>/`.
const AGENT_FILES: &[&str] = &[
    "agent.md",
    "notes.md",
    "transcript.jsonl",
    "feedback.jsonl",
    "checkpoints.jsonl",
    "instructions.md",
    "status.toml",
];
/// At the place's root, relative to `.arbos/`. The project store's
/// status page and context file are here so a window redraws "Project"
/// when root rewrites them.
const PLACE_FILES: &[&str] = &[
    "focus",
    "user.md",
    "memory.md",
    "kernel.json",
    "project.toml",
    "notes.md",
    "archived.md",
    "docs/project-context.md",
];

/// What was seen last time: path → (size, mtime millis).
#[derive(Default)]
pub struct Watch {
    seen: HashMap<String, (u64, i64)>,
    primed: bool,
}

impl Watch {
    /// Stat the watched set and return one frame per change. The first
    /// call only records what is there (a client that just attached gets
    /// the files by `read`, not a flood of `created`).
    pub fn poll(&mut self, place: &Place) -> Vec<Frame> {
        let mut now: HashMap<String, (u64, i64)> = HashMap::new();
        let arbos = place.arbos();
        for name in PLACE_FILES {
            stat_into(&arbos.join(name), name, &mut now);
        }
        for agent in list_agents(place).unwrap_or_default() {
            let dir = place.agent_dir(agent.id.as_str());
            for name in AGENT_FILES {
                let rel = format!("agents/{}/{name}", agent.id.as_str());
                stat_into(&dir.join(name), &rel, &mut now);
            }
        }
        let mut out = Vec::new();
        if self.primed {
            for (path, (size, mtime)) in &now {
                match self.seen.get(path) {
                    None => out.push(Frame::Changed {
                        path: path.clone(),
                        kind: "created".into(),
                        size: *size,
                    }),
                    Some((s, m)) if s != size || m != mtime => out.push(Frame::Changed {
                        path: path.clone(),
                        kind: "modified".into(),
                        size: *size,
                    }),
                    Some(_) => {}
                }
            }
            for path in self.seen.keys() {
                if !now.contains_key(path) {
                    out.push(Frame::Changed {
                        path: path.clone(),
                        kind: "removed".into(),
                        size: 0,
                    });
                }
            }
            out.sort_by(|a, b| frame_path(a).cmp(frame_path(b)));
        }
        self.seen = now;
        self.primed = true;
        out
    }
}

fn frame_path(f: &Frame) -> &str {
    match f {
        Frame::Changed { path, .. } => path,
        _ => "",
    }
}

fn stat_into(path: &Path, rel: &str, into: &mut HashMap<String, (u64, i64)>) {
    if let Some(seen) = stat(path) {
        into.insert(rel.to_string(), seen);
    }
}

/// A file's (size, mtime millis), or None when it is not a file.
pub fn stat(path: &Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some((meta.len(), mtime))
}
