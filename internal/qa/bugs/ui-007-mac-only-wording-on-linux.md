# ui-007: macOS wording shown on Linux: "this Mac", "Hold Fn to talk", opener "This Mac"

status: new
severity: low (copy)
scenario: internal/parity/ui_pass.py phase C (`composer-voice`), phase B (`opener-row-0`)
found: UI QA pass 2026-09-13, both branches
feature: voice button, opener step 1
fingerprints: none

## Repro

Linux build. Click the mic; hover the mic; open the Opener (cmd-t / +).

## Expected

Platform-neutral copy, or the Linux truth ("voice dictation is not available on Linux", "this machine").

## Actual

- Notice after clicking the mic: `voice failed: voice dictation only works on this Mac` (+ a Retry link).
- Mic tooltip: `Hold Fn to talk`.
- Opener step 1 lists the local machine as `This Mac`.

## Suspected location

`composer.rs` voice tooltip/notice strings; `opener.rs` local-machine label.

## Evidence

- `media/qa-ui/integration-67dcb85/011-composer-voice.png`, `media/qa-ui/pr71-afa582a/011-composer-voice.png`
- `media/qa-ui/pr71-afa582a/056-opener-escape.png` (opener step 1: `This Mac`, `Browse folders…`)
