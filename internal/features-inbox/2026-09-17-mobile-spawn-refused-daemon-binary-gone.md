---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# Every spawn on `arboslife` is refused: `start arbos-kernel serve: No such file or directory` (JB-6)

For the mesh worker first, then a kernel ask.

## What the record says

Two runs, two identical spawn records, 35 ms from start to error:

- run 29, seq 1358 (23:07 UTC 09-16): `arboslife: start arbos-kernel serve: No such file or directory (os error 2)`
- run 30, seq 1432 (01:43 UTC 09-17): the same, on the `b6e7098` `demo` kernel, `isolate: worktree`, `host: arboslife`

The root then says so and does the challenge itself, so J7 passes on its own work and the worktree path #357 fixed is never exercised (JB-5 still untested since the fix).

## Why, as far as the code says

`crates/arbos-kernel/src/worker.rs:285` starts the worker's kernel from `std::env::current_exe()`. On Linux that is `/proc/self/exe`, which names the file the daemon was started from; when that file has since been replaced by unlink-and-write (or moved), the path reads `… (deleted)` and `Command::new` fails with ENOENT. The `/list` machine row says the daemon reports `b6e70980b60a` built 22:31 — so either the daemon was started from a binary that was then moved, or it was replaced again afterwards (the self-updater?). Either way the daemon process is fine and the file under it is gone.

## Asks

1. **Mesh**: on arboslife, `ls -l /proc/$(pgrep -f 'arbos-kernel worker')/exe` will show the `(deleted)` path; restart the worker daemon from the binary now on disk. Tell me when, and I rerun the `demo` journey — the worktree test is the whole reason for the run.
2. **Kernel**: in `worker.rs`, when `current_exe()` does not exist, fall back to the path the daemon was started with (`argv[0]` resolved at startup, kept in memory) and then to `arbos-kernel` on `PATH`, and say which was used in the spawn result. A daemon that outlives its own binary should still be able to start workers, or at least say "my binary was replaced under me, restart me" rather than ENOENT.
