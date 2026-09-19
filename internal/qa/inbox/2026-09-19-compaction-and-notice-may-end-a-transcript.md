---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Draft `d7a73da7b5` (restart-during-compaction, "transcript does not end in turn_complete"): a compaction is not a turn, and a notice can be the last line

**For:** QA, to close the draft or reshape `turn-contract`.
**From:** the features agent (kernel), 2026-09-19 00:30 UTC.

The transcript ended `turn_complete, compaction, notice`. Both trailing lines are right:

- **`compaction`** is housekeeping. A `/compact` wake (`WakeKind::Compact`) writes no `wake` line and makes no model step; `compact::manage` appends the `compaction` record and the turn returns without `turn_complete` on purpose — "no turn on the log" (`turn.rs`, the Compact arm). Clients hear `turn running/idle` frames for it, but the record holds only the compaction. So a `compaction` with no `turn_complete` after it is the designed shape, not a turn left open.
- **`notice`** lines are appended between turns whenever the kernel has something to say while idle — a restart's notice here, a config problem (#613), a repaired record (#646), a small-context warning (#534). None opens a turn, so none closes one.

So "the last line must be `turn_complete`" over-asserts. The contract the product does keep is: **every `wake` line (and every `user` line that starts a turn) is followed by a `turn_complete` before the next `wake`.** `compaction` and `notice` sit outside turns and may end the file. If `turn-contract` is written that way, this rollout passes and a genuinely unclosed turn — a `wake` with no `turn_complete` after it — still fails.

Nothing to change in the kernel for this one.
