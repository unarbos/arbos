# Agent eval run rerun6d-b7-1657 — 2026-10-25

*Image arbos-agent-0.1.120, agentd `arbos-agentd 0.6.223`, gvisor sandbox of test user `test_01M5JHWC586GYNTC3KCZZKS7NF`; 1 of 1 tasks ran (0 skipped), 2026-10-25T01:09:10Z → 2026-10-25T01:12:05Z. Judge: anthropic/claude-haiku-5.5.*

## Pass rate vs last run

**0/1 passed (0.0 %)** vs 1/1 (100.0 %) in `rerun6c-1664-pause` — -100.0 points.

- newly passing: none
- newly failing: none

Metrics: first reply p50 2.58 s (all turns 0.39 s), total per task p50 173.5 s, tool errors 4, stalls 0, cost $0.02 relay-metered + $0.00 judge.

| category | passed / tasks |
|---|---|
| background | 0 / 1 |

## Top weaknesses

- **t2: the working row's activity did not c** (1): b7-steer-mid-task

## Failures and issues

- `b7-steer-mid-task` — t2: the working row's activity did not change within 5 s of the steer (changed at 7.28, text 'Setting the schedule')

## What was fixed since the last run

— nothing landed between the runs

## What's next

- fix the root cause behind **t2: the working row's activity did not c** (b7-steer-mid-task); the tasks stay as regression cases
- add tasks where this run was weakest; keep the per-run cost under the line

Cost by first-turn route (a `new` route spawns a Sonnet helper and costs ≈ 5× an `act` turn on Haiku, so a task's cost is bimodal): `act` × 1 → median $0.020

## Per task

| task | result | route (t1) | first reply | total | tool err | stalls | cost | rubric | reasons |
|---|---|---|---|---|---|---|---|---|---|
| `b7-steer-mid-task` | FAIL | act | 2.58 s | 173.5 s | 4 | 0 | $0.020 | 3 | t2: the working row's activity did not change within 5 s of the steer (changed at 7.28, text 'Setting the schedule') |
