---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j47: one thing to check in the failing rollouts before bisecting — did the turn still exist when the follow-up was queued?

**For:** QA, and whoever picks up `qal-j47`.
**From:** the features agent (kernel), 2026-09-19 01:20 UTC. Not a fix; a reading of the window you named.

Your kernel-commit window (`232518c2` → `55c82877`) holds two commits that change *how long a turn lives*, and they sit right at your "clean at 14:59" boundary: `a47c5104` (15:05, *Jev fail ends the turn; the chat model does not run*) and `2d5cad97` (15:34, the Jev slug remap). Since then every turn on a kernel with a key asks the Jev controller first, and **if that call fails — error, timeout, 429, or junk — the turn ends in the open with no chat-model fallback** (Jacob's decision, in the commit message; the notice reads *Jev did not choose the next step: … The turn stopped. The chat model did not run.*).

`sq-02` queues its follow-up *while a turn runs*. If the running turn has already ended on a Jev failure, the queued line is not queued — there is nothing to queue behind — it opens its own turn at once, and no inbox row ever appears. That is the exact symptom, and it would be a **rate**: it follows the controller's availability (`qal-j45`'s hour of 429s, or the rig's key not having the `~typesafe/jev-latest` route), not a code switch. It would also explain why an older kernel passes beside it under the same conditions: without the fail-fast, its turn lives through the chat-model call and the queue has something to wait behind.

**The one look that decides it:** in a failing rollout's root transcript, is there a `notice` containing `Jev did not choose the next step` (or a `turn_complete` within a second or two of the prompt) *before* the follow-up was sent? If yes, `qal-j47` is the Jev fail-fast meeting a failing controller on the rig — product behaviour Jacob chose, plus an environment fault — and the queue path is fine; the fix is on the rig (a key/route that reaches Jev, or `jev = false` in the rig's `config.toml` for scenarios that need a turn to stay up). If no — the turn was still running and the row still did not appear — it is a real queue regression, `#692`'s attach change is the right first suspect, and I will take it.

`jev = false` in the kernel's `config.toml` restores the one-model loop for a control run either way.
