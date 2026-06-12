---
description: Run something on a recurring schedule until cancelled
argument-hint: <what to do> every <period>
---

<task>
$ARGUMENTS
</task>

Compile this into a recurring plan node, then end the turn — the scheduler owns the cadence. Never hold the turn open, sleep, or poll to reach the next run.

- Parse the period from the message. If no period is stated or implied, ask the user before creating anything. If the tool rejects it as finer than the scheduler's floor, relay the floor and use it (or a background loop job for purely mechanical sub-minute work).
- Choose the executor by what each firing needs: a fixed command is when:{every} do:{shell}; a fixed message is do:{notify}; anything that must compose text or weigh a result each firing is an agent task (do omitted). Mechanical prep that gates a judgment finish belongs in when.condition on the poll cadence.
- Create the node with the plan tool, confirm it renders armed in the echoed forest, and report the node id and when it first fires.
