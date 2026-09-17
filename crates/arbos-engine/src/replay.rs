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
//! whoever asks next **among the agents the script never pins a line
//! to**. An agent with pinned lines anywhere in the script reads only
//! those: a script that pins root's lines and leaves a worker's unpinned
//! means the unpinned line for the worker, and root's extra step (after a
//! spawn result, on a done wake) must not take it first — which it did on
//! a loaded runner, and every such red read as a flake (2026-09-17).
//! Replies are consumed in file order within those rules. When they run
//! out, the call returns a short notice with no tool calls, which ends the
//! turn.

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
    /// How long the model "takes" before this reply, for tests of what
    /// lands during a model call (a stop, a steer). Cancellable like a
    /// real call. Default none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
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

    /// The `delay_ms` of the reply `next` would hand `agent`, without
    /// taking it.
    pub fn peek_delay(&self, agent: &str) -> Option<u64> {
        let used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        self.replies
            .iter()
            .enumerate()
            .find(|(i, r)| !used[*i] && self.serves(r, agent))
            .and_then(|(_, r)| r.delay_ms)
    }

    /// Whether the script pins any line (used or not) to `agent`: such an
    /// agent reads only its own lines, never an unpinned one.
    fn is_pinned(&self, agent: &str) -> bool {
        self.replies
            .iter()
            .any(|r| r.agent.as_deref() == Some(agent))
    }

    /// Whether an unused reply `r` may go to `agent`.
    fn serves(&self, r: &Reply, agent: &str) -> bool {
        match r.agent.as_deref() {
            Some(a) => a == agent,
            None => !self.is_pinned(agent),
        }
    }

    /// Whether the script pins at least one unused line to `agent`. A side
    /// call that is optional (a chat's title) is made under replay only
    /// when the script meant it, so it never takes a turn's line.
    pub fn has_line_for(&self, agent: &str) -> bool {
        let used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        self.replies
            .iter()
            .enumerate()
            .any(|(i, r)| !used[i] && r.agent.as_deref() == Some(agent))
    }

    /// The next reply for `agent`, or the end-of-script notice.
    pub fn next(&self, agent: &str) -> Completion {
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        let pick = self
            .replies
            .iter()
            .enumerate()
            .position(|(i, r)| !used[i] && self.serves(r, agent));
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

#[cfg(test)]
mod pinned_tests {
    use super::*;

    fn script(name: &str, lines: &str) -> Replay {
        let dir = std::env::temp_dir().join(format!("arbos-replay-pinned-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.jsonl"));
        std::fs::write(&path, lines).unwrap();
        Replay::load(&path).unwrap()
    }

    /// The race behind the #436 red: root's pinned lines run out, root
    /// takes another step, and the worker's unpinned line was first in
    /// file order. A pinned agent now reads only its own lines.
    #[test]
    fn a_pinned_agent_never_takes_an_unpinned_line_meant_for_another() {
        let r = script(
            "pinned",
            concat!(
                "{\"agent\":\"root\",\"content\":\"spawning\"}\n",
                "{\"content\":\"the codeword is marimba\"}\n",
            ),
        );
        assert_eq!(r.next("root").content, "spawning");
        // Root's extra step: exhaustion, not the worker's line.
        assert!(
            r.next("root").content.starts_with("(replay:"),
            "root took the worker's line"
        );
        assert_eq!(r.next("w1").content, "the codeword is marimba");
        assert!(r.peek_delay("root").is_none());
    }

    /// Unpinned lines still go to whoever asks among unpinned agents, in
    /// file order — the single-agent scripts keep working.
    #[test]
    fn unpinned_lines_serve_unpinned_agents_in_order() {
        let r = script(
            "unpinned",
            concat!("{\"content\":\"one\"}\n", "{\"content\":\"two\"}\n",),
        );
        assert_eq!(r.next("a").content, "one");
        assert_eq!(r.next("b").content, "two");
        assert!(r.next("a").content.starts_with("(replay:"));
    }
}
