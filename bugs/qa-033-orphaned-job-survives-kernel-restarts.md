---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qa-033: an orphaned job shell survives kernel restarts and keeps writing into `.arbos/`

- Feature: background jobs (`bash background:true`), kernel start
- Severity: high — a forgotten job wrote `.arbos/user.md` to 10k lines over three days and churned the desktop watch and the per-turn commits
- Found in the Mac wake-up incident (Mac worker's report, 2026-09-13)

## Repro

1. On a kernel from before the jobs leash (#97), or by killing the leash shell first: start `bash background:true` with a script that appends to a file under `.arbos/` every 30 s.
2. Quit the kernel (and the app). The job's parent becomes pid 1; it keeps running.
3. Relaunch. The kernel's watch and commits churn on the file; nothing stops the job.

## Actual (before #130)

`btc-feed.sh` from Sep 10 was still alive on Sep 13, parent pid 1, appending to `.arbos/user.md`.

## Expected / fix (PR #130)

At kernel start every job whose folder says it is running, and whose pid still belongs to it (start time from `/proc` matches `started_ms`; elsewhere the command line names the job folder or program), is killed with its process group; `jobs/jN/killed` reads `left over from an earlier kernel run (reaped at start)`; `kernel.log` has `job_reaped`. A `keep` file in the job folder (or `bash keep:true`) spares it. `arbos-kernel check` warns about such jobs and, on Linux, about any other process holding a file under `.arbos/` open for writing.

## Scenario to add

Start a long `sleep` job, kill the kernel with SIGKILL (so the leash cannot end it), restart: the job is gone within a second and its folder has the `killed` line; the same with a `keep` file: it lives. `crates/arbos-kernel/tests/wakeup_e2e.rs::a_leftover_job_is_reaped_at_start_unless_kept` is the kernel-side version.
