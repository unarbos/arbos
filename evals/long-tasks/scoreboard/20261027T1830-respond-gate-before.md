# Agent eval run 20261027T1830-respond-gate-before — 2026-10-27

*Image arbos-agent-0.1.165, agentd `arbos-agentd 0.6.295`, gvisor sandbox of test user `test_01M5JHWC586GYNTC3KCZZKS7NF`; 5 of 5 tasks ran (0 skipped), 2026-10-27T18:24:27Z → 2026-10-27T18:32:56Z. Judge: anthropic/claude-haiku-5.5.*

## Pass rate vs last run

**3/5 passed (60.0 %)** vs 71/81 (87.7 %) in `20261027T1520-run9` — -27.7 points.

- newly passing: none
- newly failing: none

Metrics: first reply p50 1.96 s (all turns 2.02 s), total per task p50 64.9 s, tool errors 0, stalls 0, cost $0.03 relay-metered + $0.00 judge.

| category | passed / tasks |
|---|---|
| group | 2 / 5 |

## Top weaknesses

- **t1: lane-a answered a message not addres** (1): g6-respond-gate-about-agents
- **t2: lane-b answered a message not addres** (1): g6-respond-gate-about-agents
- **weak answer quality (rubric)** (1): g9-respond-gate-one-of-you

## Failures and issues

- `g6-respond-gate-about-agents` — t1: lane-a answered a message not addressed to it: 'Long-task eval, thanks for the candid note. What has been hardest to use or most'; t2: lane-b answered a message not addressed to it: "I can't add people or agents to a
- `g9-respond-gate-one-of-you` — t1: rubric 0/5: Criteria require exactly one agent to answer; the transcript shows 'Lisbon.' twice, indicating two agents both answered, which is a fail condition.

## What was fixed since the last run

— nothing landed between the runs

## What's next

- fix the root cause behind **t1: lane-a answered a message not addres** (g6-respond-gate-about-agents); the tasks stay as regression cases
- fix the root cause behind **t2: lane-b answered a message not addres** (g6-respond-gate-about-agents); the tasks stay as regression cases
- fix the root cause behind **weak answer quality (rubric)** (g9-respond-gate-one-of-you); the tasks stay as regression cases
- add tasks where this run was weakest; keep the per-run cost under the line

Cost by first-turn route (a `new` route spawns a Sonnet helper and costs ≈ 5× an `act` turn on Haiku, so a task's cost is bimodal): `?` × 1 → median $0.008, `reply` × 4 → median $0.001

## Per task

| task | result | route (t1) | first reply | total | tool err | stalls | cost | rubric | reasons |
|---|---|---|---|---|---|---|---|---|---|
| `g6-respond-gate-about-agents` | FAIL | — | — | 207.0 s | 0 | 0 | $0.008 | — | t1: lane-a answered a message not addressed to it: 'Long-task eval, thanks for the candid note. What has been hardest to use or most'; t2: lane-b answered a mes |
| `g7-respond-gate-mention-signal` | pass | reply | 2.02 s | 121.4 s | 0 | 0 | $0.001 | — |  |
| `g8-respond-gate-everyone` | pass | reply | 1.7 s | 64.9 s | 0 | 0 | $0.001 | 2 |  |
| `g9-respond-gate-one-of-you` | FAIL | reply | 2.04 s | 64.0 s | 0 | 0 | $0.000 | 0 | t1: rubric 0/5: Criteria require exactly one agent to answer; the transcript shows 'Lisbon.' twice, indicating two agents both answered, which is a fail conditi |
| `h2-people-chat-mention-send` | pass | reply | 1.91 s | 49.6 s | 0 | 0 | $0.002 | — |  |
