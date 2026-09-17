# ui-006: follow-ups queued from the composer never get the Send now / Edit / Remove row

status: new
severity: low (the controls exist only for kernel-side inbox nodes; from the app you only get "Interrupt now" and ×)
scenario: internal/parity/ui_pass.py phase T (`queue-row`, `followup-*`, `force-queue`, `unqueue-*`)
found: UI QA pass 2026-09-13, `cursor/release-integration-52cd` @ 67dcb85
feature: follow-up rows (PR #83 `detail.rs::followups`, ids `followup-{send,edit,remove}-<chat>-<node>`) vs the desktop queue (`force-queue`, `unqueue-<ix>`)
fingerprints: none

## Repro

1. Long turn running. Type `Afterwards say hello.` + Enter. Type another + Enter. Also try cmd-Enter.

## Expected

PR #83 promised queued follow-ups as their own rows with **Send now · Edit · Remove**.

## Actual

Every message typed while busy lands in the desktop's own queue: a row per message with **Interrupt now** (`force-queue`) and **×** (`unqueue-N`). No `followup-*` element ever appears (state: `queued: 1`, `followups-head` absent). There is no Edit. cmd-Enter does nothing (text stays in the composer). The kernel row only exists when the message reaches the kernel while it is busy, which the app's composer never does.

When the queue is flushed, the messages are joined into one user bubble without a separator: `Also say thanks.Then say goodbye.` (`media/qa-ui/integration-67dcb85/022-stop-word.png`, top bubble).

## Suspected location

`detail.rs::submit` (busy → `chat.queue.push`) vs `followups()` (inbox nodes). Either send the queued message to the kernel as an inbox node, or give the desktop row the same three controls; join queued texts with a newline.

## Evidence

- `media/qa-ui/integration-67dcb85/016-queue-row.png`, `media/qa-ui/integration-67dcb85/017-composer-field-cmd-enter.png`, `media/qa-ui/integration-67dcb85/022-stop-word.png`
- `media/qa-ui/pr71-afa582a/016-queue-row.png` (same row, labelled `Force` on afa582a)
