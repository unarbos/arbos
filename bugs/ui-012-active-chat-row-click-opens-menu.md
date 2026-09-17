# ui-012: clicking the already-active chat row in the sidebar opens its … menu

status: new
severity: low (surprising; may be intended)
scenario: internal/parity/ui_pass.py phase B sidebar (`session-N`)
found: UI QA pass 2026-09-13, `cursor/release-integration-52cd` @ 67dcb85 (sidebar layout)
feature: sidebar chat rows (`sidebar.rs` `session-<ix>`, `session-dots-<ix>`)
fingerprints: none

## Repro

Two chats in the sidebar. Click the centre of the row that is already active.

## Expected

Nothing (already active), or focus moves to the composer.

## Actual

The row's Copy / Fork / Archive / Delete menu opens (`menu_open: true`), anchored at the … button. Clicking a non-active row selects it without a menu. So a second click on a row is a menu trigger.

## Evidence

- `media/qa-ui/integration-67dcb85/111-v-session-row-click.png`
- results row `session row`: `active 7 -> 7, menu_open=True`
