# qal-j17: when the checkpoint's index copy is delayed, the "before the turn" tree already holds what the turn wrote — a rewind to that turn keeps the turn's own file

- Measured at: #419 @ `5340c0d2` (`arbos-kernel 0.2.0 5340c0d27b29`), 4 of 5 runs of `rw-10b-index-is-not-a-regular-file-for-an-instant-when-the-checkpoint-copies-it`; controls `0bceb0df` and `main` `7e19f9e9` (5 runs each) lose the tree instead (below); with no delay (`ARBOS_QA_RW10B_NO_SWAP=1`, 5 runs each on `5340c0d2` and `main`) the tree is right every time. Rollouts `internal/qa/rollouts/20260917T10*-rw-10b-…`; every checkpoint's tree listed in `result.json` → `notes.checkpoint_after_turn_wrote`.
- Class: wrong record that a destructive step later trusts (the qal-j08 family). Not a loss by itself: a `rewind --files` to that turn leaves the turn's first file in place and says "restored".
- Feature: turn checkpoints (`turn.rs`: record before the turn, tree on the blocking pool while it runs; `tools/git.rs` `snapshot_turn_tree`, the index copy that #419 at `5340c0d2` retries 10 × 25 ms).

## What the probe does

Three turns; turn N writes `fN.txt` through the bash tool (replay provider: the tool call starts within milliseconds of the turn). As each turn's checkpoint record appears in `checkpoints.jsonl`, the harness swaps `.git/index` for a directory for 120 ms and puts it back — to a copy, the same "not a regular file" that another git's `index.lock → index` rename shows for an instant, held long enough to be certain. Then every checkpoint's work commit is listed: checkpoint N is the tree *before* turn N, so it must not contain `fN.txt`.

| build | tree lost (`copy the index: … neither a regular file …`) | tree present but holds `fN.txt` |
|---|---|---|
| `main` `7e19f9e9` | 3 of 5 runs (1–3 of 3 checkpoints) | 0 |
| #419 `0bceb0df` (no retry) | 5 of 5 runs | 0 |
| #419 `5340c0d2` (retry) | 0 | **4 of 5 runs — checkpoint 2 holds `f2.txt`** |
| any build, no swap | 0 | 0 |

So the retry turns a *missing* checkpoint (which `rewind --files` refuses, honestly) into a *wrong* one (which it restores, confidently). The 120 ms the copy waits is long enough for turn 2's `echo second > f2.txt` to land, and the "before" tree is taken after it.

## Why it matters beyond the probe

The window is not the retry's; the retry only widens it. The tree is taken beside the turn by design (#405: record first, tree on the blocking pool). On a large repository `git add -A` into the scratch index takes seconds — the design says so — and a model's first tool call can come sooner than that. Whatever the turn writes before the snapshot finishes is recorded as if it had been there before the turn. In a person's words: "I rewound to before that turn and the file it created is still here, and it says restored."

## What we expect

Either the turn's first mutating tool call waits for the tree snapshot to finish (the record is cheap and lands first; the tree is the slow part, and the tools are the only thing that can move the tree under it), or the snapshot is taken from a state the turn cannot have touched (the index copy plus a `git stash create`-style tree of the moment the record was written). The retry itself is right; what it waits for must not be able to change meanwhile.

## Regression check

`rw-10b` as above: passes when every checkpoint has a work tree *and* checkpoint N does not hold `fN.txt`, with the swap in place. `rw-10` (three terminals committing at ~100 commits/s each, then a rewind) is the realistic companion and passes on every build; it rarely lands on the instant, which is why `rw-10b` exists.

## Two harness notes, for the review list

- The first `rw-10b` used a FIFO for the momentary non-regular index. `open()` on a FIFO with no writer blocks forever; the kernel's turn hung on every build for 200 s. A failure the harness invented, caught by "what else could make it fail". A directory gives the same `stat` answer without blocking.
- `main` also copies the index and also loses the tree to the instant; the "once-seen" error in `qal-j16` was not introduced by #419.
