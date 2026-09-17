# ui-009: clicking the "Worked" fold does nothing in a forked chat

status: new (2 of 3; expands fine in a fresh chat)
severity: low
scenario: internal/parity/ui_pass.py phase T (`work-*`)
found: UI QA pass 2026-09-13, both branches (the pass forks before the edit turn)
feature: transcript folds (`transcript.rs` `work-<ix>`)
fingerprints: none

## Repro

1. Finish a turn, click ⑂ fork under it (new chat "… copy").
2. In the copy, send the edit prompt (`Add a mul(a, b) function …`). Wait for `Worked Ns / Done.`
3. Click the `Worked` line.

## Expected

The fold opens to the summary line (`Edited 2 files, explored 3 files, 3 searches, ran 3 commands`) and diff badges show, as it does in a fresh chat (`media/qa-ui/integration-67dcb85/102-v-work-after-click.png`).

## Actual

Nothing changes on click (footer position and element list identical before/after). In the integration run the diff badges (`+8 −3`) were also missing from the fold line; on afa582a they showed but the fold still did not open.

## Suspected location

Fork copies the transcript lines but not the tool detail needed by the fold, or the fold's expanded flag is keyed by an id that collides after the fork.

## Evidence

- `media/qa-ui/integration-67dcb85/028-work.png` (after click, no change) vs `media/qa-ui/integration-67dcb85/102-v-work-after-click.png` (fresh chat, expanded)
- `media/qa-ui/pr71-afa582a/029-work.png`
