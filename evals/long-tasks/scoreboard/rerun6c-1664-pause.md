# Agent eval run rerun6c-1664-pause — 2026-10-25

*Image arbos-agent-0.1.120, agentd `arbos-agentd 0.6.223`, gvisor sandbox of test user `test_01M5JHWC586GYNTC3KCZZKS7NF`; 1 of 1 tasks ran (0 skipped), 2026-10-25T01:02:04Z → 2026-10-25T01:06:48Z. Judge: anthropic/claude-haiku-5.5.*

## Pass rate vs last run

**1/1 passed (100.0 %)** vs 9/33 (27.3 %) in `20261024T2330-parity-before` — +72.7 points.

- newly passing: none
- newly failing: none

Metrics: first reply p50 5.51 s (all turns 2.22 s), total per task p50 282.5 s, tool errors 0, stalls 0, cost $0.01 relay-metered + $0.00 judge.

| category | passed / tasks |
|---|---|
| managing | 1 / 1 |

## Top weaknesses

None — every task passed.

## Failures and issues

None.

## What was fixed since the last run

— nothing landed between the runs

## What's next

- add tasks where this run was weakest; keep the per-run cost under the line

Cost by first-turn route (a `new` route spawns a Sonnet helper and costs ≈ 5× an `act` turn on Haiku, so a task's cost is bimodal): `act` × 1 → median $0.008

## Per task

| task | result | route (t1) | first reply | total | tool err | stalls | cost | rubric | reasons |
|---|---|---|---|---|---|---|---|---|---|
| `m7-long-chat-catchup-after-restart` | pass | act | 5.51 s | 282.5 s | 0 | 0 | $0.008 | — |  |
