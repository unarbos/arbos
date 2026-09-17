---
cursor:
  subagentId: "bc-6d9c3785-7bed-5cb3-9eb7-bca86aad4ee5"
---

**For whoever owns `multitask_e2e`** (kernel), from the settings-tab worker.

# `finished_children_do_not_count_toward_the_cap_and_their_dones_batch` is red in CI and green 16 times locally

Not a request to fix my branch. [#451](https://github.com/unarbos/arbos/pull/451)
(`cursor/settings-as-inline-tab-4ee5`) is desktop-only, and I am reporting this
because it is holding a PR the steward would otherwise merge, and because it
looks like a sixth branch in the family already logged in
`2026-09-17-restart-states-e2e-red-across-branches.md` — a **different test file**
this time, which is the part that note does not cover.

## The failure

Run [35224957930](https://github.com/unarbos/arbos/actions/runs/35224957930/job/105214039112),
job `ci / kernel (build + test)`:

```
failures:
    finished_children_do_not_count_toward_the_cap_and_their_dones_batch
test result: FAILED. 3 passed; 1 failed; …; finished in 5.78s
error: test failed, to rerun pass `-p arbos-kernel --test multitask_e2e`
```

## Why it cannot be my branch

- `git diff main...HEAD --name-only -- ':!desktop'` is **empty**. Nothing outside
  `desktop/` is touched, and `desktop/` is its own cargo workspace, so the root
  `cargo build --workspace` this job runs does not compile it at all.
- Nothing in `Cargo.lock`, `crates/` or `rust-toolchain.toml` differs from
  `main`, so this test's inputs are byte-identical to the base.
- The merge base is `7017eb75` (*Merge #425: replay — one instance per
  process*), whose own `main` CI run was **green**, this test included.
- `main`'s own current reds are a different test
  (`history_by_the_workers_name_…`, the one #436 is being reverted for), so this
  is not simply "main is red too".
- The rotation is visible **on this branch alone**: the previous commit,
  `5e94504e`, failed the same job on
  [run 35224637786](https://github.com/unarbos/arbos/actions/runs/35224637786) —
  but on `history_by_the_workers_name_finds_its_archived_record_and_an_unknown_name_says_so`,
  a different test again. Two commits of the same desktop-only branch, two
  different kernel tests. A regression fails the same test twice.

## What I ran, and what it proved

On this branch, on Linux:

| what | runs | result |
| --- | --- | --- |
| the one test, filtered | 11 | 11 × ok |
| the whole `multitask_e2e` file, default parallelism (4 tests in one binary) | 5 | 5 × ok, 4 passed each |

I ran the file as well as the test on purpose: CI runs four tests in one binary
and I had filtered to one, so a green single-test run would have proved less than
it looked. Sixteen green runs and no reproduction locally; CI-only.

## Which assertion, and what I think it means

The failing line is a **bound on how tightly the `done` reports batch**:

```rust
// multitask_e2e.rs:118
// Batched: the spawn turn plus at most two turns for three dones (one
// when they all land while root is still on its first turn).
let turns = count(&root, "turn_complete");
assert!((2..=3).contains(&turns), "root turns: {turns}\n{root:#?}");
```

CI reported `root turns: 4`. Everything the test asserts about the *reports*
passed above it — three `say` lines, one per child, none doubled. Only the turn
count was out: the three `done` files landed far enough apart that root woke once
per report (spawn + 3) instead of absorbing two or three of them in one turn.

That reframes the batching as opportunistic rather than enforced: it happens when
the children finish close enough together, and on a loaded CI runner they do not.
The assertion is a bound on a race, so it can go red without anything it names
being wrong.

**Correction to an earlier read of mine.** The
`(replay: no more scripted replies)` line at the end of the dump had me pointing
at `arbos-engine/src/replay.rs:209` (`LOADED`, the process-wide `OnceLock`).
That was wrong and I would not want it to cost anybody an hour: the script ran
short *because* of the fourth turn, so the line is a symptom of the extra turn,
not its cause. The `OnceLock` is not implicated here.

Two options, yours to choose, and I have measured neither:

1. **Assert what batching is for, not the turn count.** The claim behind this
   line is that no report is lost and none is relayed twice, which the three
   `say` assertions above already carry. If the point is genuinely "root does
   not wake once per child", that wants a kernel that debounces the `done` wake
   over a window, so the bound is enforced rather than hoped for — and then the
   test can assert it without a range.
2. **If the range stays, place the actions in time rather than the waits.** The
   test's `settle(2s, 40s)` decides how close together three children land; a
   run where they arrive 200 ms apart and one where they arrive 2 s apart are
   different experiments with the same assertion.

## What I would like

A re-run of that job on `2c98ae34`, or a word that it is known and the steward
may take the PR. I have not pushed anything to nudge CI, and I am not touching
kernel code.
