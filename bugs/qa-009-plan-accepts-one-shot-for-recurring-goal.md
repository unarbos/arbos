# qa-009: `plan add` accepts a one-shot `after` for a goal that says "every hour"; zero durations count as a second trigger

status: pr-open — https://github.com/unarbos/arbos/pull/20 (branch `cursor/fix-qa-009-plan-recurrence-wording-de28` -> `rust`)
severity: medium (a standing job the user was told exists fires once and dies; eight tool calls to schedule anything)
scenario: bench-standing-job (benchmark item 9)
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T000829Z-bench-standing-job
fingerprints: 6561543051

## Repro

Prompt: "Every hour, run `date -u` and append the output to notes.md. This must keep running after our conversation ends." Model: `openai/gpt-4.1-mini`.

## Expected

One `plan` call that lands a node with `when.every = 1h`; or a refusal that tells the model exactly what to change.

## Actual

Seven `plan` calls refused: `after: "0s"` beside `every: "1h"` -> "when.after must be a duration like 30m, got 0s" / "choose one of after, every". The eighth, `when: {after: "1m", every: ""}`, goal "…every hour", was accepted. `plan.jsonl` holds a one-shot node; the assistant replied "I set up a recurring task to run every hour". Nothing recurs.

## Suspected location

`crates/arbos-kernel/src/hooks.rs` `NewNode::build`: the recurrence-wording check (`"every "`, `"hourly"`, …) ran only when `trig == 0`; `from_json`'s `opt` treated `"0s"` as a real value.
