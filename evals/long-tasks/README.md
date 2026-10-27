# Long-task agent eval (D94)

Realistic multi-step tasks run against a dedicated **test** Arbos agent through the real product
path (relay → agentd → image on the test user's sandbox), graded programmatically plus an LLM
rubric, with a versioned scoreboard. Design, task list, cost and run reports live in the project
store (`docs/agent-eval.md`, `docs/agent-eval-runs/`).

- `tasks/<category>/<id>.json` — one task each: `turns[]` with `prompt`, optional `seed` (bash on the
  box), `generate` (fixture generator in `lib/fixtures.py`), `verify` (bash printing PASS/FAIL and
  `SHOW` lines), `truth` (driver-side comparison), `reply_regex`, `expect_question`, `routine`,
  `latency_max_s`, `rubric`, `new_chat` / `chat: "first"`; task-level `lane`, `same_chat_as`, `verify_wait`, `doc_checks`, `regression`.
- `run.py` — runner (lanes = projects on the test sandbox, run in parallel); `report.py` — scoreboard
  summary + per-run report; `lib/issues.py` — one root-cause issue per failure.
- `scoreboard/<run>.json`, `scoreboard/SUMMARY.md` — versioned results. `results/` is local evidence
  (chat docs, pi sessions) and is not committed.
- `setup_actions`: `people_group` (`peer_handle`, `with: [lanes]`, `title`) opens a people group — the eval
  user, a scratch peer, this lane's agent and the named lanes' agents — and `chat: "dm"` turns run in it; per
  turn `reply_from: "lane-a"` / `silent_agents: ["lane-b"]` check who answered by sender handle, and
  `expect_no_reply_s` checks that nobody did. `--subset respond-gate` is the group respond-gate set (g6–g9 + h2).
- One command: `scripts/evals/long-tasks/run-suite.sh [--tasks a,b] [--subset regression|quick|respond-gate] [--lanes a,b] [--no-issues]` — run from a
  checkout of `main`; it also commits the scoreboard on a fresh `feedback/long-task-eval-run-<run>` branch
  and registers it with the loop (`LT_COMMIT=0` to skip). Partial re-runs fold in with `merge.py`;
  `rejudge.py` re-runs the rubric for turns the judge skipped.

Rules: scratch test user only, approval cards are rejected never approved, no real outside
messages, runner stops at `--budget-usd` (default 8), the test user's relay daily cap is $10.
