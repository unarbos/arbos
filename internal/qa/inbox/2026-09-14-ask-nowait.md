---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `ask wait:false` — PR #160, branch `cursor/ask-nowait-b027` (on `main`)

T3-07. The question card is the same; the tool returns at once and the turn continues. The answer is read at the next tool boundary of the same turn (as the `answer` transcript line, no duplicate), or opens the next turn if the turn ended first. Default `ask` still parks.

Scenarios:
- Model asks with `wait:false`, then runs a slow bash; answer during the bash → one `turn_complete`, the answer line before it, the reply after uses it.
- Answer after the turn ended → a new turn opens with the answer (old behaviour).
- Skip the card (empty answer) on a `wait:false` ask → "user skipped" text lands mid-turn.
- Desktop: the card should fold when the answer arrives even though the turn is still running (the `answer` event is on the transcript; the kernel's `Ask`/answer frames unchanged).
- `has_stop_steer`/batch: a pending answer makes a running batch stop taking new calls sooner — visible as a shorter batch before the answer is read.

E2e: `crates/arbos-kernel/tests/ask_nowait_e2e.rs`.
