# ui-004: Skip once sent an answer for ask id "ask"; the kernel refused it and the turn hung until Stop

status: new (seen once in four Skip attempts; flaky)
severity: medium (when it happens the chat is stuck busy; every later message queues behind it)
scenario: internal/parity/ui_pass.py phase Q (`ask-skip`), first run 10:19
found: UI QA pass 2026-09-13, `cursor/release-integration-52cd` @ 67dcb85
feature: question card / ask ids (PR #81 `answer_allowed`, desktop `skip_ask`)
fingerprints: none

## Repro (as it happened)

1. Ask prompt (see ui-003). Click option A, click Other…, click A again, click **Continue** → answered `alpha`, fine.
2. Send the ask prompt again. When the card shows, click **Skip** at once.

## Expected

Card closes, turn continues (or ends) within seconds.

## Actual

Transcript: `ask` then a failed notice **`answer refused: answer names ask "ask", which this kernel never issued; the pend…`**. The card stayed on screen in the still taken 1.5 s later, then went away, and the session stayed `turn_open` for the next 4 minutes. Every prompt typed afterwards was queued (`Interrupt now ×` rows). Stop ended it.

The literal id `"ask"` suggests the desktop sent the question's *kind* (or a stale/empty id) rather than `question.id`. Did not reproduce in three later attempts, so it may depend on the second card arriving while the first ask's state is still being cleared.

## Suspected location

`desktop/src/session.rs` skip path: the ask id used for `Frame::Answer` when `chat.questions` was just replaced; kernel `answer_allowed` (PR #81) correctly refuses, but the desktop does not clear `turn_open` on that `error` frame.

## Evidence

- `media/qa-ui/integration-67dcb85/run1-027-ask-skip-refused.png` (card still up after Skip)
- the 10:28 screen in the same run: refused notice and three queued rows (described in the QA doc; the run was aborted and re-run)
