---
description: Research the task and produce a detailed implementation plan before writing any code
argument-hint: <task to plan>
---

Enter plan mode for the following task. Do NOT write or edit any source code yet — the deliverable of this turn is a plan, not an implementation.

<task>
$ARGUMENTS
</task>

Work in two phases:

## Phase 1 — Research

- Explore the codebase until you understand every part the task touches: entry points, data structures, existing patterns, and the seams where the change must land.
- Read the actual files; do not plan from assumptions. Cite real paths and line numbers.
- Identify constraints: invariants the code maintains, conventions the project follows, tests that pin current behavior.
- If the task is ambiguous in a way that materially changes the design, list the open questions instead of guessing silently.

## Phase 2 — Plan

Write the plan to `plans/<short-task-slug>.md` with this structure:

1. **Context** — what exists today and why the change is needed (2–4 sentences).
2. **Approach** — the chosen design and, briefly, why it beats the obvious alternative.
3. **Data structures first** — any new or changed types/schemas, defined before behavior.
4. **Changes by file** — an ordered list; for each file: what changes, and a short sketch of the new code where it clarifies.
5. **Verification** — how to prove it works: commands to run, behavior to observe, edge cases to check.
6. **Risks and open questions** — anything that could invalidate the plan.

Order the steps so each one leaves the tree buildable. Prefer the smallest plan that fully solves the task; flag any scope you deliberately cut.

When the plan file is written, summarize it in a few bullets and stop. Wait for approval before implementing.
