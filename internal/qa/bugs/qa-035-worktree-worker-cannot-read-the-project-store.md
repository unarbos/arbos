---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qa-035: a worktree worker's `read` of the project store is refused as "outside the workspace"

- Feature: `spawn isolate=worktree` + K-01b cwd confinement (#17); the kickoff's `read_first`
- Severity: medium — every worktree worker starts by failing its first two reads (`.arbos/docs/project-context.md`, `.arbos/notes.md`), which the brief told it to read first; its `write` of a deliverable under `.arbos/docs/` is refused the same way
- Found while verifying #195/#196 live (gpt-5.4-mini, 2026-09-14)
- **Fix: PR #197** (`fs::confine` admits the place's `.arbos/` from a worktree root, minus other worktrees; `resolve_write` checks the root-owned rules against the place). E2e `worktree_store_e2e`.

## Repro

1. A coordinator place with a git repo; root spawns a worker with `isolate:"worktree"` and the default kickoff (`read_first: .arbos/docs/project-context.md, .arbos/notes.md`).
2. The worker's transcript: `read /tmp/proj/.arbos/docs/project-context.md` → `… is outside the workspace /tmp/proj/.arbos/worktrees/<id>; file tools …`. Same for `.arbos/notes.md`. Later `write /tmp/proj/.arbos/docs/<name>.md` → same refusal.

## Expected

The confinement (K-01b) keeps a worktree child's file tools off the parent's *checkout*; the project store `.arbos/` (docs, notes, internal, media) is shared by design — the brief points at it, the output rules name it. Reads of `.arbos/**` and writes under `.arbos/docs|internal|media` should pass the confinement for a worktree child (the write guard's store rules still apply). Note the worktree itself lives under `.arbos/worktrees/`, so the carve-out must exclude other workers' worktrees.

## Where

`arbos_engine::PlanCx::resolve_*` / `RunCx::root` (K-01b, #17): the root for a worktree child is the worktree path; the store path is not an allowed second root.
