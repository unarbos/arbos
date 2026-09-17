# qal-j08: in a repository with no git identity, rewind with `files: true` deletes the kept turns' uncommitted files — and reports success

- Measured at: `main` @ `7f6a6b9a` (`arbos-kernel 0.2.0 7f6a6b9a06bc`) and #377 @ `1a0003a2` (`arbos-kernel 0.2.0 1a0003a29c4e`), both the same; replay provider, no model.
- Feature: turn checkpoints (`arbos_engine::tools::git::snapshot_turn` / `work_commit`) and the file restore on rewind (`restore`).
- Severity: high for exactly the user J1 describes — a new machine, a fresh place, no `git config user.name/email` yet. Rewind is what Jacob uses to come back after something goes wrong; here coming back deletes work from the turns he kept, and the window says the restore succeeded.
- Scenario: `rw-04-rewind-with-files-in-a-repo-without-git-identity` (fails 2/2, both kernels); the control `rw-01` with identity set passes 2/2 (`f1.txt`, `f2.txt` restored, checkpoints carry a work tree). Rollouts `internal/qa/rollouts/20260917T043540Z-rw-04-…` and `20260917T043657Z-rw-04-…`; controls `20260917T043443Z-rw-01-…`, `20260917T043559Z-rw-01-…`.

## Repro

A place that is a git repository with one commit and **no** `user.name`/`user.email` (no global config either — a fresh account). Three turns; in turns 1 and 2 the agent runs `echo first > f1.txt`, `echo second > f2.txt` (uncommitted). Rewind to turn 3 with `files: true`.

- `checkpoints.jsonl` has three lines, each `{"line", "ts", "head"}` — **no `work` field on any of them**.
- The `rewound` frame reports `restored: <head sha>`; no error.
- Afterwards the place has **no `f1.txt`, no `f2.txt`** — the files written in the turns that were kept are gone. Turn 4 writes `f4.txt`; rewind to turn 4 → all gone again.

With `git config user.name/email` set (control): every checkpoint after the first carries `work`, `restored` names `head + working tree`, and `f1.txt`, `f2.txt` are back after the rewind.

## Expected

The checkpoint of a turn holds the working tree as it was ("tracked changes and untracked files alike"); rewinding to turn 3 brings back the files turns 1–2 made. When the work-tree commit cannot be made, the rewind must not proceed to `git clean` as if it had one — it should refuse `files: true` with a reason ("no checkpoint of the working tree; set a git identity" or better, make the internal commit not need one), or restore only what it knows it can.

## Actual

`work_commit` runs `git commit-tree`, which needs an author identity; with none configured it fails, `work_commit` returns `None` silently, and the checkpoint records HEAD alone. `restore` then does `git reset --hard <head>` and `git clean -fd -e .arbos` — removing every untracked file in the project, not only those the rewound turns added — and returns the head sha as success.

## Suspected location

- `crates/arbos-engine/src/tools/git.rs::work_commit`: pass `-c user.name=arbos -c user.email=arbos@kernel` (or `GIT_AUTHOR_*`/`GIT_COMMITTER_*` env) to `commit-tree`; an internal ref needs no real identity. And when it still fails, say so in the checkpoint (`work_error`) rather than `None`.
- `crates/arbos-engine/src/tools/git.rs::restore`: with `cp.work == None`, do not `git clean`; report "no working-tree checkpoint for this turn; tracked files reset, untracked left as they are".

## Fix

Not started (features agent). Regression check: `rw-04` — after the rewind `f1.txt` and `f2.txt` exist, or the `rewound`/`error` frame says the working tree could not be restored and nothing untracked was removed.

## Beside it: what this lead was about

The standing_pass_e2e lead — "an empty transcript after a rewind, the rewind eating the record" — did **not** reproduce here. 32 rewinds (16 runs × 2 rounds) across bare, kernel-pinned-to-one-core-with-four-spinners, and disk-churn variants, on both kernels, with the transcript file read every 50 ms through each rewind (160 reads per rewind): the lowest line count ever read was the settled count (12, then 18), no read came back empty or missing, every kept turn was on the file afterwards, and a fresh attach was handed all four kept lines. If the e2e's empty read was real, it is not the transcript on disk being emptied on Linux under these loads; the author's cause is still open.
