---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# A worker the coordinator stopped is filed as "Turn ended badly"

Found by the desktop symmetry loop, cycle 77, long-form d43 (`/tmp/d41b-proj`,
`media/cursor-reference/cycle-77/d43/`), kernel
`arbos-kernel 0.2.0 e504bf2f48ad protocol 1`, model `google/gemini-2.5-flash`.

## What happened

The person asked the root to archive a live worker. The root called `say`
with a stop on it. The worker's done file, as it landed in the root's chat:

```
Turn ended badly. Last words: stopped: stopped by root: Stop
Last words before the stop: I need to provide a `task` or `brief` argument to the `spawn` tool. …
(transcript: .arbos/agents/long-running-command/transcript.jsonl)
```

`plan.rs::turn_outcome` returns `ok = false` for every `Interrupted` that
is not the user's (`user_stopped(&why)`), so the opening is
`inbox::DONE_ENDED_BADLY`. A stop the coordinator chose is not a failure of
the worker's turn; the person, who asked for it, read *ended badly* in red.

## What the desktop does now

Reads the words after the prefix: a report opening `stopped:` draws as
*<worker> stopped — by root: Stop*, muted (F-249). It is a read of the
words, not of the verdict.

## What the kernel could do

- A fifth opening in `inbox::DONE_PREFIXES` for a stop by another agent —
  *Turn stopped by <agent>. Last words: …* — beside `DONE_USER_STOP`, with
  `ok` neither true nor a failure; both readers (desktop, phone) take the
  list from `arbos_core`.
- Or fold it into `DONE_USER_STOP`'s shape with the stopper named.

## Where it is

`crates/arbos-kernel/src/plan.rs` — `turn_outcome`, the `opening` match;
`crates/arbos-core/src/inbox.rs` — `DONE_PREFIXES`.
