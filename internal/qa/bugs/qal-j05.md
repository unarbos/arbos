# qal-j05: a turn ending on the cost cap is told three times in the chat — once as an "Internal error", once as the agent apologising — and the numbers read "$0.00 over the $0.00 cap"

- Feature: the per-turn dollar cap (#347, `max_turn_cost_usd` / `ARBOS_MAX_TURN_COST`) as the desktop draws it; `main` @ `90a33cb2`, kernel and desktop built from it
- Severity: medium. The kernel does the right thing — the turn ends, the reason is on the transcript, the app stays responsive (driver answered in 83 ms) and the next line runs. But what the user reads is: an agent bubble *"I'm sorry, I cannot complete your request. This turn has exceeded the spending l…"*, a failed notice *"turn failed: Internal error — This turn has spent $0.00 on model calls, over the $0.00 cap…"*, and a third notice *"Interrupted: over the turn's cost cap"*. A configured cap is not an internal error and not something the agent should apologise for; three lines for one event; and `$0.00 … over the $0.00 cap` says nothing when the cap is small. This is the class Jacob named: a turn ending for a legitimate reason that reads as the app being broken.
- Scenario: `cp-02-desktop-turn-ends-on-cap` (passes on the assertions it makes — responsive, reason present — and recorded the three lines in `new_items`); rollout `internal/qa/rollouts/20260916T194603Z-cp-02-desktop-turn-ends-on-cap/`. Kernel side: `cp-01-turn-ends-on-spend-cap`, `20260916T194254Z-cp-01-turn-ends-on-spend-cap/`.

## Repro

Desktop app with `ARBOS_MAX_TURN_COST=0.0001` in its environment (so every turn is over the cap after its first model call). Open a fresh place; send `Run \`echo one\` with bash and tell me the output.`

Kernel transcript (correct, if terse):

```
5 user      Run `echo one` with bash and tell me the output.
6 notice    This turn has spent $0.00 on model calls, over the $0.00 cap (max_turn_cost_usd / ARBOS_MAX_TURN_COST); it ends here. What is in the working tree stays; a new message starts a new budget.   (failed: true)
7 interrupted   detail: over the turn's cost cap
8 turn_complete usage.cost 0.00377
```

Chat items the driver reports for that turn:

```
agent   "I'm sorry, I cannot complete your request. This turn has exceeded the spending l…"
notice  "turn failed: Internal error — This turn has spent $0.00 on model calls, over the $0.00 cap …"
notice  "Interrupted: over the turn's cost cap"
```

## Expected

One line, plainly: *"Stopped: this turn spent $0.0038 on model calls, over the $0.0001 per-turn cap (`max_turn_cost_usd`). What is in the working tree stays; a new message starts a new budget."* No "Internal error", no apology in the agent's voice, no second notice for the same stop. Numbers shown with enough precision to be true — or the cap as the user configured it.

## Actual

- Kernel (`turn.rs`): `${spent:.2}` / `${cap:.2}` — $0.0038 and $0.0001 both print as `$0.00`.
- Desktop: a failed notice is prefixed `turn failed: Internal error —` regardless of what the notice says; the `interrupted` line becomes a second notice; and an agent bubble apologising for the spend appears although the model wrote nothing (the apology is synthesised — the #325 path that was meant to keep the nudged-empty-reply apology from the user seems to have a sibling here).

## Suspected location

- `crates/arbos-engine/src/turn.rs` cap branch: format with the precision the numbers need (`{:.4}` under a cent, or `{:.2}` otherwise), and consider the cap notice as the turn's one closing line (no separate `interrupted` detail for the client to draw).
- `desktop/src/model/session.rs`: where a `notice{failed}` becomes `turn failed: Internal error — …` (the prefix should come from the notice's kind, not be assumed), where `interrupted` is drawn as a notice when a failed notice already closed the turn, and where the "I'm sorry, I cannot complete your request…" agent text is produced.

## Fix

Not started. Regression check: `cp-02` should assert exactly one new non-user item after a capped turn, no item containing "Internal error", no agent-kind item; `cp-01` should assert the notice's numbers are not both `$0.00`.
