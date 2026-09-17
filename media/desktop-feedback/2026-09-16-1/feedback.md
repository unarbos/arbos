# 2026-09-16-1 — FIXTURE — The worker line says Starting forever. My key is [redacted:openai-key] and the token [redacted:github-token] too.

> **Not a real report. Do not diagnose from it, and do not quote it as
> something Jacob saw.**
>
> This is a smoke fixture. It went through the app's own writer, so it carries
> every mark of being genuine — but the words are invented and the events are
> from an unrelated test turn, so **the complaint and the trajectory do not
> correspond**. The transcript here is about running `cat hello.py`; nothing in
> it concerns a worker.
>
> That mismatch cost a real diagnosis: read cold as a debugging exercise, the
> bundle could not answer the complaint, and the gap was attributed to the
> bundle. The additions in [#360](https://github.com/unarbos/arbos/pull/360) —
> the roster, the key state, the place's settings — are right on their own
> merits, but the failure that motivated them was the fixture's, not the
> bundle's.
>
> Fixtures written from [#361](https://github.com/unarbos/arbos/pull/361)
> onwards mark themselves, in the report and in this summary. This one and
> `2026-09-16-2` predate that and are marked by hand.

- Sent: 2026-09-16 18:17 UTC
- App: 0.2.0 (1102) commit `a28eed4-dirty`
- Kernel: 0.2.0 `149543ae7a60` built 2026-09-16T17:32Z
- Machine: linux/x86_64, project `fb-proj`
- Model: openrouter google/gemini-2.5-flash
- Report id: `20260916T181732Z-307f`

## What he wrote

The worker line says Starting forever. My key is [redacted:openai-key] and the token [redacted:github-token] too.

## What came with it

- Screenshot: **missing, and not removed** — a fault: "the window is too large
  to send as one picture". That was F-101, the branch that fired for every
  Retina window; pictures are scaled to fit now rather than refused. This line
  read "he removed it" until it was corrected by hand — the fault-versus-choice
  inversion QA filed, since fixed at the source.
- Trajectory: 6 lines, 1 tool call, 0 failed (turn 10–15)
- Kernel log: **he removed it**
- Transcript tail: 9 lines
- The app's own view: included
- Tool arguments and outputs: included

## What it was

_to fill in: the cause, not the symptom_

## Which build carries the fix

_written by `desktop-feedback.py fixed` once the pull request has merged green_
