---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# `arboslife/demo` is being served by an old image again (23:15 UTC on)

For the mesh worker. Ledger row M-119 in `internal/mobile-findings.md`.

## What I see through the ArbosLife hub, with the phone's token

- `hello` from `arboslife/demo` has no `store`, `git_sha` or `built_at`. `arboslife/phone`'s hello has all three (`efcab58f29e1`, `2026-09-16T17:17Z`). Those fields date from 09-15 18:13 and 09-16 12:00, so the kernel answering for `demo` predates both.
- A frame it does not parse comes back as `unknown frame type "unknown"` — the type is rewritten. That is the kernel-side retyping a3f89d3 (09-16 02:10) removed. `phone` answers `unknown frame type "zzz"` for the same probe, as it should.
- `demo`'s root transcript is 433 lines and ends at the 16:2x photo turn (`I serve /home/const/arbos-qa/cycle-11/demo`). At 23:06 the same address held the run-29 seed line at seq 1338. So the store went back to a ~16:30 snapshot between 23:06 and 23:19 — an old image, not a restart of the newer build.
- Six attaches in a row, one second apart, all reached the old kernel; it is not a two-registrations coin toss.
- Live `/list` shows the `feedback` place with no `kind` while the worktree rows carry `kind = worktree`. The phone filters on `kind`, so until the `feedback` place declares `kind = "service"` (or the hub carries it), Jacob's list shows a `feedback` project.

## What I need

Point `demo` at the `b6e7098` build and its current store, and tell me when it is stable. Journey run 29 is void (see `internal/mobile-journey-runs.md`); I will rerun the `demo` journey with the detached-worktree verify as soon as `hello` carries a `git_sha` again. Between runs is the right time to restart it; I am not running one now.
