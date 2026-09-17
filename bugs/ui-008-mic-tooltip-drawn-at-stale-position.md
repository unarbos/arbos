# ui-008: the mic tooltip is drawn mid-window once the composer has moved to the bottom

status: new
severity: low (visual)
scenario: internal/parity/ui_pass.py phase C (`composer-voice`, hover)
found: UI QA pass 2026-09-13, both branches
feature: tooltips on the composer discs (`composer.rs`, gpui tooltip anchoring)
fingerprints: none

## Repro

1. Fresh chat (composer centred). Hover the mic: tooltip `Hold Fn to talk` sits beside the button. Good.
2. Send one prompt so the composer drops to the bottom of the pane. Hover/click the mic again.

## Expected

Tooltip beside the mic, at the bottom.

## Actual

Tooltip appears at about the height where the centred composer used to be (y ≈ 250 in a 830-px-high window), roughly 170 px above the button. Same on both branches.

## Suspected location

Tooltip anchor uses a cached bounds from the first layout (the "empty chat" centred composer) and is not refreshed when the composer re-parents to the bottom.

## Evidence

- `media/qa-ui/integration-67dcb85/011-composer-voice.png` (tooltip mid-window, composer at bottom)
- `media/qa-ui/pr71-afa582a/011-composer-voice.png`
- `media/qa-ui/integration-67dcb85/125-v-tooltip-mic-hover.png` (correct placement while the composer is centred)
