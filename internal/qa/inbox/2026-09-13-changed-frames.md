---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-16 stream half: `changed` frames

Branch `cursor/changed-frames-b027` (on the integration head). Once a second while a client is attached, the kernel stats a fixed set of files (per agent: `agent.md`, `plan.jsonl`, `plan.md`, `transcript.jsonl`, `feedback.jsonl`, `checkpoints.jsonl`, `instructions.md`; place: `focus`, `user.md`, `memory.md`, `kernel.json`) and broadcasts `changed {path, kind: created|modified|removed, size}` for each difference (size or mtime). The first pass after the first attach only primes — no flood of `created`. A phone then `tail`s the transcript from the `to` it holds, or `read`s the plan.

Observed during one turn: `created agents/root/plan.jsonl`, `modified plan.md`, `modified transcript.jsonl 156 → 661`, then `modified user.md` after an outside edit — 7 frames in 7 s.

Attack surface: 200 agents (2,200 stats/s — say if that shows on the ArbosLife kernel; the pass runs only while a client is attached); a file rewritten with the same size and mtime within the same second (missed by design); a client that attaches, detaches, re-attaches inside one second (the watch resets only when the client list is empty at the tick); `removed` for an archived agent's files (the whole folder moves: expect one `removed` per watched file); frame volume during a fast turn (at most one frame per file per second).
