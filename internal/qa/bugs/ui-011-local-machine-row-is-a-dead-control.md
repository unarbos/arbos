# ui-011: the "Local" machine row under the composer is interactive but does nothing

status: new
severity: low
scenario: internal/parity/ui_pass.py phase C (`composer-machine`, `composer-context`)
found: UI QA pass 2026-09-13, both branches
feature: composer footer (`detail.rs` `composer-machine`, `composer-context`)
fingerprints: none

## Repro

Hover `Local` under the composer: tooltip `Where this agent runs`. Click it.

## Expected

Either a machine picker (the Opener's step 1) or a plain label with no hit target.

## Actual

Click accepted, no state change, nothing opens (`menu_open`, `opener_open` unchanged). The element reports `interactive: true` to the driver.

## Evidence

- `media/qa-ui/integration-67dcb85/010-composer-machine.png`, `media/qa-ui/integration-67dcb85/009-composer-context.png`
- `media/qa-ui/pr71-afa582a/010-composer-machine.png`
