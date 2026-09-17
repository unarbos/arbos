# qal-j04: the coordinator's brief names the user's deliverable under `.arbos/docs/`, and the kernel's nudge makes the worker move it out of the repo

- Feature: the spawn brief's `output` field and the "Output … not written yet" nudge (coordinator protocol, `.arbos/docs/` as the home of a worker's outputs); `main` @ `efcab58f` and #329's branch alike, model `google/gemini-2.5-flash`
- Severity: high for the thing Jacob does: he asks for a file in his project, the worker writes it in the right place, and then the kernel talks the worker into moving it into `.arbos/docs/`, where the user never looks and where `git` does not see it. The tests pass, the branch has its commit, and the CHANGELOG the user asked for is gone from the repo.
- Journey step: **J7 (the result on disk)** — `no CHANGELOG.md`, failed in two consecutive runs (`20260916T163147Z-journey-linux`, `20260916T172422Z-journey-linux`), the brief-and-move shape visible in a third (`20260916T162657Z`, where the worker copied instead of moving so J7 passed by luck). Named by the journey's twice rule.

## Repro

Fresh project (`shapes/`, a failing test, no CHANGELOG). User asks root: *"Fix the failing test in this project, add a CHANGELOG.md entry describing the fix, run the tests to prove they pass, and commit the work on a new branch. Tell me the branch name and the last line of the test output."*

Root spawns one worker. Its brief (`spawn` args, run `172422Z`):

```
"task":   "Fix the failing test, add a CHANGELOG.md entry, and prove it passes."
"output": ".arbos/docs/CHANGELOG.md"
"read_first": ".arbos/docs/project-context.md"
```

The worker does the job right — edits `shapes/geometry.py`, **writes `CHANGELOG.md` at the project root**, runs the tests (`OK`), commits on `fix/geometry-area-calculation`, reports. Then:

```
50 nudge      Your brief names Output: .arbos/docs/CHANGELOG.md — not written yet. A reply is not the deliverable: write the …
51 assistant  I apologize for missing the `CHANGELOG.md` output requirement. I will move the created `CHANGELOG.md` to `.arbos/docs/CHANGELOG.md`.
54 tool bash  (mv CHANGELOG.md .arbos/docs/CHANGELOG.md)
55 assistant  I have moved the `CHANGELOG.md` file to `.arbos/docs/CHANGELOG.md`.
```

On disk afterwards: `.arbos/docs/CHANGELOG.md` exists, the repo has no `CHANGELOG.md`, and the commit on the branch predates the move (the branch is fine, the working tree is not). Root then tells the user "I also added a `CHANGELOG.md`".

## Expected

A file the user asked for in their project is a repo deliverable and stays where the user asked for it. `output` in a brief is the place for the worker's *report or notes* (`.arbos/docs/<something>.md`), not for a file whose location the task itself fixes. Either the coordinator must not name a repo file as `output` under `.arbos/docs/`, or the nudge must accept the deliverable where the task put it (the brief's own `task` says "add a CHANGELOG.md entry", and the file exists), rather than insist on the `.arbos/docs/` path.

## Actual

The nudge enforces the literal `output` path; the model obeys and moves the file. Two rules the kernel gives the worker contradict each other and the user loses.

## Suspected location

- The coordinator prompt / `spawn` tool guidance (`crates/arbos-engine/src/prompt.rs`, `crates/arbos-kernel/src/tools.rs` `Spawn`): what `output` is for — steer it to reports, and tell the coordinator that files the user asked for in the project are not `output`.
- The "Output … not written yet" nudge (`crates/arbos-engine/src/turn.rs` or wherever the brief's outputs are checked at turn end): when the named path is under `.arbos/docs/` but a file of the same name exists in the project, treat the deliverable as written and say so, instead of nudging a move.

## Fix

Not started. Regression check: `journey-linux` J7 (`CHANGELOG.md` at the project root after the run, `changelog: true`) and a kernel e2e: a brief with `output: .arbos/docs/X.md` and a task that writes `X.md` in the project must end without the "not written yet" nudge, and `X.md` must still be in the project at turn end.
