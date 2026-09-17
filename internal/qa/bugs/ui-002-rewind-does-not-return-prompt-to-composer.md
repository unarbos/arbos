# ui-002: Rewind here cuts the transcript but does not put the prompt back in the composer

status: new
severity: medium (the promised edit-and-resend flow is missing; the user retypes the prompt)
scenario: internal/parity/ui_pass.py phase T (`rewind-turn`)
found: UI QA pass 2026-09-13, `cursor/release-integration-52cd` @ 67dcb85 (Rewind is PR #82; not on afa582a)
feature: Rewind here (`desktop/src/view/component/transcript.rs` footer, `ChatSession::rewind`, `Event::Rewound`, `draft_pushed` → `Composer::take_draft`)
fingerprints: none

## Repro

1. Idle chat with at least two finished turns; the last one was `Add a mul(a, b) function to math_utils.py and call it from main.py with mul(4, 5).`
2. Click ↺ Rewind here under that answer (`rewind-turn-<chat>-<turn>`).

## Expected

PR #82: "On `rewound` the pane is cut at the prompt, **the prompt goes back into the composer for editing**, and a notice says what happened."

## Actual

Pane cut (state items 25 → 17), notice `rewound: 17 transcript lines cut; files untouched`, but `state.composer.text` is `""` and the composer shows its placeholder. Reproduced in two runs.

Also: `files untouched` although the turn had edited `math_utils.py` and `main.py` (`files: true` was requested per the PR). Worth a look at the same time.

## Suspected location

`ChatSession::rewind` / `Event::Rewound` handling: `draft_pushed` fires only when the prompt text is found for the turn; the fork/rewind lookup ("find the turn's user prompt when the footer belongs to a tool-only turn") may return nothing for a turn whose answer is `Done.` after tool calls.

## Evidence

- `media/qa-ui/integration-67dcb85/030-rewind-turn.png` (`rewound: 17 transcript lines cut; files untouched`, composer empty)
- results row `rewind-turn`: `items 25 -> 17, composer=''`
