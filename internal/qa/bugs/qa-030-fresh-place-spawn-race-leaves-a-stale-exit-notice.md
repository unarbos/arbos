# qa-030: opening a fresh place spawns two kernels; the loser's exit stays in the chat as a failed notice

- Feature: desktop kernel launch (`desktop/src/kernel.rs`), `rust` and the integration branch alike
- Severity: medium. The chat works (the second attach wins), but the first thing a user sees in a new place is "arbos-kernel exited (exit status 1)".
- Reported by: Jacob, 2026-09-13 20:19 UTC
- Status: fix PR [#120](https://github.com/unarbos/arbos/pull/120) into `rust`; scenario `desktop-fresh-place-no-notice` in `internal/qa/desktop_scenarios.py` (runs in the cycle against the #105 desktop stack)

## Repro

1. Open a folder with no `.arbos/` in the desktop.
2. Watch the first chat: a failed notice "arbos-kernel exited (exit status 1) … place already served (…/.arbos/runtime/lock)" appears, then the chat goes live anyway.

Scenario: `ARBOS_DESKTOP_BIN=… ARBOS_DESKTOP_DRIVER=… python3 run.py --kernel <bin> --only desktop-fresh-place-no-notice` (Xvfb). It checks the chat items via the driver, every `transcript.jsonl`, and `.arbos/kernel.log` for `already served`.

## Cause

`kernel::attach_or_spawn` is called from three places when a place opens: the chat session (`agent/acp.rs`), the board hub (`boardhub.rs`) and the terminal (`view/terminal.rs`), each on its own thread. Each one reads `.arbos/kernel.json`, finds nothing live, and spawns `arbos-kernel serve`. The kernels race for `arbos_core::PlaceLock` (`.arbos/runtime/lock`); the losers exit 1 with "place already served". `wait_ready` sees the child exit and returns `Err("arbos-kernel exited (exit status 1)…")`; the session turns that into a failed `Notice`, then retries and attaches to the winner. The remote path already had this fixed (`attach_remote_cached` holds a per-key mutex); the local path did not.

## Fix

`desktop/src/kernel.rs`:
- one `spawn_lock(workspace)` mutex per place: the first attacher spawns, the others wait on it and then find the live `kernel.json`;
- `wait_ready`: a child that exits with "place already served" in its log is not a failure — another process holds the place; `wait_other` polls for that kernel's `kernel.json` until the same deadline. No notice is written for a lost lock race.

Kernel unchanged: a second `serve` still exits non-zero with a clear message (the `second-serve` scenario relies on it).

Regression checks: `desktop/src/kernel/tests.rs` `concurrent_local_attaches_spawn_one_kernel` (six threads, one kernel, no "place already served" in the log; skips when no kernel binary is built) and `a_lost_lock_race_is_not_a_kernel_failure`.
