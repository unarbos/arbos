---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# K-16 follow-up: `serve --until-idle`, `--horizon`, `--now`

Branch `cursor/until-idle-now-b027` (on `cursor/release-integration-52cd`). The fixture runner's flags from `docs/filesystem-state-design.md` "Testing by authored states".

- `--until-idle` (`ARBOS_UNTIL_IDLE=1`): exit 0 when no turn runs, no plan node/condition is in flight, no pending node is fireable now or due within `--horizon` (`ARBOS_HORIZON`, default `1h`), and no `ask` waits. Exit 3 when only a question waits. One check a second after a 1.5 s grace; two quiet checks in a row.
- `--now 2026-09-13T09:00:00Z` (`ARBOS_NOW`, also unix millis/seconds): `arbos_core::now_ms()` starts at that instant and runs forward. Every timestamp (transcript `ts`, `kernel.log`, plan `created_ms`, attempts) shifts together.

Verified with a hand-written `plan.jsonl` (shell node `echo 42`, report `BTC: {output}`, `after_ms` = 09:00Z): `--now 06:00Z` → exits idle in 3 s, node untouched (2h20m away, beyond the horizon); `--now 08:20:01Z` → stays up (due in 40 min, inside the horizon — correct, so use `--horizon` deliberately); `--now 09:00:01Z` → node fires, `say root → user "BTC: 42"` on the transcript at `ts` 09:00:01Z, node `done`, exit 0 in 3 s.

Attack surface: a fixture whose first node is an agent turn with `--provider replay` (once #69 and this are on one branch); `--until-idle` with a detached job still running (jobs do not count as busy — say if they should); a recurring node (`every`) inside the horizon keeps the kernel up forever — expected, document or cap; `--now` with a local-time string (rejected: UTC only; the message names the format); a kernel started by `arbos-kernel run` while `ARBOS_UNTIL_IDLE=1` is exported (it will exit after the turn; `run` then sees the connection close — check the exit path); wall-clock-dependent code that does not use `now_ms` (provider trace timestamps, `Instant`-based timeouts) stays on real time by design.
