# qa-020: a job that hits `timeout_ms` runs `kill -9 -<pid>` without `--`, which procps turns into `kill(-1, SIGKILL)`: every process of the user dies

status: pr-open — https://github.com/unarbos/arbos/pull/73 (`cursor/fix-qa-020-killpg-de28` -> `rust`) and https://github.com/unarbos/arbos/pull/74 (`cursor/fix-qa-020-killpg-52cd` -> `cursor/release-integration-52cd`)
severity: CRITICAL (destroys the user's whole session; took down a production validator twice)
scenario: bench-screenshot (any scenario with a background job and a timeout)
rollout: evidence kept on ArbosLife by the Affine agent: `/tmp/arbos-qa-bench-screenshot-pmh2e3vu`, `/tmp/arbos-qa-bench-screenshot-vlwqwtea` (`jobs/j1/meta.json`), write-up `~/arbos-qa/deploy/DISABLED-BY-AFFINE-AGENT-2026-09-13.md`
fingerprints: none (the kernel that finds it dies with everything else)

## What happened

2026-09-13 00:24:33 and 03:09:29 UTC on ArbosLife: every process owned by `const` was SIGKILLed at once (pm2 with affine-validator, dash, evalwatch, tunnels, king-datagen; `user@1001.service`; every SSH session; `forest-head.service`, `arbos-web.service`). The validator was down 00:24 -> 04:29 UTC. Both times, 28 ms / 1000 ms earlier, the QA loop's kernel had started `python3 -m http.server 8765` as a background job with `timeout_ms` and then killed it on timeout. The Affine agent's `strace` showed `kill(-1, 9)`.

My 00:2x "user manager restarted" finding in the design doc was this bug, not session lifecycle.

## Root cause

`crates/arbos-engine/src/tools/bash.rs:448` `kill_job`: `Command::new("kill").args(["-9", &format!("-{pid}")])`. procps-ng 4.0.4 `/usr/bin/kill` parses `-1678616` as an option, not a process group, and signals -1.

## Fix

`libc::killpg(pid, SIGKILL)` on the job's own process group (jobs are spawned with `process_group(0)`), `libc::kill` as fallback, pids 0/1 refused. No shell. Tests: a job group dies while a bystander in another group survives; 0 and 1 are no-ops.

## Loop status

Disabled on ArbosLife by the Affine agent (units unlinked, cycle stopped). Do not re-enable until #73/#74 are on the branches under test, and then run the loop as its own Unix user so an agent bug can never reach pm2 again. Files under `~/arbos-qa/` are untouched.
