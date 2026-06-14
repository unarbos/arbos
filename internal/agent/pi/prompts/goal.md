---
description: Pursue a goal end-to-end autonomously — decompose, execute, verify, report
argument-hint: <goal>
---

Process the following as a goal, not a task list. Work autonomously end-to-end: do not hand control back until the goal is achieved and verified — or, when the goal lives beyond this turn, until the pipeline that achieves it is planted in the plan and armed. Stop early only if you are genuinely blocked on a decision only the user can make.

<goal>
$ARGUMENTS
</goal>

## Operating rules

**Understand first.** Restate the goal as a falsifiable definition of done — what must be observably true when you finish. Explore whatever code, docs, or systems you need to remove ambiguity. If two readings of the goal lead to materially different work, pick the most plausible one, note the assumption, and continue; do not stall on questions you can resolve yourself.

**Route by where done lives.** Decide whether the definition of done can hold by the end of this turn.

- *In-turn goal* ("optimize this", "fix the build"): work it here, milestone by milestone, per the rules below.
- *Beyond-turn goal* — done spans future invocations, time, or external events ("each time I'm called…", "count to N, one per call", "every hour…", "when X happens…", "keep doing Y until Z"): achieving it means compiling it into plan nodes. Create the (when × do) nodes, run any step that is due right now, verify the next firing is armed, and end the turn. The scheduler's firings are the future invocations; each node's recorded outcome is the state between them. Never simulate the future inside this turn (sleep, polling, await), never park state in ad-hoc files, and never hand the loop back to the user to re-invoke manually.

**Decompose into milestones.** Break the goal into the smallest sequence of independently verifiable milestones. Order them so each leaves the system in a working state. Track them in the plan tool when it is available — it is the first-class primitive for this and survives anything this turn does not — otherwise a simple list you maintain in your replies.

**Keep a living canvas.** Maintain one self-contained HTML view of progress at `canvas/<goal-slug>.html`, built per the `/canvas` conventions — theme tokens (`var(--color-*, fallback)`), inline CSS/JS, no external requests, no invented data; every number comes from real state. A glance should answer: the definition of done, the milestones and which hold, what was verified and how, and any open items. Refresh it as the work moves — after each milestone for an in-turn goal, and on every firing for a beyond-turn goal. Make the refresh mechanical so it costs no model turn: write a tiny render step that reads the real state (the plan, a results log, the repo) and emits the HTML, and for a beyond-turn goal run it as a `do:{shell}` node in the pipeline. Call the `show` tool once, the first time you write the file, to open it in a panel beside the chat; the panel re-fetches live, so every later rewrite updates the view on its own.

**Execute relentlessly.** Work milestone by milestone. After each one, verify it actually holds — run the build, run the tests, exercise the behavior — before moving on. When something fails, debug it from evidence rather than retrying blindly. Prefer the smallest change that genuinely achieves the milestone; no drive-by refactors.

**Self-correct.** If a milestone proves wrong or the decomposition was bad, revise the plan and say so in one line. Backtracking early is cheaper than building on a wrong foundation.

**Finish properly.** For an in-turn goal, done means: definition of done holds, the tree builds, tests pass, the canvas reflects the final state, and nothing is left half-wired. For a beyond-turn goal, finishing this turn means: the nodes exist and render armed, any step due now already ran, the canvas is rendered and shown, and the pipeline ends in a node that reports completion (a plain do:{notify} sibling, or a when:{onDeps} agent finisher) — the goal itself completes when the pipeline does. Then report concisely:

- what was done (by milestone)
- how it was verified (commands run, behavior observed)
- assumptions made and anything deliberately left out

If hard-blocked, report exactly what is blocked, why, what you tried, and the single decision or input you need to continue.
