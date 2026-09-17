---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# `child_model` — PR #167, branch `cursor/spawn-model-force-b027` (on `main`)

T3-05. `child_model = "<id>"` in config.toml pins every agent with a parent to that model, over the spawn call and the kind; root untouched; the picker's one-turn switch still wins. Decided at turn time; agent.md keeps what was asked. `kernel.log` `prompt_size` lines now start with `model=<id>`.

Scenarios:
- Set `child_model = "openai/gpt-5.4-mini"`, root on a bigger model, spawn a worker with `model:"anthropic/…"` → the worker's `prompt_size` line says the mini; root's does not; `w1/agent.md` still names anthropic.
- Remove the setting, restart → the worker's next turn uses what agent.md says.
- Kickoff benchmark cost check: the same 3-worker ask with and without `child_model` → `usage.cost` on workers' `turn_complete` should drop.

E2e: `crates/arbos-kernel/tests/child_model_e2e.rs`.
