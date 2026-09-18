//! Inbox files: `agents/<id>/inbox/<utc-time>-<from>-<seq>.md`. A message
//! to an agent is one file you can `cat` — the user's prompt, a peer's
//! `say`, a spawn brief, a kernel notice. Front matter in TOML between
//! `+++` lines, then the body. (Phase 2 of the file-system design.)
//!
//! ```markdown
//! +++
//! from = "agent:root"
//! kind = "request"
//! wake = true
//! hops = 2
//! sent = "2026-09-12T21:52:10Z"
//! +++
//! Please run the Linux build with the x11 feature and report the first error.
//! ```
//!
//! `wake = true` asks for a turn when the agent is idle; `false` is read at
//! the start of the agent's next turn, whatever causes it. The kernel
//! claims a waking file by renaming it into `turns/tNNNN/cause.md`: the
//! rename is the claim, so two schedulers cannot both take it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Place;

/// Reply budget for agent-to-agent chains (`say`): how many requests may
/// chain from one message before they fall back to notes.
pub const DEFAULT_HOPS: u8 = 3;

/// A `message` is read at the next turn; a `request` wants a turn now; a
/// `brief` is a spawned child's mission; a `wake` carries no words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Message {
    /// `user`, `user:<name>`, `agent:<id>`, `cron:<node>`, `kernel`.
    pub from: String,
    /// `message` | `request` | `brief` | `answer` | `approval` | `wake`.
    pub kind: String,
    pub wake: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reply_to: String,
    /// Reply budget left for agent-to-agent chains.
    pub hops: u8,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// RFC 3339 UTC.
    pub sent: String,
    /// How a person's words arrived: `voice` (a call) or `text` (typed).
    /// Empty on messages from agents and the kernel, and on files from
    /// before the key existed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub channel: String,
    /// The client that carried a person's words: `phone`, `desktop`, `cli`.
    /// Empty when unknown or not a person's message.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub device: String,
    /// A model for the turn this message opens, and that turn only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The short label of the turn this message opens (Cursor's
    /// `SendToAgent … title`): shown as the agent's live line until it
    /// says a step of its own, and kept in the turn's `meta.toml`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip)]
    pub body: String,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            from: "user".into(),
            kind: "request".into(),
            wake: true,
            reply_to: String::new(),
            hops: 0,
            attachments: Vec::new(),
            sent: rfc3339(crate::now_ms()),
            channel: String::new(),
            device: String::new(),
            model: String::new(),
            title: String::new(),
            body: String::new(),
        }
    }
}

impl Message {
    pub fn new(from: impl Into<String>, kind: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            kind: kind.into(),
            body: body.into(),
            ..Self::default()
        }
    }

    /// The whole file: front matter, then the body.
    pub fn render(&self) -> Result<String> {
        let mut front = toml::to_string(self)?;
        if !front.ends_with('\n') {
            front.push('\n');
        }
        let mut body = self.body.clone();
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        Ok(format!("+++\n{front}+++\n{body}"))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let rest = text
            .strip_prefix("+++\n")
            .or_else(|| text.strip_prefix("+++\r\n"))
            .context("inbox file does not start with +++")?;
        let end = rest
            .find("\n+++")
            .context("inbox file has no closing +++")?;
        let front = &rest[..end];
        let after = &rest[end + 4..];
        let body = after
            .strip_prefix("\r\n")
            .or_else(|| after.strip_prefix('\n'))
            .unwrap_or(after);
        let mut msg: Message = toml::from_str(front).context("inbox front matter")?;
        msg.body = body.trim_end_matches('\n').to_string();
        Ok(msg)
    }

    /// Unix millis of `sent`, when it parses.
    pub fn sent_ms(&self) -> Option<i64> {
        crate::parse_instant_ms(&self.sent)
    }
}

/// A file in an inbox, read.
#[derive(Debug, Clone)]
pub struct Filed {
    pub path: PathBuf,
    /// The file name, the message's id from the outside.
    pub name: String,
    pub msg: Message,
}

pub fn inbox_dir(place: &Place, agent: &str) -> PathBuf {
    place.agent_dir(agent).join("inbox")
}

pub fn turns_dir(place: &Place, agent: &str) -> PathBuf {
    place.agent_dir(agent).join("turns")
}

/// Drop a message into an agent's inbox: whole or absent, never partial.
/// Returns the file name.
pub fn deliver(place: &Place, agent: &str, msg: &Message) -> Result<String> {
    let dir = inbox_dir(place, agent);
    std::fs::create_dir_all(&dir)?;
    let stamp = file_stamp(msg.sent_ms().unwrap_or_else(crate::now_ms));
    let who = slug(&msg.from);
    let text = msg.render()?;
    for seq in 0..1000u32 {
        let name = format!("{stamp}-{who}-{seq:03}.md");
        let path = dir.join(&name);
        if path.exists() {
            continue;
        }
        let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
        std::fs::write(&tmp, &text)?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => return Ok(name),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                if path.exists() {
                    continue;
                }
                return Err(e).with_context(|| format!("deliver {}", path.display()));
            }
        }
    }
    bail!(
        "inbox {}: a thousand files in one millisecond",
        dir.display()
    )
}

/// How a worker's `done` message opens, one phrase per way a turn can
/// end. The kernel writes these (`plan::child_done`, the remote track's
/// relay) and every window reads them to draw the worker's last words as
/// a card (the desktop's `done_report`); a phrasing on one side the other
/// does not know is drawn raw — prefix, ellipsis, and
/// `.arbos/agents/…/transcript.jsonl` — in a person's chat (QA `qal-j36`,
/// `mt-14`). One list, both sides.
pub const DONE_ENDED: &str = "Turn ended. Last words:";
pub const DONE_ENDED_BADLY: &str = "Turn ended badly. Last words:";
pub const DONE_USER_STOP: &str = "Turn stopped by the user. Last words:";
pub const DONE_TURN_CAP: &str = "Turn stopped at the per-turn cap. Last words:";
pub const DONE_PREFIXES: [&str; 4] = [DONE_ENDED, DONE_ENDED_BADLY, DONE_USER_STOP, DONE_TURN_CAP];

/// The prefix a done body opens with, and the words after it, or `None`
/// for a line that is not a worker's report.
pub fn done_report(text: &str) -> Option<(&'static str, &str)> {
    let text = text.trim_start();
    DONE_PREFIXES
        .iter()
        .find_map(|p| text.strip_prefix(*p).map(|rest| (*p, rest.trim_start())))
}

/// Every message waiting, oldest first. Unreadable files are skipped.
pub fn list(place: &Place, agent: &str) -> Vec<Filed> {
    let dir = inbox_dir(place, agent);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<Filed> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.ends_with(".md") {
                return None;
            }
            let text = std::fs::read_to_string(e.path()).ok()?;
            let msg = Message::parse(&text).ok()?;
            Some(Filed {
                path: e.path(),
                name,
                msg,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read one message by file name.
pub fn read(place: &Place, agent: &str, name: &str) -> Result<Filed> {
    let path = inbox_dir(place, agent).join(name);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Filed {
        path,
        name: name.to_string(),
        msg: Message::parse(&text)?,
    })
}

/// Kinds a running turn takes at its tool boundaries: words meant for
/// the turn under way (`steer`), and the kernel's "something finished"
/// (`wake`). Requests, briefs, and notes wait for the next turn.
pub fn is_steer_kind(kind: &str) -> bool {
    // An `answer` reaches a running turn too: a question asked with
    // `wait:false` gets its reply at the next tool boundary instead of a
    // turn later. Idle, it opens the next turn like any wake. A worker's
    // `done` reaches a running parent the same way: the report is what a
    // parent that ran `sleep` was waiting for, and the yield to it (#432)
    // is only honest if the report then follows at the boundary.
    matches!(kind, "steer" | "wake" | "answer" | "done")
}

/// Whether a steer waits: a batch of tool calls stops taking new calls
/// so the model reads it sooner.
pub fn has_steer(place: &Place, agent: &str) -> bool {
    list(place, agent)
        .iter()
        .any(|f| is_steer_kind(&f.msg.kind))
}

/// Whether a person's own words wait for this agent's running turn: a
/// steer from `user` (not a peer's `say mode=steer`, not the kernel's
/// wake). What an attached tool call yields to.
/// A worker's report (`done`) is waiting in `agent`'s inbox: the event a
/// parent that ran `sleep` to wait for it was waiting for. What an
/// attached tool call of a parent yields to, beside the user's words.
pub fn has_child_done(place: &Place, agent: &str) -> bool {
    list(place, agent)
        .iter()
        .any(|f| f.msg.kind == "done" && f.msg.from.starts_with("agent:"))
}

pub fn has_user_steer(place: &Place, agent: &str) -> bool {
    list(place, agent)
        .iter()
        .any(|f| f.msg.kind == "steer" && (f.msg.from == "user" || f.msg.from.starts_with("user:")))
}

/// A message from the same sender with the same words already waiting
/// (a steer or a queued prompt): the one to point at instead of filing
/// the words again. A person who repeats "run it" four times into a
/// silent turn wants one answer, not four stacked lines.
pub fn pending_duplicate(place: &Place, agent: &str, from: &str, body: &str) -> Option<Filed> {
    let want = body.trim();
    if want.is_empty() {
        return None;
    }
    list(place, agent)
        .into_iter()
        .find(|f| f.msg.from == from && f.msg.body.trim() == want)
}

/// Whether a steer waiting for this agent says stop (a stop word on its
/// own, or a first line that is one). Such a steer cancels work that has
/// not started; any other steer waits for the tool boundary and cancels
/// nothing the user already asked for.
pub fn has_stop_steer(place: &Place, agent: &str) -> bool {
    list(place, agent).iter().any(|f| {
        is_steer_kind(&f.msg.kind)
            && (crate::is_stop_word(&f.msg.body)
                || f.msg.body.lines().next().is_some_and(crate::is_stop_word))
    })
}

/// The messages a running turn reads now, oldest first, still in the
/// inbox. The turn writes them to its transcript and only then calls
/// [`release`] on each: a person's words are never deleted before the
/// record that replaces them is on disk. A file the turn never reaches
/// (it ends first) stays and starts the next turn, so nothing said
/// mid-turn is lost to timing — and one left behind by a crash between
/// the write and the release is said twice, which a reader can see,
/// rather than lost, which nobody can.
pub fn steers(place: &Place, agent: &str) -> Vec<Filed> {
    list(place, agent)
        .into_iter()
        .filter(|f| is_steer_kind(&f.msg.kind))
        .collect()
}

/// The inbox file of a steer the transcript now holds, taken out.
pub fn release(filed: &Filed) -> Result<()> {
    std::fs::remove_file(&filed.path).with_context(|| format!("remove {}", filed.path.display()))
}

/// [`steers`] then [`release`], for callers that keep the words in memory.
pub fn take_steers(place: &Place, agent: &str) -> Vec<Message> {
    let mut out = Vec::new();
    for filed in steers(place, agent) {
        if release(&filed).is_ok() {
            out.push(filed.msg);
        }
    }
    out
}

/// Rewrite a message in place (its `wake`, say). Whole or absent.
pub fn rewrite(filed: &Filed) -> Result<()> {
    let tmp = filed
        .path
        .with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, filed.msg.render()?)?;
    std::fs::rename(&tmp, &filed.path)?;
    Ok(())
}

/// Take the file out of the inbox for good.
pub fn remove(place: &Place, agent: &str, name: &str) -> Result<()> {
    let path = inbox_dir(place, agent).join(name);
    std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))
}

/// Claim a waking message for a turn: `turns/tNNNN/` is made, the file is
/// renamed into it as `cause.md`. The rename is the claim. Returns the
/// turn folder.
pub fn claim(place: &Place, agent: &str, filed: &Filed) -> Result<PathBuf> {
    let turns = turns_dir(place, agent);
    std::fs::create_dir_all(&turns)?;
    let mut n = next_turn_number(&turns);
    loop {
        let dir = turns.join(format!("t{n:04}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                std::fs::rename(&filed.path, dir.join("cause.md"))
                    .with_context(|| format!("claim {}", filed.name))?;
                return Ok(dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e.into()),
        }
    }
}

fn next_turn_number(turns: &Path) -> u32 {
    std::fs::read_dir(turns)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_prefix('t')?
                .parse::<u32>()
                .ok()
        })
        .max()
        .map(|m| m + 1)
        .unwrap_or(1)
}

/// `20260912T215210.123Z` — sortable, to the millisecond.
fn file_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}.{frac:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `2026-09-12T21:52:10Z`.
pub fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn slug(from: &str) -> String {
    let s: String = from
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.trim_matches('-').chars().take(24).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// qal-j36 / mt-14: the kernel's done line and the windows' reader
    /// share one list, so a way a turn can end is never drawn raw. Every
    /// prefix reads back to itself with the words after it; the file
    /// pointer the kernel appends is the reader's to cut; prose that
    /// merely mentions a turn is not a report.
    #[test]
    fn every_done_prefix_reads_back_and_prose_does_not() {
        for p in DONE_PREFIXES {
            let body = format!("{p} the words\n(transcript: .arbos/agents/w1/transcript.jsonl)");
            let (got, rest) = done_report(&body).expect(p);
            assert_eq!(got, p);
            assert!(rest.starts_with("the words"), "{rest}");
        }
        assert_eq!(
            done_report(
                "Turn stopped at the per-turn cap. Last words: Stopped at the per-turn cap: $1.20 over $1.00"
            ),
            Some((
                DONE_TURN_CAP,
                "Stopped at the per-turn cap: $1.20 over $1.00"
            ))
        );
        assert!(done_report("The turn ended. Last words were fine.").is_none());
        assert!(done_report("Turn ended with no report").is_none());
        // No prefix is a prefix of another: the first match is the match.
        for a in DONE_PREFIXES {
            for b in DONE_PREFIXES {
                assert!(a == b || !b.starts_with(a), "{a:?} opens {b:?}");
            }
        }
    }

    #[test]
    fn a_boundary_takes_every_waiting_steer_in_order() {
        let dir = std::env::temp_dir().join(format!("arbos-inbox-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let place = Place::new(&dir);
        std::fs::create_dir_all(place.agent_dir("root")).unwrap();
        for i in 0..25 {
            let mut m = Message::new("user", "steer", format!("STEER-{i:02}"));
            m.sent = rfc3339(1_789_000_000_000 + i);
            deliver(&place, "root", &m).unwrap();
        }
        let mut note = Message::new("agent:peer", "message", "a note, not a steer");
        note.wake = false;
        deliver(&place, "root", &note).unwrap();
        let all: Vec<String> = take_steers(&place, "root")
            .into_iter()
            .map(|m| m.body)
            .collect();
        assert_eq!(all.len(), 25);
        assert_eq!(all.first().map(String::as_str), Some("STEER-00"));
        assert_eq!(all.last().map(String::as_str), Some("STEER-24"));
        assert!(all.windows(2).all(|w| w[0] < w[1]), "oldest first");
        assert!(take_steers(&place, "root").is_empty());
        assert!(!has_steer(&place, "root"));
        assert_eq!(
            list(&place, "root").len(),
            1,
            "the note stays for the next turn"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn channel_is_written_when_set_and_absent_otherwise() {
        let mut voice = Message::new("user", "request", "send an agent to fix CI");
        voice.channel = "voice".into();
        voice.device = "desktop".into();
        let text = voice.render().unwrap();
        assert!(text.contains("channel = \"voice\"\n"), "{text}");
        assert!(text.contains("device = \"desktop\"\n"), "{text}");
        let back = Message::parse(&text).unwrap();
        assert_eq!(back.channel, "voice");
        assert_eq!(back.device, "desktop");
        assert_eq!(back.body, "send an agent to fix CI");

        let note = Message::new("agent:peer", "message", "a note");
        let text = note.render().unwrap();
        assert!(!text.contains("channel"), "agents have no channel: {text}");
        // Files from before the key existed parse with an empty channel.
        let old = "+++\nfrom = \"user\"\nkind = \"request\"\nwake = true\nhops = 0\nsent = \"2026-09-12T21:52:10Z\"\n+++\nhello\n";
        assert_eq!(Message::parse(old).unwrap().channel, "");
    }
}
