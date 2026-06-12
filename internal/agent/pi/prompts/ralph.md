---
description: Start a Ralph loop — a durable, scheduler-driven iteration on one goal until it is done
argument-hint: <goal>
---

Start a Ralph loop for this goal. A Ralph loop is relentless iteration: one small verified step per iteration, the plan as the only memory, no stopping until the goal demonstrably holds. Build it on the plan tool — NOT on ad-hoc files and NOT by holding this turn open forever. The scheduler owns the loop; it survives restarts, interrupts, and session ends.

<goal>
$ARGUMENTS
</goal>

## Phase 1 — Synthesize the goal

Restate the user's words as a falsifiable definition of done: what must be observably true when the loop may stop, and the command or criterion that proves it. If the goal is ambiguous in a way that materially changes the work, add a `do:{ask}` node for that question — do not stall the whole loop on it.

## Phase 2 — Build the hierarchical plan

With `plan op:add parent:0`, create the mission root (the goal, with its definition-of-done as `check`) and decompose it into children, each small enough to finish and verify in one iteration:

- Judgment work → plain agent task nodes, each with a `check` when one is stateable.
- Mechanical steps (build, test, lint, commit) → `do:{shell}` nodes; the kernel runs them with zero model turns and wakes you only on failure.
- Questions only the user can answer → `do:{ask}` nodes.
- Last sibling: a plain `do:{notify:"ralph: <goal> complete"}` node so the user gets pinged exactly once — mechanical nodes fire when everything before them is done; do not add `when:{onDeps}` (the tool rejects it on non-agent nodes).

## Phase 3 — Arm the driver (this is the loop)

Add ONE recurring agent node under the root — the Ralph driver:

- `when:{every:"5m"}` (pick the cadence by task weight: 2m for small fixes, 10–15m for heavy builds; never finer than the scheduler's floor — if the tool rejects the period, use the floor it reports).
- Goal: "Ralph driver: run one iteration — pick the highest-priority pending task under root #N, claim it, complete it with the smallest change, verify its check, record the outcome. Add newly discovered work as sibling nodes. If every remaining task is blocked or done, mark the root accordingly and cancel this driver." Substitute the real root id from Phase 2's add result for #N.

Each firing is one Ralph iteration in a fresh session. The `<<plan>>` block and each node's `last:` outcome line are the iteration's entire memory — write outcomes as messages to the next iteration (what changed, what was verified, what to do next).

## Phase 4 — Run the first iteration now

Do not wait for the first firing: execute one iteration in this turn (claim → smallest change → verify → done with outcome). Before ending the turn, confirm the forest is sound from the tool's echoed rendering: root with its check, pending children, driver armed with its cadence. Then end the turn — the scheduler continues from there.

If the plan tool is unavailable in this session, say so in one line and run the loop in-turn instead — iterate milestone by milestone here until done or blocked. Never degrade to ad-hoc state files or a promise that future invocations will happen.

## Iteration discipline (applies to every firing)

- One node per iteration. Smallest change that completes it. No scope creep.
- Never leave the tree red: verification failing means fix or revert before marking done.
- Record an outcome on every touched node — it is the loop's memory.
- Blocked is not stopped: mark the node blocked with what is needed, move to the next pending one.
- The loop ends only two ways: the root's check holds (mark root done, cancel the driver, notify), or every remaining node is blocked on the user (the ask nodes say exactly what is needed).
