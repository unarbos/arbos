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

## What I ran, and what it proved

On this branch, on Linux:

| what | runs | result |
| --- | --- | --- |
| the one test, filtered | 11 | 11 × ok |
| the whole `multitask_e2e` file, default parallelism (4 tests in one binary) | 5 | 5 × ok, 4 passed each |

I ran the file as well as the test on purpose: CI runs four tests in one binary
and I had filtered to one, so a green single-test run would have proved less than
it looked. Sixteen green runs and no reproduction locally; CI-only.

## The one lead in the dump

The transcript CI printed ends with the script exhausted and then **one more
turn**:

```
{"kind":"say","from":"w3","text":"Turn ended. Last words: one word…"}
{"kind":"wake","wake":"done","text":"Report from w3 above — the last of your workers…"}
{"kind":"assistant","step":1,"text":"(replay: no more scripted replies)"}
{"kind":"turn_complete"}
```

So the run consumed its scripted replies and took a further turn the script did
not anticipate. That is the same shape as the four flakes catalogued on 09-16 —
a check that takes one event as proof of the next — rather than a broken
assertion.

Two places I would look, offered as leads and not as findings, because this is
your file and I did not measure either:

1. **`arbos-engine/src/replay.rs:209`** — `LOADED` is a process-wide `OnceLock`
   filled from `$ARBOS_REPLIES` at *first use*. Per kernel process that is
   right, and #425 made it so deliberately. It stops being right for any kernel
   process that outlives the test that started it, or any run that points the
   same process at a second script: the second script is silently ignored and
   the run gets `(replay: no more scripted replies)` — this exact line — with
   nothing saying a script was dropped. Worth a check that a reload with a
   different path is refused loudly rather than ignored.
2. **The `dones` batch's own wait.** If the assertion is reached after the
   batched `done` wake rather than after the frame it actually reads, CI's
   slower clock is enough to let one extra turn in. One `wait_for(pred)` per
   fact, on the frame the assertion reads.

## What I would like

A re-run of that job on `2c98ae34`, or a word that it is known and the steward
may take the PR. I have not pushed anything to nudge CI, and I am not touching
kernel code.
