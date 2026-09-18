---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

# Three kernel e2e tests failing in two hours, none of them a branch's fault

Written by the iPhone loop (cycle 61) after being asked to fix
`kernel (build + test)` on `cursor/mobile-cycle-60-oldest-rows-a4fa`. There
is nothing on that branch to fix, and the reason is worth having written
down, because the same red will appear on the next branch that touches no
Rust at all.

## The branch cannot have caused it

`cursor/mobile-cycle-60-oldest-rows-a4fa` changes **no files under
`crates/`** — `git diff origin/main...<branch> -- crates/` is empty. Its Rust
tree is byte-identical to main's.

The same tree, on that same branch, across four runs:

| commit | kernel job |
|---|---|
| `590c0b17` | pass |
| `e84c1ced` | pass |
| `b5a6f070` | **fail** |
| `7dc2d40c` | pass |

One failure in four runs of an unchanged tree. Also: #543 merged at 01:26
(merge commit `53dffd33`), so `b5a6f070` was two commits stale when the red
was reported.

## It is not one test, and it is not one branch

Fifteen recent runs on **main** carry two failures, and neither is the test
that failed on the branch. Three different tests in about two hours:

| where | test |
|---|---|
| branch `b5a6f070` | `a_coordinator_that_spawns_and_leaves_the_page_alone_is_nudged` — `audit_kernel_2_e2e.rs:138` |
| main `dd7814fc` | `history_for_an_archived_worker_replays_its_record_and_says_so` — `history_archived_e2e.rs:42` |
| main `58105882` | `tools::git::tests::checkpoint_refs_are_dropped_by_line_and_by_agent` |

Two of the three are on main. So this is the kernel e2e suite, not anything
a client branch did.

## At least one is a staging timeout, not a behaviour failure

The archived-worker one panics on its **setup** assertion:

```
panicked at crates/arbos-kernel/tests/history_archived_e2e.rs:42:5:
w1 archived once root read its done
```

That is the test waiting for `w1` to become archived and giving up — the
behaviour under test is never reached. Worth knowing before anyone reads
these reds as the features being broken.

## Why the iPhone loop cares about that particular one

`history_for_an_archived_worker_replays_its_record_and_says_so` is about
exactly the question this loop reopened tonight.

Cycle 46 concluded (M-153) that a finished worker's chat is empty because
the kernel has nothing for it, and a features ask was filed on that. Cycle
61 found the measurement behind it was unsound — `kernel.py total <agent>`
was printing the **root's** count for every agent asked about (M-224) — and
re-measured: a live worker's chat does have content, 8 lines, and the app
draws it (M-225). Whether an *archived* worker keeps its transcript went
back to unknown, because the original sample project now has zero children.

The existence of this test says the kernel side intends archived history to
replay. That is useful context for whoever picks up
`internal/features-inbox/2026-09-16-mobile-worker-history-archived-agents.md`,
which now carries a correction noting its evidence was unsound.

## What this loop did and did not do

Did not touch the kernel tests. They are not this loop's, the branch in
question is clean, and a phone loop pushing timing changes into another
team's e2e suite is how a flake becomes two flakes.

Recorded as M-215 in `internal/mobile-findings.md` at the first sighting and
extended here once the pattern across main was visible.

## Added 02:22 — the same commit passing and failing

Asked a second time to fix the job, I looked for the one piece of evidence
that ends the question: does the **identical commit** ever go both ways?

It does, twice, on `main`:

| commit | runs |
|---|---|
| `dd7814fc` | success, success, success, **failure** |
| `58105882` | success, success, **failure** |

Same code in, different answer out, repeatedly. That is a flake by
definition, and no change to any client branch can affect it.

`main` is currently green: `9bb49c61`, `e767633d` and `dd7814fc` all pass on
their latest runs.

**This loop cannot re-run the job either** — `gh` here is read-only:

```
run 35294320123 cannot be rerun; Resource not accessible by integration
```

So there is no action available to the iPhone loop on this: not a code fix
(the branch has no Rust), not a re-run (no permission), and not a change to
the tests (another team's suite, and three separate tests are involved).
What is available is this evidence, which is why it is written down.

## A fourth, 14:26 UTC — and this one names the test

[#659](https://github.com/unarbos/arbos/pull/659) changes one SwiftUI file
and nothing else. `kernel (build + test)` failed:

```
failures:
    a_coordinator_that_spawns_and_leaves_the_page_alone_is_nudged
test result: FAILED. 2 passed; 1 failed; 0 ignored; finished in 30.73s
```

Every other suite in the same run reported `0 failed`, and the iPhone build
passed. The test is a timing one — a nudge after a coordinator goes quiet —
and it took 30.73 s of a run that is otherwise seconds per suite.

[#658](https://github.com/unarbos/arbos/pull/658), pushed minutes apart and
also touching no Rust, passed the same job. Still not this loop's to fix;
recorded because the earlier entries here have the shape and not the name.

## A fifth, 22:35 UTC — a different e2e again

[#702](https://github.com/unarbos/arbos/pull/702) changes one bash file.
`kernel (build + test)` failed:

```
test a_person_watches_the_page_live_and_takes_the_wheel ... FAILED
thread '…' panicked at crates/arbos-kernel/tests/browser_takeover_e2e.rs:175:5
test result: FAILED. 0 passed; 1 failed; finished in 3.71s
```

Every other job in the run passed, including `macOS (check kernel +
desktop)`, which compiles the same crate. That is now five failures across
the day on branches touching no Rust, and **each one a different e2e test**:
the coordinator nudge, and now the browser takeover. A single flaky test
would repeat; five different ones point at the environment these e2e tests
run in rather than at any of them.

Still not this loop's to fix. Recorded because the pattern — different test
each time — is worth more to whoever does own it than another instance of
the same name.
