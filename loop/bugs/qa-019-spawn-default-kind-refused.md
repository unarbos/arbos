# qa-019: `spawn kind="default"` is refused on a fresh place (#58)

status: pr-open — #66 merged into #58; follow-up https://github.com/unarbos/arbos/pull/70 (`inherit`, no-definitions fallback, plan condition message)
severity: high on #58 (no worker can be spawned on a fresh project by the default model; benchmark items 5, 6, 7 fail)
scenario: kickoff-session, spawn-storm (#58 only)
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260913T071654Z-kickoff-session
fingerprints: none (model-visible tool error, scored through the checklist)

## Repro

On #58, prompt root to spawn a worker. The model sends `kind: "default"` (it fills every optional field).

## Expected

The built-in child, as on `rust`.

## Actual

`spawn: no agent definition named "default"; none exist here (add .arbos/agents-defs/<name>.md)` twice; the agent then tells the user it cannot spawn. Kickoff replay on #58: 7, 7, 5 of 12 against 9 on `rust`.

## Suspected location

`crates/arbos-kernel/src/hooks.rs` `spawn_isolated`: any non-empty `kind` must name a definition (commit 8a3a811 "Custom agent definitions").
