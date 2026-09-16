# qa-028: plan.jsonl migration turns an open `ask` node into a notes.md line; the question is never put to the user

- Feature: plan engine replacement, PR #104 (`cursor/plan-engine-b027`) + #106 (`cursor/ask-parks-b027` @ `271e9b4`)
- Severity: medium. Jacob's migration rule: standing crons, pending prompts and blocked asks must survive. Two of three do.
- Scenario: `fp-migration-legacy-plan` (rollout `internal/qa/rollouts/20260913T172755Z-fp-migration-legacy-plan/`)

## Repro

`python3 run.py --kernel <ask-parks kernel> --kernel-branch cursor/ask-parks-b027 --fileplan on --only fp-migration-legacy-plan`

1. A place with a legacy `agents/root/plan.jsonl` holding three open nodes: a 30 s shell cron, a pending user prompt (`when.wake`), and `{"do": {"kind": "ask"}, "goal": "Which colour, teal or red?", "status": "pending"}`.
2. Start the #106 kernel.

## Expected

The cron becomes `subscriptions/0001-*.toml` and ticks (it does). The prompt runs (it does). The open ask becomes `waiting/ask-<id>.toml` (question, options, asked) so the desktop shows the question card and the answer arrives as an inbox message per #106 — or, at least, the agent is woken with the question to re-ask.

## Actual

`notes.md` gains `- [ ] Which colour, teal or red?` and nothing else happens. No `waiting/` folder, no `ask` transcript line, no wake. The user never sees the question; it sits in the agent's checklist until some later turn happens to read it.

## Suspected location

`crates/arbos-kernel/src/migrate.rs` `migrate_plan()`: `OldDo::Agent | OldDo::Ask => {}` — an unscheduled ask falls through to the notes path with the other unscheduled nodes. Suggest: for `OldDo::Ask` with status `pending|active|blocked`, write `waiting/ask-migrated-<id>.toml` via `arbos_core::waiting` (question = goal, asked = now) and append the `ask` transcript line, so the answer path of #106 picks it up.
