# Agent eval run rerun6f-b7b8-1657 — 2026-10-25

*Image arbos-agent-0.1.120, agentd `arbos-agentd 0.6.223`, gvisor sandbox of test user `test_01M5JHWC586GYNTC3KCZZKS7NF`; 2 of 2 tasks ran (0 skipped), 2026-10-25T01:18:48Z → 2026-10-25T01:24:46Z. Judge: anthropic/claude-haiku-5.5.*

## Pass rate vs last run

**1/2 passed (50.0 %)** vs 0/1 (0.0 %) in `rerun6e-b8-1657` — +50.0 points.

- newly passing: none
- newly failing: none

Metrics: first reply p50 7.79 s (all turns 2.48 s), total per task p50 178.65 s, tool errors 2, stalls 1, cost $0.03 relay-metered + $0.00 judge.

| category | passed / tasks |
|---|---|
| background | 1 / 2 |

## Top weaknesses

- **t3: FAIL: last=40 lines=40** (1): b8-steer-in-turn
- **t3: reply matches forbidden /(last|final** (1): b8-steer-in-turn

## Failures and issues

- `b8-steer-in-turn` — t3: FAIL: last=40 lines=40; t3: reply matches forbidden /(last|final) number[^.\n]{0,40}\b40\b|\b40 lines/

## What was fixed since the last run

— nothing landed between the runs

## What's next

- fix the root cause behind **t3: FAIL: last=40 lines=40** (b8-steer-in-turn); the tasks stay as regression cases
- fix the root cause behind **t3: reply matches forbidden /(last|final** (b8-steer-in-turn); the tasks stay as regression cases
- add tasks where this run was weakest; keep the per-run cost under the line

Cost by first-turn route (a `new` route spawns a Sonnet helper and costs ≈ 5× an `act` turn on Haiku, so a task's cost is bimodal): `act` × 1 → median $0.016, `reply` × 1 → median $0.011

## Per task

| task | result | route (t1) | first reply | total | tool err | stalls | cost | rubric | reasons |
|---|---|---|---|---|---|---|---|---|---|
| `b7-steer-mid-task` | pass | act | 13.09 s | 181.8 s | 2 | 0 | $0.016 | 3 |  |
| `b8-steer-in-turn` | FAIL | reply | 2.48 s | 175.5 s | 0 | 1 | $0.011 | 3 | t3: FAIL: last=40 lines=40; t3: reply matches forbidden /(last|final) number[^.\n]{0,40}\b40\b|\b40 lines/ |
