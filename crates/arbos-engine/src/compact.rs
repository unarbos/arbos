//! Context compaction: fold, then summarise. Nothing leaves the disk.
//!
//! Two stages, both recorded as events so every projection is
//! deterministic and the cached prefix survives between steps:
//!
//! 1. **Fold** (`fold_at`, no model call). Tool bodies older than the newest
//!    `protect_tool_results` collapse to a one-line cite. Observation
//!    masking; halves cost at equal solve rate on SWE-bench.
//! 2. **Compaction** (`compact_at`). The oldest units are replaced by a
//!    structured checkpoint written by a (possibly cheaper) model. Units are
//!    whole turns, and model steps inside the newest turn, so a tool call
//!    never splits from its result and one long agentic turn still folds.
//!    Recent units worth `keep_recent` tokens stay verbatim. A later
//!    compaction sees the earlier summary in place of its raw events, so
//!    summariser input is bounded by prior-summary + newly folded units.
//!
//! The summary renders at the position of the first folded line, so order
//! is preserved when several checkpoints coexist.
//!
//! Addresses are `Event::seq`: the physical line in transcript.jsonl. A
//! `Compaction` names `lo..=hi` by line, so it stays right if a line in
//! between is damaged, and the cite the model reads is what `grep -n` shows.

use anyhow::Result;
use arbos_core::{Agent, Event, EventKind, Place, append_event, append_events, load_transcript};
use std::{path::Path, sync::Arc};

use crate::{
    control::TurnControl,
    project::{Projection, cite_path, project, render_compaction},
    provider::{ChatMessage, Interrupted, Provider},
    summarise::{self, Summarised},
    tools::Hooks,
};

/// Past this share of the window a failed summariser no longer gets to
/// wait for the next step: the recovery record is committed instead.
const HARD_LIMIT: f64 = 0.95;
/// Below this many tokens freed, a compaction is not worth its model call.
const MIN_FREED: u64 = 512;

#[derive(Debug, Clone)]
pub struct Policy {
    pub window: u64,
    /// Tokens at which stage two runs.
    pub compact_at: u64,
    /// Tokens at which stage one runs.
    pub fold_at: u64,
    /// Tokens of recent units kept verbatim.
    pub keep_recent: u64,
    /// `keep_recent` for a manual `compact`: the configured value as given,
    /// without the 1 000-token floor automatic compaction applies. The user
    /// asked; only the newest unit is guaranteed to stay.
    pub keep_recent_manual: u64,
    /// Output headroom for the summariser.
    pub reserve: u64,
    pub protect_tool_results: usize,
    /// Summariser model. Empty = the turn's model.
    pub model: String,
    /// The summariser model's window, when smaller than the turn's.
    pub summary_window: u64,
    /// Most bytes one step's fresh tool results may take together. The
    /// projection's own cap on a large window; a share of a small one, so
    /// a single big read cannot fill the model's whole context.
    pub step_bytes: usize,
}

impl Policy {
    const MIN_FRAC: f64 = 0.05;
    const MAX_FRAC: f64 = 0.99;
    /// Share of the window one step's fresh tool results may take.
    const STEP_SHARE: f64 = 0.4;
    /// Chars per token when sizing tool bodies. Code and logs run denser
    /// than prose; 3 leaves margin against the provider's count.
    const BODY_CHARS_PER_TOKEN: u64 = 3;

    /// Clamp raw config into something that cannot deadlock the loop:
    /// fold strictly before compact, keep and reserve inside the window.
    pub fn new(window: u64, cfg: &crate::host::HostConfig) -> Self {
        let frac = |f: f64| f.clamp(Self::MIN_FRAC, Self::MAX_FRAC);
        let compact_at = frac(cfg.compact_at);
        let fold_at = frac(cfg.fold_at).min(compact_at - 0.01).max(Self::MIN_FRAC);
        let step_tokens = (window as f64 * Self::STEP_SHARE) as u64;
        Self {
            window,
            compact_at: (window as f64 * compact_at) as u64,
            fold_at: (window as f64 * fold_at) as u64,
            keep_recent: cfg.keep_recent_tokens.max(1_000).min(window / 2),
            keep_recent_manual: cfg.keep_recent_tokens.min(window / 2),
            reserve: cfg.reserve_tokens.max(1_024).min(window / 4),
            protect_tool_results: cfg.protect_tool_results.max(1),
            step_bytes: (step_tokens * Self::BODY_CHARS_PER_TOKEN)
                .min(crate::project::STEP_BYTES as u64) as usize,
            model: cfg.compact_model.clone(),
            summary_window: if cfg.compact_window_tokens == 0 {
                window
            } else {
                cfg.compact_window_tokens.min(window)
            },
        }
    }

    fn hard_limit(&self) -> u64 {
        (self.window as f64 * HARD_LIMIT) as u64
    }
}

/// One thing the model may see, in transcript order.
#[derive(Debug, Clone)]
pub enum Item<'a> {
    Event {
        seq: u64,
        event: &'a Event,
        /// A tool body shown as a cite, not text.
        folded: bool,
    },
    /// An earlier compaction, standing in for lines `lo..=hi`.
    Compaction { lo: u64, hi: u64, summary: &'a str },
}

impl Item<'_> {
    pub fn seq_lo(&self) -> u64 {
        match self {
            Item::Event { seq, .. } => *seq,
            Item::Compaction { lo, .. } => *lo,
        }
    }

    pub fn seq_hi(&self) -> u64 {
        match self {
            Item::Event { seq, .. } => *seq,
            Item::Compaction { hi, .. } => *hi,
        }
    }

    pub fn is_tool(&self) -> bool {
        matches!(self, Item::Event { event, .. } if matches!(event.kind, EventKind::Tool(_)))
    }
}

struct Span<'a> {
    lo: u64,
    hi: u64,
    /// Line of the event that declared this span.
    seq: u64,
    summary: &'a str,
}

const RESET_SUMMARY: &str = "Earlier turns were hidden by a window reset before compaction existed. The record is in the transcript.";

/// Apply every Compaction, Fold and legacy WindowReset to the log.
pub fn visible(events: &[Event]) -> Vec<Item<'_>> {
    let mut spans: Vec<Span> = Vec::new();
    let mut fold_through: Option<u64> = None;
    for e in events {
        match &e.kind {
            EventKind::Compaction {
                lo, hi, summary, ..
            } => spans.push(Span {
                lo: *lo,
                hi: *hi,
                seq: e.seq,
                summary,
            }),
            EventKind::WindowReset {} if e.seq > 1 => spans.push(Span {
                lo: 1,
                hi: e.seq - 1,
                seq: e.seq,
                summary: RESET_SUMMARY,
            }),
            EventKind::Fold { through, .. } => {
                fold_through = Some(fold_through.map_or(*through, |t| t.max(*through)));
            }
            _ => {}
        }
    }
    // A span is superseded when another contains it: a later, wider one
    // merged it; a narrower one inside it says nothing new. Identical
    // ranges: the newer wins. Only live spans hide anything, so a span can
    // never vanish without its summary.
    let contains = |o: &Span, s: &Span| o.lo <= s.lo && s.hi <= o.hi;
    let mut live: Vec<&Span> = spans
        .iter()
        .filter(|s| {
            !spans.iter().any(|o| {
                o.seq != s.seq && contains(o, s) && ((o.lo, o.hi) != (s.lo, s.hi) || o.seq > s.seq)
            })
        })
        .collect();
    live.sort_by_key(|s| s.lo);
    let hidden = |seq: u64| live.iter().any(|s| s.lo <= seq && seq <= s.hi);

    let mut items = Vec::with_capacity(events.len());
    let mut next = 0;
    for e in events {
        // A summary renders where its first replaced line was, even if that
        // line itself is now unparseable and absent from `events`.
        while let Some(s) = live.get(next).filter(|s| s.lo <= e.seq) {
            items.push(Item::Compaction {
                lo: s.lo,
                hi: s.hi,
                summary: s.summary,
            });
            next += 1;
        }
        if hidden(e.seq) {
            continue;
        }
        match &e.kind {
            EventKind::Compaction { .. }
            | EventKind::Fold { .. }
            | EventKind::WindowReset {}
            | EventKind::Thinking { .. } => {
                continue;
            }
            _ => {}
        }
        let folded =
            matches!(e.kind, EventKind::Tool(_)) && fold_through.is_some_and(|t| e.seq <= t);
        items.push(Item::Event {
            seq: e.seq,
            event: e,
            folded,
        });
    }
    for s in &live[next..] {
        items.push(Item::Compaction {
            lo: s.lo,
            hi: s.hi,
            summary: s.summary,
        });
    }
    items
}

/// Indices where a turn begins. Turns are delimited by their end
/// (`TurnComplete` / `Interrupted`), not by the wake: a `say` from another
/// agent, an answer, or a kernel notice lands on the log *before* the wake
/// it causes, and belongs to the turn it triggers. A wake that follows
/// mid-turn events marks a turn that never ended (the kernel died); it
/// starts a new turn too. Never a tool result.
pub fn turn_starts(items: &[Item]) -> Vec<usize> {
    let mut starts = vec![0];
    for i in 1..items.len() {
        let start = match (&items[i - 1], &items[i]) {
            (_, Item::Compaction { .. }) | (Item::Compaction { .. }, _) => true,
            (Item::Event { event: prev, .. }, Item::Event { event, .. }) => {
                prev.ends_turn()
                    || (matches!(event.kind, EventKind::Wake { .. })
                        && matches!(
                            prev.kind,
                            EventKind::Assistant { .. }
                                | EventKind::Tool(_)
                                | EventKind::Wake { .. }
                        ))
            }
        };
        if start {
            starts.push(i);
        }
    }
    starts
}

/// Fold units: every turn start, plus every model step inside the newest
/// turn. A step is an assistant reply and the tool results it called for,
/// so a cut there never separates a call from its result. Without the step
/// cuts, one long agentic turn would be the only unit and could never shrink.
pub fn units(items: &[Item]) -> Vec<usize> {
    let mut starts = turn_starts(items);
    let last_turn = *starts.last().unwrap_or(&0);
    for (i, item) in items.iter().enumerate().skip(last_turn + 1) {
        if matches!(item, Item::Event { event, .. } if matches!(event.kind, EventKind::Assistant { .. }))
        {
            starts.push(i);
        }
    }
    starts
}

/// The item range `[0, b]` to fold: the oldest units, keeping the newest
/// units that fit in `keep_recent` tokens (always the newest one). None
/// when there is nothing old enough, or when all that would fold is an
/// earlier summary (re-summarising it frees nothing).
pub fn choose_range(items: &[Item], tokens: &[u64], keep_recent: u64) -> Option<usize> {
    choose_range_freeing(items, tokens, keep_recent, MIN_FREED)
}

/// `choose_range` with an explicit floor on the tokens a compaction must
/// free. Automatic compaction uses `MIN_FREED` (a cache miss must pay for
/// itself); a manual `compact` is the user asking, so anything old enough
/// goes.
pub fn choose_range_freeing(
    items: &[Item],
    tokens: &[u64],
    keep_recent: u64,
    min_freed: u64,
) -> Option<usize> {
    let starts = units(items);
    if items.is_empty() || starts.len() <= 1 {
        return None;
    }
    let unit_tokens = |u: usize| -> u64 {
        let a = starts[u];
        let b = starts.get(u + 1).copied().unwrap_or(items.len());
        tokens[a..b].iter().sum()
    };
    let mut keep = 0;
    let mut acc = 0;
    for u in (0..starts.len()).rev() {
        let t = unit_tokens(u);
        // A unit that would take the kept set over the line is old enough
        // to go. Keeping it instead left one long turn just behind the
        // newest uncompactable: with only the earlier summary in front of
        // it, the loop sat over budget with "nothing old enough".
        if keep > 0 && acc + t > keep_recent {
            break;
        }
        acc += t;
        keep += 1;
    }
    if keep >= starts.len() {
        return None;
    }
    let b = starts[starts.len() - keep] - 1;
    let only_summary = b == 0 && matches!(items[0], Item::Compaction { .. });
    if only_summary || tokens[..=b].iter().sum::<u64>() < min_freed {
        return None;
    }
    Some(b)
}

/// The line through which tool bodies should fold. Oldest first, and only
/// as many as it takes to bring `used` down to `target`; the newest
/// `protect` results are never touched. Every fold changes the middle of
/// the prompt and costs one cache miss, so nothing folds unless at least
/// `MIN_FREED` tokens come back.
///
/// Folding by count ("everything but the newest 8") was what made a model
/// in the middle of a feature re-read the same files every other step:
/// the eight newest results are rarely the eight it still needs. Folding
/// by size, oldest first, keeps the working set the model built up.
pub fn fold_point(
    items: &[Item],
    tokens: &[u64],
    protect: usize,
    used: u64,
    target: u64,
) -> Option<u64> {
    let tools: Vec<(u64, u64)> = items
        .iter()
        .zip(tokens)
        .filter_map(|(it, t)| match it {
            Item::Event {
                seq, folded: false, ..
            } if it.is_tool() => Some((*seq, *t)),
            _ => None,
        })
        .collect();
    if tools.len() <= protect {
        return None;
    }
    let need = used.saturating_sub(target).max(MIN_FREED);
    let mut freed = 0;
    let mut through = None;
    for (seq, t) in &tools[..tools.len() - protect] {
        freed += t;
        through = Some(*seq);
        if freed >= need {
            break;
        }
    }
    if freed < MIN_FREED {
        return None;
    }
    through
}

/// What every step of a turn needs to manage its context. Built once.
pub struct Cx<'a> {
    pub place: &'a Place,
    pub agent: &'a Agent,
    pub transcript: &'a Path,
    pub skills: &'a [String],
    pub policy: &'a Policy,
    pub hooks: &'a Arc<dyn Hooks>,
}

/// The messages for the next model call, with the numbers the caller needs.
pub struct Managed {
    pub messages: Vec<ChatMessage>,
    /// Uncalibrated chars/4 estimate. Divide the provider's count by this.
    pub raw: u64,
    /// Calibrated estimate of what `messages` will cost.
    pub estimated: u64,
    /// Calibrated estimate of the system prompt alone (the messages before
    /// the conversation): what every call pays before any history.
    pub system: u64,
}

/// The working set as projected right now.
struct Working<'a> {
    items: Vec<Item<'a>>,
    proj: Projection,
    raw: u64,
    used: u64,
}

impl<'a> Working<'a> {
    fn load(cx: &Cx<'_>, events: &'a [Event], calib: f64) -> Self {
        let items = visible(events);
        let proj = project(cx.place, cx.agent, &items, cx.skills, cx.policy.step_bytes);
        let raw = proj.tokens();
        Self {
            items,
            proj,
            raw,
            used: scaled(raw, calib),
        }
    }
}

fn scaled(n: u64, calib: f64) -> u64 {
    (n as f64 * calib) as u64
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Fold,
    Compact,
    Done,
}

enum Move {
    Fold(u64),
    /// Fold items `0..=b`.
    Compact(usize),
    Done(Option<String>),
}

/// One decision, no side effects.
fn next_move(w: &Working, policy: &Policy, stage: Stage, manual: bool) -> Move {
    if stage <= Stage::Fold && (manual || w.used >= policy.fold_at) {
        // Aim a little under the line so the next few steps do not fold
        // again at once; each fold is a prompt-cache miss.
        let target = policy.fold_at.saturating_sub(policy.window / 20);
        // `item_tokens` are raw estimates; `used` is calibrated. Compare in
        // raw units so the sum of freed items means the same thing.
        let calib = if w.raw > 0 {
            w.used as f64 / w.raw as f64
        } else {
            1.0
        };
        let target_raw = (target as f64 / calib.max(0.01)) as u64;
        if let Some(through) = fold_point(
            &w.items,
            &w.proj.item_tokens,
            policy.protect_tool_results,
            w.raw,
            target_raw,
        ) {
            return Move::Fold(through);
        }
    }
    if stage <= Stage::Compact && (manual || w.used >= policy.compact_at) {
        let (keep, min_freed) = if manual {
            (policy.keep_recent_manual, 1)
        } else {
            (policy.keep_recent, MIN_FREED)
        };
        if let Some(b) = choose_range_freeing(&w.items, &w.proj.item_tokens, keep, min_freed) {
            return Move::Compact(b);
        }
        if manual {
            return Move::Done(Some(
                "nothing to compact yet: the whole working set is recent".into(),
            ));
        }
        if w.used >= policy.compact_at {
            return Move::Done(Some(format!(
                "over budget (~{}k tokens) with nothing old enough to compact; the next call may be rejected",
                w.used / 1000
            )));
        }
    }
    Move::Done(None)
}

/// Bring the working set under budget before a model call: fold old tool
/// bodies first (free), summarise the oldest units if still over (one
/// cheap model call). Both are appended to the transcript, so the
/// projection stays deterministic and the cached prefix survives.
///
/// Returns `Err(Interrupted)` if the user stopped the turn while the
/// summariser was running; nothing partial is written in that case.
pub async fn manage(
    cx: &Cx<'_>,
    events: &mut Vec<Event>,
    provider: &Provider,
    control: &TurnControl,
    calib: f64,
    manual: bool,
) -> Result<Managed> {
    let mut stage = Stage::Fold;
    let mut manual = manual;
    loop {
        let w = Working::load(cx, events, calib);
        match next_move(&w, cx.policy, stage, manual) {
            Move::Done(note) => {
                if let Some(text) = note {
                    // On the transcript as well as the wire: a client that
                    // attaches later should still see why nothing changed.
                    let notice = Event::new(EventKind::Notice {
                        text,
                        failed: false,
                    });
                    let _ = append_event(cx.transcript, &notice);
                    cx.hooks.emit(&notice);
                }
                return Ok(Managed {
                    system: scaled(w.proj.base_tokens, calib),
                    messages: w.proj.messages,
                    raw: w.raw,
                    estimated: w.used,
                });
            }
            Move::Fold(through) => {
                let tokens = w.used;
                drop(w);
                append_event(
                    cx.transcript,
                    &Event::new(EventKind::Fold { through, tokens }),
                )?;
                stage = Stage::Compact;
            }
            Move::Compact(b) => {
                let span = &w.items[..=b];
                let (lo, hi) = (span[0].seq_lo(), span[b].seq_hi());
                let turns = turn_starts(span).len();
                let freed = scaled(w.proj.item_tokens[..=b].iter().sum::<u64>(), calib);
                cx.hooks.emit(&Event::new(EventKind::Notice {
                    text: format!("compacting {turns} turn(s), ~{}k tokens…", w.used / 1000),
                    failed: false,
                }));
                let outcome = summarise::run(provider, cx.policy, span, control.cancel()).await;
                let summarised = match outcome {
                    Ok(s) => s,
                    Err(e) if e.is::<Interrupted>() || control.is_stopped() => {
                        return Err(Interrupted.into());
                    }
                    Err(e) if w.used < cx.policy.hard_limit() => {
                        // Room to try again next step with a real summary
                        // rather than commit the lossy record now.
                        cx.hooks.emit(&Event::new(EventKind::Notice {
                            text: format!("compaction postponed: summariser failed ({e:#}); retrying next step"),
                            failed: false,
                        }));
                        return Ok(Managed {
                            system: scaled(w.proj.base_tokens, calib),
                            messages: w.proj.messages,
                            raw: w.raw,
                            estimated: w.used,
                        });
                    }
                    Err(e) => Summarised::Recovery {
                        text: summarise::recovery_record(span),
                        reason: format!("{e:#}"),
                    },
                };
                let after = w.used.saturating_sub(freed)
                    + scaled(
                        crate::project::message_tokens(&ChatMessage::plain(
                            "user",
                            Some(render_compaction(
                                lo,
                                hi,
                                summarised.text(),
                                &cite_path(cx.agent),
                            )),
                        )),
                        calib,
                    );
                let notice = format!(
                    "compacted {turns} turn(s): ~{}k → ~{}k tokens ({}); full record in {}:{lo}–{hi}",
                    w.used / 1000,
                    after / 1000,
                    summarised.how(),
                    cite_path(cx.agent),
                );
                let before = w.used;
                drop(w);
                let (summary, model) = summarised.into_parts();
                let landed = [
                    Event::new(EventKind::Compaction {
                        lo,
                        hi,
                        summary,
                        tokens_before: before,
                        tokens_after: after,
                        model,
                    }),
                    Event::new(EventKind::Notice {
                        text: notice,
                        failed: false,
                    }),
                ];
                // The kernel's transcript tail puts these on the wire.
                append_events(cx.transcript, &landed)?;
                manual = false;
                stage = Stage::Done;
            }
        }
        *events = load_transcript(cx.transcript)?;
    }
}
