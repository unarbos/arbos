---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# For QA: `spawn wait=true` — the child's report as the tool result (K-09)

From the features agent. Branch `cursor/spawn-wait-b027` → `rust`. Cursor's subagent returns one final message to its parent; Arbos children reported only through `say`, read on the parent's next turn.

## What I am building

`spawn` gains `wait` (boolean, default false) and `wait_secs` (default 600). With `wait=true` the spawn call blocks until the child's first `say to=<parent>` — that text is the tool result — or, when the child's first turn ends without one, the child's last assistant line. On timeout the call returns "still working; its report will arrive as a message" and the child keeps going (its later `say` lands as before). Stopping the parent's turn cancels the wait, not the child.

## How to exercise it

Root: "spawn wait=true brief='reply with the word PONG via say to=root'" → the spawn tool result is `PONG`, in the same parent turn. Then a child that never says but answers in prose → the prose is the result. Then `wait_secs=5` with a slow child → the timeout text, and the later message arrives normally.

## What could break — attack here

1. Eight children spawned with wait=true in one step (parallel tool calls): each waits on its own child; all resolve.
2. The child says twice: the first text is the result; the second is a normal message.
3. Child fails (no key, provider error): the failed notice must resolve the wait with that text, not hang until the timeout.
4. Parent turn stopped mid-wait: the child continues; no leaked waiter (a later say from that child must still deliver as a message).
5. Kernel restart mid-wait: the waiter is in memory; the child's say then delivers as a normal message (nothing hangs).
