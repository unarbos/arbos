//! Pull requests the agents here opened: `.arbos/prs.jsonl`, one record per
//! PR, place-wide. The kernel appends when a `gh pr create` succeeds; the
//! desktop's "PRs N" pill and the prompt's `<<prs>>` roster read it.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::place::Place;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRec {
    /// Unix millis when the kernel saw it.
    pub ts: i64,
    /// The agent whose command opened it.
    pub agent: String,
    pub url: String,
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    /// The head branch, when the command said (`--head x` or the current
    /// branch is unknown to the kernel → empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
}

pub fn prs_path(place: &Place) -> PathBuf {
    place.arbos().join("prs.jsonl")
}

pub fn load_prs(place: &Place) -> Vec<PrRec> {
    load_from(&prs_path(place))
}

fn load_from(path: &Path) -> Vec<PrRec> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Append unless the URL is already recorded. Returns whether it was new.
pub fn record_pr(place: &Place, rec: &PrRec) -> Result<bool> {
    let path = prs_path(place);
    if load_from(&path).iter().any(|r| r.url == rec.url) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{}", serde_json::to_string(rec)?)?;
    Ok(true)
}

/// GitHub pull-request URLs in free text, normalised to
/// `https://github.com/<owner>/<repo>/pull/<n>`, in order, de-duplicated.
pub fn pr_urls(text: &str) -> Vec<(String, String, u64)> {
    let mut out: Vec<(String, String, u64)> = Vec::new();
    for word in text.split(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '`')
    }) {
        let word = word.trim_end_matches(['.', ',', ';', ':']);
        let Some(rest) = word
            .strip_prefix("https://github.com/")
            .or_else(|| word.strip_prefix("http://github.com/"))
        else {
            continue;
        };
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 4 && parts[2] == "pull" {
            let Ok(number) = parts[3].parse::<u64>() else {
                continue;
            };
            let repo = format!("{}/{}", parts[0], parts[1]);
            let url = format!("https://github.com/{repo}/pull/{number}");
            if !out.iter().any(|(u, _, _)| *u == url) {
                out.push((url, repo, number));
            }
        }
    }
    out
}

/// Whether a shell command opens a pull request: `gh pr create` in any
/// position of a pipeline or `&&` chain. `gh pr view` / `list` do not.
pub fn opens_pr(command: &str) -> bool {
    command.split(['\n', ';', '|', '&']).any(|seg| {
        let words: Vec<&str> = seg.split_whitespace().collect();
        words
            .windows(3)
            .any(|w| w[0].ends_with("gh") && w[1] == "pr" && w[2] == "create")
    })
}

/// `--head x` / `-H x` / `--head=x` from a `gh pr create` command.
pub fn head_branch(command: &str) -> String {
    let words: Vec<&str> = command.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if let Some(v) = w.strip_prefix("--head=") {
            return v.trim_matches(['"', '\'']).to_string();
        }
        if (*w == "--head" || *w == "-H") && i + 1 < words.len() {
            return words[i + 1].trim_matches(['"', '\'']).to_string();
        }
    }
    String::new()
}

/// The PRs opened by `agent` and every agent under it.
pub fn prs_of_tree(prs: &[PrRec], agent: &str, agents: &[crate::Agent]) -> Vec<PrRec> {
    prs.iter()
        .filter(|p| p.agent == agent || under(&p.agent, agent, agents))
        .cloned()
        .collect()
}

fn under(id: &str, ancestor: &str, agents: &[crate::Agent]) -> bool {
    let mut cur = agents
        .iter()
        .find(|a| a.id.as_str() == id)
        .and_then(|a| a.parent.clone());
    let mut hops = 0;
    while let Some(p) = cur {
        if p.as_str() == ancestor {
            return true;
        }
        hops += 1;
        if hops > 32 {
            break;
        }
        cur = agents
            .iter()
            .find(|a| a.id == p)
            .and_then(|a| a.parent.clone());
    }
    false
}
