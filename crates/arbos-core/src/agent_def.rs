//! Custom agent definitions: `.arbos/agents-defs/<name>.md`.
//!
//! A definition describes a *kind* of agent — its model, its allowlist,
//! whether it may write, and its standing instructions — so a parent can
//! `spawn kind=<name>` instead of spelling those out each time. The same
//! front-matter shape as Cursor's `.cursor/agents/*.md`, which is read too.

use std::path::{Path, PathBuf};

use crate::place::Place;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentDef {
    /// The kind name, what `spawn kind=` takes. The file stem by default.
    pub name: String,
    /// One line for the parent's prompt: when to use this kind.
    pub description: String,
    /// `inherit` or a model id. Empty means the spawn call decides.
    pub model: String,
    /// Tool names. Empty means the parent's list.
    pub allowlist: Vec<String>,
    pub readonly: bool,
    pub cwd: Option<PathBuf>,
    /// Standing instructions for the child: the text after the front matter.
    pub body: String,
    pub path: PathBuf,
}

/// Directories searched, in order. The first file to claim a name wins.
pub fn def_dirs(place: &Place) -> Vec<PathBuf> {
    vec![
        place.arbos().join("agents-defs"),
        place.path.join(".cursor").join("agents"),
    ]
}

/// Every definition in the place, sorted by name, one per name.
pub fn load_defs(place: &Place) -> Vec<AgentDef> {
    let mut defs: Vec<AgentDef> = Vec::new();
    for dir in def_dirs(place) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(def) = AgentDef::parse(&path, &text) else {
                continue;
            };
            if defs.iter().any(|d| d.name == def.name) {
                continue;
            }
            defs.push(def);
        }
    }
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

pub fn find_def(place: &Place, name: &str) -> Option<AgentDef> {
    let q = name.trim();
    load_defs(place).into_iter().find(|d| d.name == q)
}

/// A kind name is a file stem: letters, digits, `-`, `_`, `.`; no slashes.
pub fn valid_kind(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl AgentDef {
    /// Front matter between `---` lines, then the body. A file without
    /// front matter is all body. Returns `None` when the name is unusable.
    pub fn parse(path: &Path, text: &str) -> Option<Self> {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let mut def = AgentDef {
            name: stem.to_string(),
            path: path.to_path_buf(),
            ..Default::default()
        };
        let (front, body) = split_front_matter(text);
        for raw in front.lines() {
            let line = raw.trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().trim_matches(['"', '\'']);
            match key.as_str() {
                "name" if !value.is_empty() => def.name = value.to_string(),
                "description" => def.description = value.to_string(),
                "model" => def.model = value.to_string(),
                "allowlist" | "tools" => {
                    def.allowlist = value
                        .trim_matches(['[', ']'])
                        .split(',')
                        .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "readonly" => def.readonly = matches!(value, "true" | "yes" | "1"),
                "cwd" if !value.is_empty() => def.cwd = Some(PathBuf::from(value)),
                _ => {}
            }
        }
        def.body = body.trim().to_string();
        if !valid_kind(&def.name) {
            return None;
        }
        Some(def)
    }

    /// `name — description` for the roster of kinds in a prompt.
    pub fn roster_line(&self) -> String {
        let mut line = self.name.clone();
        if !self.description.is_empty() {
            line.push_str(" — ");
            line.push_str(&self.description);
        }
        let mut flags = Vec::new();
        if self.readonly {
            flags.push("readonly".to_string());
        }
        if !self.model.is_empty() && self.model != "inherit" {
            flags.push(format!("model {}", self.model));
        }
        if !flags.is_empty() {
            line.push_str(&format!(" ({})", flags.join(", ")));
        }
        line
    }
}

/// `(front, body)`. Front is empty when the text does not open with `---`
/// or the closing `---` is missing.
fn split_front_matter(text: &str) -> (&str, &str) {
    let t = text.trim_start_matches('\u{feff}');
    let Some(rest) = t.strip_prefix("---") else {
        return ("", t);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return ("", t);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (&rest[..offset], &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    ("", t)
}
