# ui-005: Plan strip "Stop" leaves the standing node in place

status: new (suspected; seen on both branches, once each; the stills of those two runs were lost when the pass was re-run, and in three later attempts the model could not get a node past the scheduler, so the strip never came back)
severity: medium (the only control for stopping standing work appears to do nothing)
scenario: internal/parity/ui_pass.py phase P (`plan-stop`)
found: UI QA pass 2026-09-13, both branches
feature: plan strip (`desktop/src/view/detail.rs::plan`, `plan-stop-<chat>` → kernel stop of standing work)
fingerprints: none

## Repro

1. Send `Set up a recurring plan: every hour, run main.py and report if it fails. Confirm once it is scheduled.` Wait for `Scheduled.` and the strip `Plan · 1 standing   Stop`.
2. Click **Stop** (tooltip: "Stop this agent's standing work and its children").

## Expected

The standing node is stopped; the strip drops to `0 standing` or disappears.

## Actual

1.5 s later: strip unchanged, `1 standing`, Stop still shown (integration). On PR 71 the strip still read `Plan · 1 standing   Stop` about 60 s later, after several other checks (`media/qa-ui/pr71-afa582a/056-opener-escape.png`, bottom left).

Not re-verified in the manual session because the model failed to schedule that time ("the plan API rejected the recurring node format") — a kernel/tool issue for the QA agent.

## Suspected location

`plan-stop` click → `chat.stop_work` / kernel `stop` on standing nodes; either the kernel does not remove recurring nodes or the desktop's plan list is not refreshed after the stop.

## Evidence

- Seen in the 10:36 integration run (`plan-stop` row: "strip still shows stop", 1.5 s after the click, tooltip "Stop this agent's standing work and its children" visible) and in the 10:44 PR 71 run (`Plan · 1 standing   Stop` still present ~60 s later in the opener still). Those two result folders were replaced by the 10:59/11:06 re-runs.
- `media/qa-ui/integration-67dcb85/131-v-plan-none.png`: three later attempts; the model reports `the scheduler rejects when.condition on this kind of node` — the plan tool's node shape is hard for gpt-5.4-mini to hit, which is a kernel/tool-schema finding for the QA agent.
- To reproduce: any prompt that gets `Scheduled.` and the `Plan · 1 standing` strip, then Stop, then wait 15 s and read `plan-stop-*` from the driver.
