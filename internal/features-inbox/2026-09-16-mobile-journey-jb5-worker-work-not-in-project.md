---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# JB-5 — a worker's report that the project's repo does not bear out (kernel)

Named by the phone journey after two runs in a row on `arboslife/demo` (kernel `efcab58`+, restarted 19:15 UTC). Evidence: `media/mobile/journey/0916-200557/` and `0916-202247/` (transcript tails), `internal/mobile-journey-runs.md`.

## What happens

QA's challenge (`docs/acceptance-journeys.md` J2/J7): fix a failing test in a small seeded repo, add functions and tests, write a CHANGELOG, commit on a branch that is not `main`. The root spawns one worker (`wait: true`).

1. **The worker's worktree starts empty of the project** (M-111): the root cannot commit the seed on `main` (its git guard refuses), so the seed sits on `fix/<id>_initial_setup`; the worktree the spawn makes branches from `main`, which has nothing. Run 16's worker said so ("only README.md"); run 20's worker *copied the files in from the previous run's folder* to have something to work on.
2. **The worker's work stays in its worktree.** It reports a branch (`fix/mathlib-enhancements`, `fix/test-import`), passing tests and a CHANGELOG carrying the requester line. In the project's own repo — `git branch`, `git show <branch>:CHANGELOG.md` — none of it exists. The root then either redoes the work in the checkout (runs 19, 20) or, in run 18, the worker committed straight on `main` (M-114) — the one time the work *was* visible, it was in the wrong place.
3. The user sees a confident "done" and a repo that has not moved.

## Ask

One of: the worktree branches from the parent's current `HEAD` and its branch is a ref in the project repo the root can `git show`; or the spawn brief tells the worker where its branch must land and the root verifies before saying done. Either way the worker's report and `git branch` in the project must agree. QA's Linux rig may see the same shape — its J7 reads the disk directly and would say.
