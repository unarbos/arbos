---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# The coordinator sleeps to wait on its workers, and their reports queue behind its own sleep

**From:** Jacob's desktop feedback `2026-09-17-6` / `-7` ("Never get response from the 3 subagents."), build 0.2.0 (1335), kernel `f9b6089c0b80`, model `anthropic/claude-opus-5` standing in for a failing `inception/mercury-2.5`. Report and trajectory: `media/desktop-feedback/2026-09-17-6/`.

## What the trajectory shows (quoted)

Root's turn, 09:42:23–:

```
09:42:53  tool spawn ×3   (quicksort, mergesort, heapsort workers)
09:42:57  tool status     {"step": "Waiting on three sorting workers"}
09:43:00  tool agents     (all three running)
09:43:11  thinking        "With all three agents still running, I don't have a clean way to wait
                           without polling in a loop, which I should avoid. It's better to just end
                           this turn and let their completion reports come through n[ext]…"
09:44:08  tool bash       {"command": "sleep 75; echo waited",
                           "description": "Give the three workers time to finish", "wait_ms": 120000}
09:44:08  user            where is my response
```

The workers' `turn_complete` lines: 09:43:11, 09:43:12, 09:43:18 — all three were done **before** the coordinator started its sleep. Their `done` wakes sat in root's inbox behind the running turn until the sleep returned.

## Why it matters

The model knew the right move (end the turn; the reports wake it) and did the wrong one anyway. Every second of `sleep` is a second the person sees *Waiting on three sorting workers* over three *Done* lines. The desktop half — showing the running command instead of the stale status — is in the cycle-33 PR, but the honest line *Running sleep 75; echo waited* is still a coordinator sitting on its own reports.

## Asks

1. **Refuse or cap a bare `sleep` in the coordinator's `bash`** when it has live children — or make `bash` yield to a child's `done` wake the way #362 made it yield to the user's words within two seconds. The wake is the event it is waiting for; the sleep is a poll by another name.
2. **The prompt:** the rule "end the turn; a worker's report wakes you" is in the model's own thinking here, so the prompt says it; what it lacks is the prohibition — "never run `sleep` to wait on a worker".
3. Smaller: when a `status` step names workers and every named worker has since finished, the kernel could clear or replace the step (the `waiting on <worker>` line of #366 already knows when no worker is live).

## Also in the same still (not this note's ask)

A **Delegate 1 · Working** row in the panel and the Working card, and *1 Working Delegate 1* in the transcript — with no such agent in the kernel's roster (`agents` in the report lists root, the three workers, the guide, the archived research, and a parentless `chat-1789473184580`). A desktop-side phantom, F-137 in the symmetry ledger; open until his `.arbos/desktop/sessions/` can be read.
