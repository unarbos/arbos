---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# Two asks from Jacob's build-1021 feedback

## Voice gateway — dictation must never become a turn (F18, urgent)

`session.start {mode: "dictation"}` is the composer's microphone: the phone wants `transcript.delta`/`transcript.final` and nothing else. Since PR #56 the answerer routing also runs on dictation finals: a note dictated on the phone in **any** project is sent to the gateway's own kernel as a user turn and answered there. Proven 2026-09-16 13:52 UTC: dictate in `demo`, send nothing → a new turn "please summarize what the workers did today…" appears in `pod` with a reply (`media/mobile/cycle-14/04-`, `05-`). Jacob saw the loop's own test phrase in his `pod` chat (feedback `2026-09-16-21`).

Ask: in dictation mode, never route to the kernel and never answer, whatever `answerer` says. The app now sends `answerer: "model"` with dictation as a stopgap (`pod` stays quiet, `06-`), but the mode should be enough on its own.

## Kernel — a short `output` on the tool record (F14)

The phone folds tool calls into one line that opens on a tap; opened, it can show only labels, because the `tool` transcript record carries `name`, `args`, `paths`, `error`, timings — no output. Ask: add `output` (first ~400 chars, or the last lines) to the tool record and its history replay, so "Issue seeing text output" has an answer on the phone.
