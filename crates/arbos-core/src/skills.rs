//! Skills: reusable instructions a user or agent invokes as `/name args`.
//!
//! A skill is `<dir>/<name>/SKILL.md` or `<dir>/<name>.md` under one of the
//! skill folders of a place (or the user's config). Front matter may give
//! `name` and `description`; the body is what the model reads. `$ARGUMENTS`
//! (and `$1`…`$9`) in the body take the words after the slash command.

use std::path::{Path, PathBuf};

use crate::place::Place;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub body: String,
}

/// Where skills live, in priority order. Place folders first (Arbos, then
/// the `.agents` / `.cursor` conventions, then a bare `skills/`), prompt
/// templates next, the user's own last.
pub fn skill_dirs(place: &Place) -> Vec<PathBuf> {
    let mut dirs = vec![
        place.arbos().join("skills"),
        place.path.join(".agents").join("skills"),
        place.path.join(".cursor").join("skills"),
        place.path.join("skills"),
        place.arbos().join("prompts"),
    ];
    if let Some(cfg) = config_dir() {
        dirs.push(cfg.join("skills"));
        dirs.push(cfg.join("prompts"));
    }
    dirs
}

fn config_dir() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|b| !b.is_empty()) {
        return Some(PathBuf::from(base).join("arbos"));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("arbos"))
}

/// Every skill reachable from the place, one per name (first folder wins),
/// sorted by name.
pub fn load_skills(place: &Place) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    for dir in skill_dirs(place) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file_name.starts_with('.') || file_name == "node_modules" {
                continue;
            }
            let file = if path.is_dir() {
                let f = path.join("SKILL.md");
                if !f.is_file() {
                    continue;
                }
                f
            } else if path.extension().and_then(|e| e.to_str()) == Some("md")
                && file_name != "SKILL.md"
                && file_name != "README.md"
            {
                path.clone()
            } else {
                continue;
            };
            let Some(skill) = Skill::load(&file) else {
                continue;
            };
            if out.iter().any(|s| s.name.eq_ignore_ascii_case(&skill.name)) {
                continue;
            }
            out.push(skill);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn find_skill(place: &Place, name: &str) -> Option<Skill> {
    let q = name.trim().trim_start_matches('/');
    load_skills(place)
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(q))
}

/// `/name rest` → `(name, rest)` when the text opens with a slash command.
pub fn split_slash(text: &str) -> Option<(&str, &str)> {
    let t = text.trim_start();
    let rest = t.strip_prefix('/')?;
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return None;
    }
    Some((name, rest[end..].trim()))
}

/// A user message that starts with `/name` for a skill here → the skill
/// and its arguments. Anything else is `None`: the text stands as written.
pub fn slash_skill(place: &Place, text: &str) -> Option<(Skill, String)> {
    let (name, args) = split_slash(text)?;
    let skill = find_skill(place, name)?;
    Some((skill, args.to_string()))
}

impl Skill {
    pub fn load(file: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(file).ok()?;
        let default_name = if file.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            file.parent()?.file_name()?.to_str()?.to_string()
        } else {
            file.file_stem()?.to_str()?.to_string()
        };
        let (front, body) = split_front_matter(&text);
        let mut name = default_name;
        let mut description = String::new();
        for line in front.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let v = v.trim().trim_matches(['"', '\'']);
            match k.trim().to_ascii_lowercase().as_str() {
                "name" if !v.is_empty() => name = v.to_string(),
                "description" => description = v.to_string(),
                _ => {}
            }
        }
        if name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Skill {
            name,
            description,
            path: file.to_path_buf(),
            body: body.trim().to_string(),
        })
    }

    /// The body with the arguments filled in. `$ARGUMENTS` takes them all;
    /// `$1`…`$9` take one word each. A body that names neither gets an
    /// `Arguments:` line appended when there are any.
    pub fn render(&self, args: &str) -> String {
        let args = args.trim();
        let words: Vec<&str> = args.split_whitespace().collect();
        let mut out = self.body.replace("$ARGUMENTS", args);
        let mut used_positional = false;
        for (i, w) in words.iter().enumerate().take(9) {
            let key = format!("${}", i + 1);
            if out.contains(&key) {
                used_positional = true;
                out = out.replace(&key, w);
            }
        }
        if !args.is_empty() && !self.body.contains("$ARGUMENTS") && !used_positional {
            out.push_str("\n\nArguments: ");
            out.push_str(args);
        }
        out
    }

    /// `name — description` for a roster.
    pub fn roster_line(&self) -> String {
        self.name.clone()
    }
}

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
