---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Coordinator answer shape (Cursor's) — PR #195, branch `cursor/answer-shape-b027` (on `main`)

From the symmetry loop's `2026-09-14-symmetry-answer-shape-vs-cursor.md`: the coordinator's answers ran ~3× Cursor's for the same task.

Change (prompt): `COORDINATOR_CONTRACT` and `PROTOCOL.md` carry an answer contract — lead with what was done in 1–2 sentences, file names inline; the one thing the user asked to see in a single block, nothing else in blocks; bullets only for 3+ items; no headings/bold/lone "Done." in a short answer; never narrate the delegation (worker, workstream, branch, hash) or the bookkeeping (notes.md); a dispatch reply is one sentence; "done" only for what is in the user's checkout; a side failure is one sentence at the end; no closing offer unless blocked. Also: a lone worker edits the checkout in place; `isolate=worktree` only for two or more workers editing at once.

Change (kernel): a `[done]` message for a worker that ran in a worktree ends with the worktree's state (`UNCOMMITTED: N path(s) …`, `N commit(s) on branch …`, or "no changes").

Scenarios (coordinator place, gpt-5.4-mini, pair each with Cursor as the loop does):
- `Rename the function sub to subtract everywhere in this project, keeping behaviour, and run main.py to check.` (project: `math_utils.py` with add/sub, `main.py` printing both, `tests/test_math_utils.py`) → final answer ≤ ~40 words, one output block (`5` / `4`), no headings, no "worker"/"branch"/hash, no offer; `git status` shows the three files modified in the checkout. Measured 2026-09-14: 34 words (was ~100).
- The same with two workers editing different files → each may get a worktree; the answer says where the change is if not merged.
- A worker in a worktree that never commits → done message carries `UNCOMMITTED`; root's answer says the checkout is unchanged (seen live: "The rename is in the worktree … still uncommitted, so your checkout is unchanged").
- Dispatch turn (worker not `wait=true`) → the reply is one sentence, no code block.
- Drift to watch: root still "researches first" (reads, repro attempts, tries write/edit itself and is refused) before spawning — 9 calls before the spawn in the last run. See the follow-up in the PR summary.

E2e: none new (model behaviour). Unit/e2e suites green.
