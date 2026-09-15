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
    /// `role:` in the front matter. `worker` (the default for any child
    /// without a kind) keeps the child to its own task; `none` gives a kind
    /// no role line at all, e.g. for a kind that is meant to delegate.
    pub role: Option<String>,
    /// An outside program that speaks the Agent Client Protocol runs this
    /// kind's turns (P-14): the command line, started in the agent's cwd,
    /// e.g. `npx -y @zed-industries/claude-code-acp`.
    pub acp: Option<String>,
    /// `inline: true`: a short-lived helper (Cursor's typed subagent) —
    /// `spawn kind=<name>` waits for its result unless told `wait:false`.
    pub inline: bool,
    /// Standing instructions for the child: the text after the front matter.
    pub body: String,
    pub path: PathBuf,
}

/// Directories searched, in order. The first file to claim a name wins:
/// the place's own kinds, then Cursor's, then the host's
/// (`~/.config/arbos/agents-defs/`, the same kinds in every place).
pub fn def_dirs(place: &Place) -> Vec<PathBuf> {
    vec![
        place.arbos().join("agents-defs"),
        place.path.join(".cursor").join("agents"),
        host_def_dir(),
    ]
}

/// The host's kinds: `~/.config/arbos/agents-defs/`.
pub fn host_def_dir() -> PathBuf {
    crate::host_dir().join("agents-defs")
}

/// The kinds every place has without a file (Cursor's typed helpers and
/// the area coordinator). A file of the same name in any definition
/// directory replaces the built-in.
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "explore",
        r#"---
description: Read-only codebase search that answers one question with path:line cites; runs inline and returns.
tools: ls, read, find, grep, search, fetch, status
readonly: true
inline: true
role: worker
---
You are an explore helper. Answer the question in your brief from the files: find, grep, read, and give the answer in a few lines with `path:line` cites for every claim. Say plainly what you did not find. Read only: never write, never run a build, never spawn. Your final words are the answer your parent reads.
"#,
    ),
    (
        "computer-use",
        r#"---
description: Drives a page or the machine's screen to test or demonstrate; returns what it saw, with stills under media/<topic>/.
tools: browser, screenshot, record, bash, read, write, ls, find, grep, status
inline: true
role: worker
---
You are a computer-use helper. Drive the page or app your brief names with `browser`, `screenshot`, and `record`; look at the pixels after every step instead of assuming; save each still to the exact `.arbos/media/<topic>/` path the brief gives and check the file exists before you name it. Never edit code. Your final words say what you saw, what failed and at which step, and the path of every still.
"#,
    ),
    (
        "video-review",
        r#"---
description: Reviews a recording frame by frame (ffmpeg) and says whether it shows what was claimed.
tools: bash, read, ls, find, status
readonly: true
inline: true
role: worker
---
You are a video-review helper. For the recording your brief names: extract frames with `ffmpeg -i <video> -vf fps=1 <tmpdir>/f%03d.png` (one per second; `fps=2` for a short clip), read them in order, and answer the brief's questions — what the video shows, whether it matches the claim, and the second at which anything goes wrong. Frames go in a temp folder, never into the project. Your final words are the review.
"#,
    ),
    (
        "coordinator",
        r#"---
description: An area coordinator: splits its ask into workstreams, one worker each, handles their dones itself, and returns one combined result to its parent.
role: coordinator
---
You coordinate one area for your parent. Split the ask into independent workstreams, spawn one worker per stream in one response, and handle their `[done]` messages yourself: verify what they claim, chain follow-ups, steer. Your parent hears from you once, with one combined result as your final words, when the area is done or blocked — interim dones stay with you. Keep the area's status in your own checklist (`plan`); the project page is your parent's.
"#,
    ),
];

/// The built-in kinds, parsed.
pub fn builtin_defs() -> Vec<AgentDef> {
    BUILTIN
        .iter()
        .filter_map(|(name, text)| AgentDef::parse(Path::new(&format!("builtin/{name}.md")), text))
        .collect()
}

/// Every definition in the place, sorted by name, one per name: the
/// place's files, Cursor's, the host's, then the built-ins a file did not
/// replace.
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
    for def in builtin_defs() {
        if !defs.iter().any(|d| d.name == def.name) {
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
                "inline" => def.inline = matches!(value, "true" | "yes" | "1"),
                "role" if !value.is_empty() => def.role = Some(value.to_ascii_lowercase()),
                "cwd" if !value.is_empty() => def.cwd = Some(PathBuf::from(value)),
                "acp" | "acp_command" if !value.is_empty() => def.acp = Some(value.to_string()),
                _ => {}
            }
        }
        def.body = body.trim().to_string();
        if !valid_kind(&def.name) {
            return None;
        }
        Some(def)
    }

    /// One of [`BUILTIN`], not a file of the place, Cursor, or the host.
    pub fn is_builtin(&self) -> bool {
        self.path.starts_with("builtin")
    }

    /// `name — description` for the roster of kinds in a prompt.
    pub fn roster_line(&self) -> String {
        let mut line = self.name.clone();
        let mut flags = Vec::new();
        if self.readonly {
            flags.push("readonly");
        }
        if self.acp.is_some() {
            flags.push("acp");
        }
        if self.inline {
            flags.push("inline");
        }
        if self.role.as_deref() == Some(crate::project::COORDINATOR) {
            flags.push("coordinator");
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

#[cfg(test)]
mod host_dir_tests {
    use super::*;

    #[test]
    fn the_built_in_helpers_are_typed_read_only_inline_kinds_and_a_file_replaces_one() {
        let defs = builtin_defs();
        let explore = defs.iter().find(|d| d.name == "explore").unwrap();
        assert!(explore.inline && explore.readonly && explore.is_builtin());
        assert!(explore.allowlist.iter().any(|t| t == "grep"));
        assert!(
            !explore
                .allowlist
                .iter()
                .any(|t| t == "write" || t == "spawn")
        );
        assert_eq!(explore.role.as_deref(), Some("worker"));
        assert!(
            explore.roster_line().contains("readonly, inline"),
            "{}",
            explore.roster_line()
        );
        let cu = defs.iter().find(|d| d.name == "computer-use").unwrap();
        assert!(cu.inline && !cu.readonly);
        assert!(cu.allowlist.iter().any(|t| t == "browser"));
        let coord = defs.iter().find(|d| d.name == "coordinator").unwrap();
        assert_eq!(coord.role.as_deref(), Some(crate::project::COORDINATOR));
        assert!(!coord.inline);
        assert!(coord.roster_line().contains("coordinator"));

        let root = std::env::temp_dir().join(format!(
            "arbos-agent-def-builtin-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(root.join(".arbos/agents-defs")).unwrap();
        std::fs::write(
            root.join(".arbos/agents-defs/explore.md"),
            "---\ndescription: my own explore\ntools: read\n---\nMine.\n",
        )
        .unwrap();
        let place = Place::new(&root);
        let mine = find_def(&place, "explore").unwrap();
        assert_eq!(mine.description, "my own explore");
        assert!(!mine.is_builtin() && !mine.inline);
        assert!(find_def(&place, "video-review").unwrap().is_builtin());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_kinds_load_after_the_places_and_a_place_kind_of_the_same_name_wins() {
        let root = std::env::temp_dir().join(format!(
            "arbos-agent-def-host-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let place_dir = root.join("place");
        let xdg = root.join("xdg");
        std::fs::create_dir_all(place_dir.join(".arbos/agents-defs")).unwrap();
        std::fs::create_dir_all(xdg.join("arbos/agents-defs")).unwrap();
        // The test owns XDG_CONFIG_HOME for its length; tests in this
        // crate that read host_dir() run under the same variable, so it is
        // set to a folder with nothing else in it.
        let saved = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &xdg) };
        std::fs::write(
            xdg.join("arbos/agents-defs/reviewer.md"),
            "---\nname: reviewer\ndescription: host reviewer\nreadonly: true\n---\nReview.\n",
        )
        .unwrap();
        std::fs::write(
            xdg.join("arbos/agents-defs/tester.md"),
            "---\nname: tester\ndescription: host tester\n---\nTest.\n",
        )
        .unwrap();
        std::fs::write(
            place_dir.join(".arbos/agents-defs/tester.md"),
            "---\nname: tester\ndescription: place tester\n---\nTest here.\n",
        )
        .unwrap();
        let place = Place::new(&place_dir);
        let defs = load_defs(&place);
        let names: Vec<&str> = defs
            .iter()
            .filter(|d| !d.is_builtin())
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(names, vec!["reviewer", "tester"], "{names:?}");
        // The built-in helpers ride along, after the files; a file of the
        // same name would replace one.
        let builtin: Vec<&str> = defs
            .iter()
            .filter(|d| d.is_builtin())
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(
            builtin,
            vec!["computer-use", "coordinator", "explore", "video-review"]
        );
        let tester = defs.iter().find(|d| d.name == "tester").unwrap();
        assert_eq!(tester.description, "place tester", "the place's file wins");
        assert!(tester.path.starts_with(&place_dir));
        let reviewer = defs.iter().find(|d| d.name == "reviewer").unwrap();
        assert!(reviewer.readonly && reviewer.path.starts_with(&xdg));
        match saved {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
