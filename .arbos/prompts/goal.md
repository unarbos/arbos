---
description: Pursue a goal end-to-end autonomously — decompose, execute, verify, report
argument-hint: <goal>
---

Process the following as a goal, not a task list. Work autonomously end-to-end: do not hand control back until the goal is achieved and verified, or you are genuinely blocked on a decision only the user can make.

<goal>
$ARGUMENTS
</goal>

## Operating rules

**Understand first.** Restate the goal as a falsifiable definition of done — what must be observably true when you finish. Explore whatever code, docs, or systems you need to remove ambiguity. If two readings of the goal lead to materially different work, pick the most plausible one, note the assumption, and continue; do not stall on questions you can resolve yourself.

**Decompose into milestones.** Break the goal into the smallest sequence of independently verifiable milestones. Order them so each leaves the system in a working state. Track them as you go (plan/todo tooling if available, otherwise a simple list you maintain in your replies).

**Execute relentlessly.** Work milestone by milestone. After each one, verify it actually holds — run the build, run the tests, exercise the behavior — before moving on. When something fails, debug it from evidence rather than retrying blindly. Prefer the smallest change that genuinely achieves the milestone; no drive-by refactors.

**Self-correct.** If a milestone proves wrong or the decomposition was bad, revise the plan and say so in one line. Backtracking early is cheaper than building on a wrong foundation.

**Finish properly.** Done means: definition of done holds, the tree builds, tests pass, and nothing is left half-wired. Then report concisely:

- what was done (by milestone)
- how it was verified (commands run, behavior observed)
- assumptions made and anything deliberately left out

If hard-blocked, report exactly what is blocked, why, what you tried, and the single decision or input you need to continue.
