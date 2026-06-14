---
description: Start a Ralph loop — design a relentless self-improving loop, plant it in the plan forest, and let the scheduler run it forever
argument-hint: <goal>
---

Start a Ralph loop for this goal. A Ralph loop is relentless, explorative iteration toward one goal: each pass does a little real work, checks it, writes down what it learned, refreshes a live view of progress, and repeats — forever, until the goal demonstrably holds. You design the loop ONCE, plant it in the plan forest, and end the turn. The scheduler owns the cadence from then on; it survives restarts, interrupts, and session ends. Never build it on ad-hoc files, and never hold this turn open to run it yourself.

**Order is non-negotiable: think (Phase 0–1), then plant the forest (Phase 2), then — and only then — touch any artifact (Phase 3–4).** Do not write or run a single script, file, or command before `plan op:add` has created the root and the driver. If you catch yourself building or running anything while the forest does not yet exist, stop and plant it first. The forest is the deliverable; scripts, state files, and the canvas are things the *planted* loop operates on, not work you do ahead of it. A turn that ran the heartbeat but never called `plan op:add` has failed, no matter how much real work it did.

<goal>
$ARGUMENTS
</goal>

The heartbeat of every iteration is fixed — design everything else around it:

1. **Do work** — make the single smallest change worth trying this pass.
2. **Check work** — run the verification; did it help, hold, or regress?
3. **Write state** — record the outcome as the loop's memory for the next pass.
4. **Refresh the canvas** — update the live view of what has been tried and how it's going.
5. **Repeat** — the recurrence fires the next pass; the loop ends only when done holds.

## Phase 0 — Think first: design the best loop

Before creating anything, spend real thought on how to make THIS loop good. A Ralph loop is a search, not a checklist — the quality of the loop is the quality of the search. Decide deliberately:

- **What is being optimized**, and the one command or criterion that measures it. This is the loop's compass; everything compares against it.
- **The unit of one iteration** — the smallest do→check cycle that leaves a comparable result. Too big and the loop crawls; too small and it thrashes.
- **Mechanical vs judgment.** Building, running, measuring, committing are mechanical → `do:{shell}` nodes the kernel runs with zero model turns, waking you only on failure. Proposing the next change and weighing the result is judgment → the agent firing. A long-running "do work" step (a training run, a build, a benchmark) is a `do:{shell}` node the kernel runs *between* firings — the next firing reads its result, it never watches it run.
- **Where state lives.** Each node's `last:` outcome is the working memory across firings (a fresh session each time — that line is all the next pass remembers). Durable detail (a leaderboard, a results log) lives in a repo file the loop appends to.
- **The canvas** — the living view of the work (Phase 3). Decide what a glance at it should answer: best-so-far, the metric over time, what each pass tried and whether it helped.
- **Cadence** — size `when:{every}` to one iteration's wall-clock (2m for small fixes, 10–15m for heavy builds; never finer than the scheduler's floor — if the tool rejects the period, use the floor it reports).

Be explorative on purpose. Each pass should try a *real* variation — not the same safe tweak — keep it only if it beats the best, and record what was learned so a later pass never repeats a dead end. The recorded outcomes are your experiment log.

## Phase 1 — Synthesize the goal

Restate the user's words as a falsifiable definition of done: what must be observably true when the loop may stop, and the command or criterion that proves it. If the goal is ambiguous in a way that materially changes the work, add a `do:{ask}` node for that question — do not stall the whole loop on it.

## Phase 2 — Build the plan forest (do this before writing or running anything)

This is the first action that changes the world. Nothing in Phase 0–1 created a file or ran a command — it was all thought. Now, before you write a single script, render a canvas, or run a verifier, plant the forest. With `plan op:add parent:0`, create the mission root (the goal, with its definition-of-done as `check`), then arm the loop as ONE recurring agent node under it — the Ralph driver:

- `when:{every:"<cadence>"}`, goal: "Ralph driver: run one iteration of the heartbeat — do the smallest worthwhile change, check it, write state, refresh the canvas, record the outcome. Keep a change only if it beats the best; explore a new variation each pass and never repeat a recorded dead end. Add discovered work as sibling nodes. When the root's check holds, mark it done, cancel this driver, and notify."
- Route the heavy or fixed steps out of the firing: a long or mechanical "do work"/"check" step becomes a `do:{shell}` node so it costs no model turn and wakes you only on failure. The firing is for judgment — choosing the change and reading the result.
- Last sibling: a `do:{notify}` reporter so the user hears about a milestone (e.g. a new best) exactly once, without watching. Keep it quiet — ping on progress worth interrupting for, not every pass.

Each firing is one iteration in a fresh session. The `<<plan>>` block and each node's `last:` outcome are the iteration's entire memory — write outcomes as messages to the next iteration: the current best, what this pass changed, the measured result, and what to try (or avoid) next.

## Phase 3 — The canvas: a living view of the work

The loop maintains one self-contained HTML canvas of its own progress — `canvas/<goal-slug>.html` — and refreshes it every iteration. A glance should tell the story: the best result so far, the metric over time (a small inline SVG chart, drawn by hand), and a row per pass showing what it tried and whether it helped. Build it per the `/canvas` conventions — theme tokens (`var(--color-*, fallback)`), no external requests, no invented data; every number comes from the real state file.

Make the refresh mechanical so it costs no model turn: write a tiny render step that reads the state file (the leaderboard/results log) and emits the HTML, and run it as a `do:{shell}` node in the heartbeat. Call the `show` tool once, on the first iteration, to open the canvas in a panel beside the chat; the panel re-fetches the file live, so every later rewrite updates the view on its own.

## Phase 4 — Run the first iteration now (only after the forest is planted)

The forest from Phase 2 now exists; the scripts it references may not yet — that is fine, you write them here as part of this first pass. Do not wait for the first firing: execute one full heartbeat in this turn — do the first real change, check it, write the first state, render and `show` the canvas, record the outcome on the driver. Then confirm from the tool's echoed forest that the loop is sound: root with its check, the driver armed with its cadence and next-fire time, mechanical nodes in place. End the turn — the scheduler continues from there.

If the plan tool is unavailable in this session, say so in one line and run the loop in-turn instead — iterate the heartbeat here until done or blocked. Never degrade to ad-hoc state files or a promise that future invocations will happen.

## Iteration discipline (applies to every firing)

- One variation per iteration. Smallest change that tests it. No scope creep.
- Never leave the tree red: a failed check means fix or revert before marking the pass done.
- Record an outcome on every touched node — it is the loop's memory and its experiment log. Include the values the next pass must compare against.
- Explore, don't circle: read the recorded outcomes and try something genuinely new each pass; a variation that already failed is not a candidate.
- Refresh the canvas every pass so the view never lies about the current state.
- Blocked is not stopped: mark the node blocked with what is needed, move to the next pending one.
- The loop ends only two ways: the root's check holds (mark root done, cancel the driver, notify), or every remaining path is blocked on the user (the ask nodes say exactly what is needed).
