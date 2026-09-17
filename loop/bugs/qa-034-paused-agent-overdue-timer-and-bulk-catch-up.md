---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qa-034: a paused agent's overdue timer, a 17,969-line transcript, and bulk catch-up at kernel start

- Feature: subscriptions (timers), pause/resume, transcripts of standing agents
- Severity: medium — a resume or a kernel start could fire a backlog of missed timers, each a model call; a transcript that never ends grows every prompt and every load
- Found in the Mac wake-up incident (Mac worker's report, 2026-09-13)

## Repro

1. An agent with an hourly timer; pause it for five hours (or stop the kernel for five hours).
2. Resume it / start the kernel.

## Actual (before #130)

The timer's `next_due` sat in the past; nothing fired while paused (correct), but a resume or a start could fire it at once, and a migrated subscription from the old plan engine kept its old `next_due`. The agent's transcript had 17,969 rows and kept growing.

## Expected / fix (PR #130)

- Paused agents never fire (pinned by an e2e).
- On resume every subscription due in the past is rescheduled one period from now (`subscriptions_resumed` in `kernel.log`).
- At any tick a subscription overdue by whole periods fires **once** with "(missed N earlier firing(s) …)" appended to its message, or with `catch_up = "skip"` in `config.toml` only reschedules. Never one firing per missed period.
- Past `transcript_roll_lines` (default 10000) the transcript rolls at a turn end into `transcript-archive/NNNN.jsonl`; the fresh file opens with the last compaction summary and a pointer; the window gets a `rewound` frame.
- `check` warns at 10k lines and about paused agents with overdue subscriptions.

## Scenarios to add

`crates/arbos-kernel/tests/wakeup_e2e.rs` covers catch-up once, skip, paused/resume. Add with a real model: an agent paused for an hour with a 5-minute timer, resumed — one turn at most in the next five minutes, and the message carries no "missed" note (the resume rescheduled it). And a 10,001-line transcript: after the next turn ends the window shows the short file and `transcript-archive/0001.jsonl` exists.
