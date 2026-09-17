# 2026-09-16-2 — FIXTURE — the sheet froze when I opened it, and my key [redacted:openai-key] was on screen

> **Not a real report. Do not diagnose from it, and do not quote it as
> something Jacob saw.**
>
> This is the smoke fixture that proved the hop through the hub, written by
> `write_one_report_for_the_poller` and delivered with the real writer
> credentials. Its words and its events are both invented and do not
> correspond: the note complains about the sheet freezing, and the trajectory
> is a made-up `cargo test` failure.
>
> It is genuine evidence of one thing only — that a report can cross the hub
> and be picked up. Everything inside it is made up.
>
> Fixtures written from [#361](https://github.com/unarbos/arbos/pull/361)
> onwards mark themselves. This one and `2026-09-16-1` predate that and are
> marked by hand.

- Sent: 2026-09-16 15:42 UTC
- App: 0.2.0 (1113) commit `0a44e09`
- Kernel: 0.2.0 `abc123def456` built ?
- Machine: macos/aarch64, project `subnet120`
- Model: openrouter gemini
- Report id: `20260916T154210Z-686c`

## What he wrote

the sheet froze when I opened it, and my key [redacted:openai-key] was on screen

## What came with it

- Screenshot: **missing, and he did not remove it** — a fault: "Screen Recording
  is not allowed for Arbos". This line read "he removed it" until it was
  corrected by hand: `chose.screenshot` is `true`, so it was kept and the
  capture failed. That inversion is the one QA filed, since fixed at the source.
- Trajectory: 3 lines, 1 tool call, 1 failed (turn 112–140)
- Kernel log: 1 line
- Transcript tail: 0 lines
- The app's own view: included
- Tool arguments and outputs: included

**1 credential removed** — kernel `{"secrets": 0, "tokens": 1, "values": 0, "blocks": 0}`, app `{}`.

## The calls that failed

- `bash` — exit 101

## What it was

_to fill in: the cause, not the symptom_

## Which build carries the fix

_written by `desktop-feedback.py fixed` once the pull request has merged green_
