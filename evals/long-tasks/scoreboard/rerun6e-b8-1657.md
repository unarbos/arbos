# Agent eval run rerun6e-b8-1657 — 2026-10-25

*Image arbos-agent-0.1.120, agentd `arbos-agentd 0.6.223`, gvisor sandbox of test user `test_01M5JHWC586GYNTC3KCZZKS7NF`; 1 of 1 tasks ran (0 skipped), 2026-10-25T01:13:58Z → 2026-10-25T01:17:38Z. Judge: anthropic/claude-haiku-5.5.*

## Pass rate vs last run

**0/1 passed (0.0 %)** vs 0/1 (0.0 %) in `rerun6d-b7-1657` — +0 points.

- newly passing: none
- newly failing: none

Metrics: first reply p50 2.67 s (all turns 1.58 s), total per task p50 219.7 s, tool errors 0, stalls 1, cost $0.01 relay-metered + $0.00 judge.

| category | passed / tasks |
|---|---|
| background | 0 / 1 |

## Top weaknesses

- **turn did not finish** (1): b8-steer-in-turn
- **t2: the working row's activity did not c** (1): b8-steer-in-turn
- **t3: FAIL: last=40 lines=40** (1): b8-steer-in-turn
- **t3: reply matches forbidden /(last|final** (1): b8-steer-in-turn

## Failures and issues

- `b8-steer-in-turn` — t2: turn did not finish before the task timeout; t2: the working row's activity did not change within 5 s of the steer (changed at 7.58, text 'Steering: Change of plan: stop at 20, not 40. Plea…'); t3: FAIL: last=40 line

## What was fixed since the last run

— nothing landed between the runs

## What's next

- fix the root cause behind **turn did not finish** (b8-steer-in-turn); the tasks stay as regression cases
- fix the root cause behind **t2: the working row's activity did not c** (b8-steer-in-turn); the tasks stay as regression cases
- fix the root cause behind **t3: FAIL: last=40 lines=40** (b8-steer-in-turn); the tasks stay as regression cases
- add tasks where this run was weakest; keep the per-run cost under the line

Cost by first-turn route (a `new` route spawns a Sonnet helper and costs ≈ 5× an `act` turn on Haiku, so a task's cost is bimodal): `reply` × 1 → median $0.011

## Per task

| task | result | route (t1) | first reply | total | tool err | stalls | cost | rubric | reasons |
|---|---|---|---|---|---|---|---|---|---|
| `b8-steer-in-turn` | FAIL | reply | 2.67 s | 219.7 s | 0 | 1 | $0.011 | 3 | t2: turn did not finish before the task timeout; t2: the working row's activity did not change within 5 s of the steer (changed at 7.58, text 'Steering: Change  |
