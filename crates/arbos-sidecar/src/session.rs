//! One persistent agent session per connection (spec §13.2). The record lives next to the mirror
//! and survives restarts; every command-waker turn resumes it, a failed resume falls back to a
//! new session once, and rotation hands the chats' briefs into the new session.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::WakerConfig;
use crate::mirror::{now_ms, Mirror};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub generation: u32,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub last_used: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_from: Option<String>,
    /// Set when the next wake must start a new session (rotation requested / resume failed).
    #[serde(default)]
    pub rotate_pending: bool,
}

/// How the next turn is launched.
#[derive(Debug, Clone, PartialEq)]
pub enum Attempt {
    New { handoff: bool },
    Resume,
}

/// The two argv templates for one agent kind. `{session}` is the id, `{prompt}`/`{json}`/`{chat}`
/// as in the plain command waker. `capture_codex_thread` reads the id from `--json` output.
#[derive(Debug, Clone)]
pub struct Templates {
    pub new: Vec<String>,
    pub resume: Vec<String>,
    pub capture_codex_thread: bool,
}

fn v(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Presets for the agents people run (spec §13.2; cursor and hermes unverified).
pub fn preset(agent: &str) -> Option<Templates> {
    Some(match agent {
        "claude" => Templates {
            new: v(&[
                "claude",
                "-p",
                "--output-format",
                "text",
                "--session-id",
                "{session}",
                "{prompt}",
            ]),
            resume: v(&[
                "claude",
                "-p",
                "--output-format",
                "text",
                "--resume",
                "{session}",
                "{prompt}",
            ]),
            capture_codex_thread: false,
        },
        "codex" => Templates {
            new: v(&[
                "codex",
                "exec",
                "--json",
                "--skip-git-repo-check",
                "{prompt}",
            ]),
            resume: v(&[
                "codex",
                "exec",
                "resume",
                "{session}",
                "--json",
                "--skip-git-repo-check",
                "{prompt}",
            ]),
            capture_codex_thread: true,
        },
        "cursor" => Templates {
            new: v(&["cursor-agent", "-p", "--output-format", "text", "{prompt}"]),
            resume: v(&[
                "cursor-agent",
                "-p",
                "--output-format",
                "text",
                "--resume",
                "{session}",
                "{prompt}",
            ]),
            capture_codex_thread: false,
        },
        "hermes" => Templates {
            new: v(&["hermes", "chat", "-p", "{prompt}"]),
            resume: v(&["hermes", "chat", "--resume", "{session}", "-p", "{prompt}"]),
            capture_codex_thread: false,
        },
        _ => return None,
    })
}

/// Templates from the config: preset by `agent`, else `command_new`/`command_resume`, else the
/// plain `command` for both (a stateless agent: every wake is "new").
pub fn templates(cfg: &WakerConfig) -> Option<Templates> {
    if let Some(a) = cfg.agent.as_deref() {
        if let Some(t) = preset(a) {
            return Some(t);
        }
    }
    match (&cfg.command_new, &cfg.command_resume) {
        (Some(n), Some(r)) => Some(Templates {
            new: n.clone(),
            resume: r.clone(),
            capture_codex_thread: false,
        }),
        _ => cfg.command.clone().map(|c| Templates {
            new: c.clone(),
            resume: c,
            capture_codex_thread: false,
        }),
    }
}

/// Does this stderr/stdout say the session could not be resumed?
pub fn looks_like_resume_failure(out: &str, pattern: &str) -> bool {
    let lower = out.to_lowercase();
    pattern
        .split('|')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .any(|p| lower.contains(&p))
}

/// Does the agent's output say its context is full?
pub fn looks_like_context_full(out: &str, pattern: &str) -> bool {
    looks_like_resume_failure(out, pattern)
}

/// `codex exec --json` prints `{"type":"thread.started","thread_id":"…"}` first.
pub fn codex_thread_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["type"] == "thread.started")
        .and_then(|v| v["thread_id"].as_str().map(str::to_owned))
}

pub fn new_id() -> String {
    // UUID v4 from the ULID generator's randomness: 32 hex digits in 8-4-4-4-12 form.
    let raw = format!("{:0>32}", hex_lower(&arbos_rand(16)));
    format!(
        "{}-{}-4{}-{}{}-{}",
        &raw[0..8],
        &raw[8..12],
        &raw[13..16],
        ["8", "9", "a", "b"][(raw.as_bytes()[16] as usize) % 4],
        &raw[17..20],
        &raw[20..32]
    )
}

fn arbos_rand(n: usize) -> Vec<u8> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(now_ms() as u128 ^ (out.len() as u128) << 64);
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

fn hex_lower(b: &[u8]) -> String {
    hex::encode(b)
}

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    mirror: Mirror,
    max_turns: u32,
}

impl Store {
    pub fn new(mirror: Mirror, max_turns: u32) -> Self {
        Self {
            path: mirror.paths.data_dir.join("session.json"),
            mirror,
            max_turns,
        }
    }

    pub fn max_turns(&self) -> u32 {
        self.max_turns
    }

    pub fn load(&self) -> Session {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, s: &Session) -> Result<()> {
        std::fs::create_dir_all(self.mirror.paths.data_dir.as_path())?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Decide how the next turn starts. A session that never ran, was asked to rotate, or hit
    /// the turn cap starts new; everything else resumes.
    pub fn plan(&self, cfg: &WakerConfig, agent: &str) -> (Session, Attempt) {
        let mut s = self.load();
        if s.agent != agent {
            s = Session {
                agent: agent.to_owned(),
                ..Default::default()
            };
        }
        let rotate = s.rotate_pending || s.turns >= cfg.max_turns_per_session.max(1);
        if s.id.is_empty() {
            (s, Attempt::New { handoff: false })
        } else if rotate {
            (s, Attempt::New { handoff: true })
        } else {
            (s, Attempt::Resume)
        }
    }

    /// Start a new session record (keeps the old id as `handoff_from`).
    pub fn begin_new(&self, s: &Session, id: String) -> Result<Session> {
        let n = Session {
            id,
            agent: s.agent.clone(),
            generation: s.generation + 1,
            created: now_ms(),
            turns: 0,
            last_used: 0,
            handoff_from: if s.id.is_empty() {
                None
            } else {
                Some(s.id.clone())
            },
            rotate_pending: false,
        };
        self.save(&n)?;
        Ok(n)
    }

    /// A turn ran on `s`: count it, and arm rotation if the output says the context is full.
    pub fn after_turn(&self, s: &mut Session, context_full: bool) -> Result<()> {
        s.turns += 1;
        s.last_used = now_ms();
        if context_full {
            s.rotate_pending = true;
        }
        self.save(s)
    }

    pub fn request_rotation(&self) -> Result<Session> {
        let mut s = self.load();
        s.rotate_pending = true;
        self.save(&s)?;
        Ok(s)
    }

    /// The first prompt of a new session after a rotation: the chats' briefs, then the nudge.
    pub fn handoff_prompt(&self, chats: &[String]) -> String {
        let mut out = String::from(
            "You are continuing after a session rotation; your previous session's context is gone. \
             Here is the brief of each chat you were woken for:\n\n",
        );
        for c in chats {
            let b = self.mirror.brief(c);
            if !b.trim().is_empty() {
                out.push_str(&format!("--- chat {c} ---\n{b}\n"));
            }
        }
        out.push_str("\nUse arbos_read_chat for the full rows. Then handle the wake below.\n\n");
        out
    }

    pub fn json(&self) -> Value {
        let s = self.load();
        json!({
            "id": s.id, "agent": s.agent, "generation": s.generation, "created": s.created,
            "turns": s.turns, "last_used": s.last_used, "handoff_from": s.handoff_from,
            "rotate_pending": s.rotate_pending, "path": self.path,
        })
    }
}

/// The stable session field webhook / file receivers key their own thread on.
pub fn external_session(agent_id: &str, generation: u32) -> String {
    format!("{agent_id}:{generation}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::tests::temp_mirror;

    fn cfg(agent: &str) -> WakerConfig {
        WakerConfig {
            agent: Some(agent.into()),
            max_turns_per_session: 3,
            ..Default::default()
        }
    }

    #[test]
    fn ids_look_like_uuid_v4() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert_ne!(new_id(), new_id());
    }

    #[test]
    fn presets_resume_with_the_session_id() {
        for a in ["claude", "codex", "cursor", "hermes"] {
            let t = preset(a).unwrap();
            assert!(t.resume.iter().any(|x| x == "{session}"), "{a}");
            assert!(t.new.iter().any(|x| x == "{prompt}"), "{a}");
        }
        assert!(preset("nope").is_none());
        let t = preset("codex").unwrap();
        assert!(t.capture_codex_thread);
        assert_eq!(
            codex_thread_id(
                "{\"type\":\"thread.started\",\"thread_id\":\"abc\"}\n{\"type\":\"turn.started\"}"
            ),
            Some("abc".into())
        );
    }

    /// First wake → new; next wakes → resume; the cap → new with a handoff; a failed resume
    /// (rotate_pending) → new; the generation climbs and the old id is kept.
    #[test]
    fn session_lifecycle_new_resume_rotate() {
        let (_d, m) = temp_mirror();
        let store = Store::new(m, 3);
        let c = cfg("claude");
        let (s, a) = store.plan(&c, "claude");
        assert_eq!(a, Attempt::New { handoff: false });
        let mut s = store.begin_new(&s, "id-1".into()).unwrap();
        assert_eq!(s.generation, 1);
        for _ in 0..2 {
            let (_, a) = store.plan(&c, "claude");
            assert_eq!(a, Attempt::Resume);
            store.after_turn(&mut s, false).unwrap();
        }
        store.after_turn(&mut s, false).unwrap();
        let (s2, a) = store.plan(&c, "claude");
        assert_eq!(a, Attempt::New { handoff: true }, "turn cap reached");
        let s3 = store.begin_new(&s2, "id-2".into()).unwrap();
        assert_eq!(s3.generation, 2);
        assert_eq!(s3.handoff_from.as_deref(), Some("id-1"));
        store.request_rotation().unwrap();
        let (_, a) = store.plan(&c, "claude");
        assert_eq!(a, Attempt::New { handoff: true });
        // A different agent kind starts over.
        let (_, a) = store.plan(&cfg("codex"), "codex");
        assert_eq!(a, Attempt::New { handoff: false });
    }

    #[test]
    fn failure_and_context_full_patterns() {
        let p = "No conversation|not found|unknown session|invalid session";
        assert!(looks_like_resume_failure(
            "Error: No conversation found with session ID",
            p
        ));
        assert!(!looks_like_resume_failure("ok", p));
        assert!(looks_like_context_full(
            "the context window is full",
            "context window|too long|token limit"
        ));
    }

    #[test]
    fn handoff_prompt_carries_the_briefs() {
        let (_d, m) = temp_mirror();
        m.apply_event(&crate::mirror::tests::received(
            1, "c1", "m1", "hello", true,
        ))
        .unwrap();
        let store = Store::new(m, 50);
        let p = store.handoff_prompt(&["c1".into()]);
        assert!(p.contains("session rotation"));
        assert!(p.contains("--- chat c1 ---"));
        assert!(p.contains("1 rows"));
    }
}
