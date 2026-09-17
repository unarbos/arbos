---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# The kernel CI job's reds on 2026-09-17: what each one means

Question asked: for each test that went red in the `kernel (build + test)` job today on a branch that could not affect it, does the failure mean the product is wrong?

Short answer: none of the five did. Four are tests that are right about intent and wrong about timing. One is a merge that took the wrong commit. Two carry a real lesson for how the suite is written.

## The five, one by one

| Test | Runs seen | What CI saw | What it means | Fix |
|---|---|---|---|---|
| `history_archived_e2e::history_by_the_workers_name_…` (line 168) | `main` ×2 (12:41, 12:59), `rust` ×1, five branches | The archived worker's transcript holds `(replay: no more scripted replies)` instead of `the codeword is marimba` | **The merge of #436 took commit `33435a8d` (11:04), not the branch tip `a2295f2f` (11:51).** The tip pins the worker's scripted line to its id; the merged text left it unpinned, so root's step after the `spawn` took the line first and the worker ran out of script. The tip's CI was green before the merge and is green now. The revert on `main` (`62684e17`) removed the whole feature for a race the branch had already fixed. | Re-merge #436 at its tip. No code change needed. |
| `multitask_e2e::…their_dones_batch` (line 121) | desktop-only branch | `root turns: 4`; every report assertion passed | Done-batching is opportunistic (`plan.rs::batch_done_files` joins whatever is already in the inbox). Three children finishing far enough apart give one turn each. The bound `(2..=3)` treated that as enforced. | #458: assert each child is in exactly one `done` wake, turns = spawn turn + done wakes. |
| `helper_kinds_e2e::an_area_coordinator_…` (line 108) | `cursor/coordinator-sleep-b027` | The area coordinator completed one turn, not two; the worker's report is on its transcript inside that turn | The worker finished before the area's first turn ended, so its done folded into the running turn (designed since the done-wake fold). The test waited for a second turn that the timing never opens. | #463: wait for the report on disk and the turn holding it to end; assert once. |
| `binary_gone_e2e::a_kernel_whose_directory_was_renamed_…` (line 303) | `cursor/side-panel-tabs-bde4` | "the kernel never restarted after its directory was renamed" within 30 s | On `main` a re-exec attempt that finds the new file still being copied is `Failed` and retries in **60 s**; the test allows 30 s. On a loaded runner the copy is slower and the first attempt lands inside it. | #453 (open): `NotReady` retries in 2 s; `Failed` keeps 60 s. The test's 30 s is then right. |
| `arbos-update::kernel::tests::the_place_probe_refuses_a_build_that_reads_the_store_worse` (line 723) | `cursor/side-panel-tabs-bde4` | "the new kernel would not run" — the probe could not start the stub | The stub is a `#!/bin/sh` script written and then executed at once from a test process that is also forking other children. A `fork` in another test thread between the write and the close copies the open write descriptor into a child that holds it until its `exec`; the `execve` of the stub then fails with `ETXTBSY`. The same race `binary_gone_e2e` met and answered with `spawn_retrying`. | Update worker's crate. Retry the stub's spawn on `ETXTBSY` in the test (a few tries, short sleep), or write the stub before any thread in the binary spawns. Not a product fault: the real probe runs a binary that was closed long before. |

## The two shapes behind four of them

**Shape A — a count that the timing decides.** `multitask_e2e` and `helper_kinds_e2e` both counted turns. A turn count is a fact about *when* things landed relative to each other, and the runner owns that. The property each test wanted — every report reaches its parent, once — is on the transcript and does not move with load. Rule: assert the record (a `say` from X exactly once; each child in exactly one `done` wake), never the number of turns it took to get there.

**Shape B — a replay line that anyone may take.** In `replay.rs`, a scripted reply without `agent` goes to the first agent that asks. In a script where root spawns a worker and root has no more pinned lines, root's next model step (after the `spawn` tool result, or on the done wake) takes the worker's line. Whether that happens depends on whether the worker's first call beats root's next step — the runner's business again. #436's tip fixed one instance; the census below lists the rest.

## Census: multi-agent scripts with unpinned lines

Tests that call `spawn` and have at least one `{"content": …}` line with no `agent` (count of such lines first). Each is a candidate for shape B; none has gone red today except the first, and some are safe because root's script ends before the worker's line is reachable. They are listed so the next red in one of them is read in under a minute rather than an hour.

```
11 output_owed_e2e.rs
 5 show_nudge_e2e.rs
 3 audit_kernel_2_e2e.rs
 3 archive_children_e2e.rs
 2 worktree_store_e2e.rs
 2 job_leash_e2e.rs
 1 worktree_cleanup_e2e.rs
 1 worker_chat_e2e.rs
 1 standing_pass_e2e.rs
 1 say_archived_e2e.rs
 1 remote_track_e2e.rs
 1 page_algorithm_e2e.rs
 1 history_archived_e2e.rs   (fixed at the #436 tip)
 1 child_model_e2e.rs
```

The cheap, mechanical fix for all of them is to pin every line in a multi-agent script. It is not done here because each pin needs the agent's id as the kernel mints it, and a wrong pin makes a test fail deterministically rather than rarely — better done file by file, when each is next touched, than in one sweep.

## Two lessons for how the suite is written

1. **The attach stream is a queue, not a log.** `Attach::wait` reads forward and discards what it passes. A test that waits for root's `idle` and then for a child's `idle` will hang if the child's came first. When two agents' frames can interleave, wait on the transcript on disk (the file is the record; the stream is a view of it). #463 does this.

2. **A test must prove its fault was staged.** QA's warning from `qal-j22`: a fix often removes the very mechanism the injection used (an atomic rename replaces an unwritable file without writing to it). A green result then says nothing. Both tests in #456 assert the injection held (the mark write failed; the stale mark with the old `ts` is still on disk; the `Checkpoint not written` notice exists) before asserting the refusal. Worth applying to every fault-injection test in the kernel suite; none of the timing tests above are of that kind, but the read-only-folder tests in `confirmed_reads_e2e` and `rewind_no_identity_e2e` are, and the next pass should check each stages what it claims.

## What is not in this list

The `dev channel` workflow's failures today, and the desktop and iOS jobs, were not read; the question was the kernel job.
