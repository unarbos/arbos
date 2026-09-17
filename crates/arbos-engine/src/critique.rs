//! A second reading of the diff before the turn ends.
//!
//! The class of SWE-bench failure that survived every lever through cycle
//! 10 is the right file fixed by the wrong mechanism: the agent's own
//! reproduction passes, so nothing it can run tells it the request is only
//! partly met. This asks one fresh model call — no history, only the
//! request and the diff — whether each behaviour the request names is
//! addressed, and nudges once with the gap it finds. Opt-in
//! (`ARBOS_CRITIQUE=1`): it costs a call per task, and cycle 11 of the
//! SWE-bench loop is where it is measured.

use anyhow::Result;
use arbos_core::{Event, EventKind};
use std::path::Path;
use std::process::Command;

use crate::provider::{ChatMessage, Provider};

pub const ENABLED_ENV: &str = "ARBOS_CRITIQUE";

/// The request and the diff each get this many characters at most; a
/// review of the tail of a huge diff is still a review, a refused call is
/// not.
const REQUEST_CHARS: usize = 12_000;
const DIFF_CHARS: usize = 40_000;

const SYSTEM: &str = "You review a code change against the request that asked for it. You did not write the change and have no other context. \
Read the request and list every distinct behaviour it asks for or symptom it reports, including the ones its examples imply. \
For each one, check the diff: addressed, partly, or not, with the diff hunk that does it. \
Then name one concrete input, taken from the request's own wording, that the diff would still get wrong — or say there is none. \
Do not comment on style, naming, or tests. Do not propose a rewrite. Be brief: under 200 words. \
End with exactly one line: `VERDICT: COMPLETE` or `VERDICT: INCOMPLETE — <the one gap that matters most>`.";

/// The first `n` chars of `s`, whole text, with a note when cut.
/// (`text::clip` keeps one line; the reviewer needs the whole request.)
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}\n[... clipped at {n} chars ...]")
}

pub fn enabled() -> bool {
    std::env::var(ENABLED_ENV).is_ok_and(|v| v == "1" || v == "true")
}

/// Whether this task (since the last user message) edited a file.
pub fn edited_in_task(events: &[Event]) -> bool {
    let mut edited = false;
    for e in events {
        match &e.kind {
            EventKind::User { .. } => edited = false,
            EventKind::Tool(rec)
                if rec.error.is_none()
                    && matches!(rec.name.as_str(), "edit" | "write" | "apply_patch") =>
            {
                edited = true
            }
            _ => {}
        }
    }
    edited
}

/// The task's own words: the last user message on the transcript.
pub fn request_text(events: &[Event]) -> Option<String> {
    events.iter().rev().find_map(|e| match &e.kind {
        EventKind::User { text, .. } => Some(text.clone()),
        _ => None,
    })
}

pub struct Critique {
    pub complete: bool,
    pub text: String,
    pub cost: Option<f64>,
}

/// The working tree's change against HEAD, untracked files included, or
/// None when there is nothing to review.
fn diff(cwd: &Path) -> Option<String> {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let mut out = run(&["diff", "HEAD", "--no-color", "--", ".", ":!*.lock"]).unwrap_or_default();
    if let Some(list) = run(&["ls-files", "--others", "--exclude-standard"]) {
        for f in list.lines().filter(|l| !l.is_empty()).take(8) {
            if let Ok(body) = std::fs::read_to_string(cwd.join(f)) {
                out.push_str(&format!(
                    "\n--- /dev/null\n+++ b/{f}\n(new file)\n{}\n",
                    cut(&body, 4_000)
                ));
            }
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// One call to the turn's model with the request and the diff. Ok(None)
/// when there is no diff to review; Err only when the call itself fails.
pub async fn run(provider: &Provider, request: &str, cwd: &Path) -> Result<Option<Critique>> {
    let Some(diff) = diff(cwd) else {
        return Ok(None);
    };
    let diff = if diff.len() > DIFF_CHARS {
        format!(
            "{}\n[... diff clipped at {DIFF_CHARS} chars ...]",
            &diff[..DIFF_CHARS]
        )
    } else {
        diff
    };
    let user = format!(
        "REQUEST:\n{}\n\nDIFF:\n```diff\n{}\n```",
        cut(request, REQUEST_CHARS),
        diff
    );
    let reviewer = Provider {
        trace_purpose: "critique".into(),
        reasoning_effort: None,
        cache_ttl: None,
        ..provider.clone()
    };
    let messages = [
        ChatMessage::plain("system", Some(SYSTEM.into())),
        ChatMessage::plain("user", Some(user)),
    ];
    let reply = reviewer.complete(&messages, &[]).await?;
    let text = reply.content.trim().to_string();
    let verdict = text
        .lines()
        .rev()
        .find(|l| l.contains("VERDICT:"))
        .unwrap_or("");
    let complete = verdict.contains("COMPLETE") && !verdict.contains("INCOMPLETE");
    Ok(Some(Critique {
        complete,
        text,
        cost: reply.cost,
    }))
}

/// What the agent reads when the reviewer found a gap.
pub fn nudge(c: &Critique) -> String {
    format!(
        "A second reader compared your diff with the request, without your history, and found a gap:\n\n{}\n\nEither close it — edit, then re-run your reproduction — or say in one sentence why the reader is wrong. Then finish.",
        c.text
    )
}
