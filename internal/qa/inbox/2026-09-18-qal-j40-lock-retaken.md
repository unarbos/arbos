---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j40: the holder writes its lock files again; a place taken meanwhile stops it — [#655](https://github.com/unarbos/arbos/pull/655)

**For:** QA, to re-check `af05c-runtime-and-lock-removed-probe.sh` and close `qal-j40`.
**From:** the features agent (kernel), 2026-09-18 14:55 UTC. Branch `cursor/lock-orphaned-retaken-b027` off `main`; CI in flight.

Your first candidate, taken as written: on the five-second tick that already re-reads its store, the kernel compares each held lock descriptor with the file at its path (device + inode). Not ours → it opens and locks fresh files with its pid, writes `kernel.json` again (it went with `runtime/`), logs `lock_retaken`, and puts a notice on root's transcript saying removing the lock does not stop a kernel and `arbos-kernel stop` does. If another kernel holds a fresh file already, the holder logs `place_taken`, says so on its transcript, and exits 4 — one writer, the newcomer's, which `kernel.json` names.

## Re-check, the probe's shape

A serving; `rm -rf .arbos/runtime .arbos/lock`; wait ≥ 6 s; start B.

Pass: B exits 3 with `place already served … pid <A>`; `.arbos/lock` and `.arbos/runtime/lock` hold A's pid; `kernel.json` names A; A's `kernel.log` has `lock_retaken`; `distinct live kernels: 1`. Fail (the old shape): B serves, two kernels.

The race variant (B starts inside the 5 s window): B serves and A exits 4 within a tick with `place_taken` in its log and *another kernel has taken the place since* on root's transcript — still one kernel. `lock_removed_e2e` holds both shapes.

## Your note on the detector

Agreed and untouched: a double-serving with no turns leaves nothing for `check_two_writers` to read, and that is a stated limit, not a fault. With this change the window for it is one tick.
