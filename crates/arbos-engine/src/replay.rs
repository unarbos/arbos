//! A model with a script. `ARBOS_PROVIDER=replay ARBOS_REPLIES=<file>` makes
//! every provider call return the next line of the file instead of going
//! to the network, so a rollout runs the same way twice and a test needs no
//! key. The file is what `arbos-kernel rollout export` writes and what a
//! hand can author: one JSON object per line.
//!
//! ```json
//! {"content": "Let me look.", "calls": [{"name": "bash", "arguments": {"command": "ls"}}]}
//! {"content": "Two files.", "agent": "root"}
//! ```
//!
//! `agent` pins a reply to one agent's calls; a reply without it goes to
//! whoever asks next. Replies are consumed in file order within those
//! rules. When they run out, the call returns a short notice with no tool
//! calls, which ends the turn.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::{Completion, ToolCall};

/// Env var naming the provider; `replay` selects this module.
pub const PROVIDER_ENV: &str = "ARBOS_PROVIDER";
/// Env var with the replies file for `replay`.
pub const REPLIES_ENV: &str = "ARBOS_REPLIES";

/// One scripted model step.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<ReplyCall>,
    /// Only this agent's calls take the line. None = anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Where the line came from (`trace`, `transcript`, `hand`), for the eye.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What this reply "cost" in US dollars, for tests of spend accounting.
    /// Default nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// A thought to stream before the content, for tests of thinking
    /// records. Default none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplyCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    /// Kept from the recording so the transcript's `call_id`s line up;
    /// made up when missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The script and how far each reader got.
#[derive(Debug)]
pub struct Replay {
    pub path: PathBuf,
    replies: Vec<Reply>,
    /// Indices already handed out.
    used: Mutex<Vec<bool>>,
    /// Calls served so far, for made-up ids and the summary.
    served: Mutex<u64>,
}

impl Replay {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read replies {}", path.display()))?;
        let mut replies = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let r: Reply = serde_json::from_str(line)
                .with_context(|| format!("{}:{}: bad reply line", path.display(), i + 1))?;
            replies.push(r);
        }
        let n = replies.len();
        Ok(Self {
            path: path.to_path_buf(),
            replies,
            used: Mutex::new(vec![false; n]),
            served: Mutex::new(0),
        })
    }

    pub fn len(&self) -> usize {
        self.replies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.replies.is_empty()
    }

    /// Lines nobody has taken yet.
    pub fn remaining(&self) -> usize {
        self.used
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|u| !**u)
            .count()
    }

    /// The next reply for `agent`, or the end-of-script notice.
    pub fn next(&self, agent: &str) -> Completion {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        let pick = self
            .replies
            .iter()
            .enumerate()
            .position(|(i, r)| !used[i] && r.agent.as_deref().is_none_or(|a| a == agent));
        let Some(ix) = pick else {
            return Completion {
                content: "(replay: no more scripted replies)".into(),
                ..Completion::default()
            };
        };
        used[ix] = true;
        drop(used);
        let reply = &self.replies[ix];
        let mut served = self.served.lock().unwrap_or_else(|p| p.into_inner());
        let calls = reply
            .calls
            .iter()
            .map(|c| {
                *served += 1;
                ToolCall {
                    id: c
                        .id
                        .clone()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("replay_{served}")),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                }
            })
            .collect();
        Completion {
            content: reply.content.clone(),
            calls,
            usage: Some((0, 0)),
            cost: Some(reply.cost.unwrap_or(0.0)),
            cached: None,
            thinking: reply.thinking.clone(),
            reasoning_details: Vec::new(),
        }
    }
}

/// The process's replay script, read once from the environment. `None`
/// when `ARBOS_PROVIDER` is not `replay`. A named file that does not parse
/// is an error at first use, not a silent fall back to the network.
///
/// One instance per process, whoever asks first: two turns starting at
/// once (a parent and its child continued after a restart) each loaded
/// their own copy, one of them not the one kept, and the third caller
/// then took a line the first had already used — the parent's "restarted,
/// still waiting" said twice and swallowed as a repeat (CI, one run in
/// a few). The load happens inside the once-cell.
pub fn current() -> Result<Option<Arc<Replay>>> {
    let loaded: &Result<Option<Arc<Replay>>, String> = LOADED.get_or_init(|| {
        let selected = std::env::var(PROVIDER_ENV)
            .map(|p| p.trim().eq_ignore_ascii_case("replay"))
            .unwrap_or(false);
        if !selected {
            return Ok(None);
        }
        let path = std::env::var(REPLIES_ENV)
            .ok()
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| format!("{PROVIDER_ENV}=replay needs {REPLIES_ENV}=<replies.jsonl>"))?;
        let replay = Replay::load(Path::new(&path)).map_err(|e| format!("{e:#}"))?;
        Ok(Some(Arc::new(replay)))
    });
    match loaded {
        Ok(r) => Ok(r.clone()),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

static LOADED: OnceLock<Result<Option<Arc<Replay>>, String>> = OnceLock::new();

/// Select the replay provider for this process (what `serve --provider
/// replay --replies FILE` does before anything reads the environment).
pub fn select(path: &Path) {
    // SAFETY: called from `main` before the runtime and its threads start.
    unsafe {
        std::env::set_var(PROVIDER_ENV, "replay");
        std::env::set_var(REPLIES_ENV, path);
    }
}
