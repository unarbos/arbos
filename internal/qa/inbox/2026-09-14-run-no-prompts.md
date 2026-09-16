---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `arbos-kernel run --no-prompts` — PR #164, branch `cursor/run-no-prompts-b027` (on `main`)

T3-04. Unattended runs never park (exit 3): approvals are denied, questions answered with "no one is here; choose a default and say which"; each named on stderr or as a `prompt` JSON line. `rollout replay` uses it. Fix on the way: a fast approval frame arriving before the run saw its own prompt echoed was dropped (run then timed out) — gated on the agent only now.

Scenarios:
- `run --no-prompts "…"` where the model runs `sudo …` → stderr `--no-prompts: denied: …`, tool error "user denied bash", turn continues, exit 0/2 by outcome.
- Same with an `ask` → `unanswered: <question>`, the next turn opens with the unattended answer.
- Without the flag → exit 3 and the hint now mentions `--no-prompts`.
- With #158 merged: a protected-file write under `--no-prompts` is denied — the two together make a safe cron.
- `--json`: the `prompt` line has `question` and `outcome`.

E2e: `crates/arbos-kernel/tests/run_no_prompts_e2e.rs`.
