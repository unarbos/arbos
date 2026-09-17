# ui-001: Stop (button or stop word) shows a failed "no reply from the kernel · Retry" notice instead of "Stopped by you"

status: new
severity: medium (the user pressed Stop and is told the kernel failed, with a Retry button that would re-run what they just stopped)
scenario: internal/parity/ui_pass.py phase T (`composer-stop`, `stop-word`, `stop-notice`, `stop-word-notice`)
found: UI QA pass 2026-09-13, `cursor/release-integration-52cd` @ 67dcb85 and `cursor/window-layout-tabs-right-panel` @ afa582a
feature: desktop transcript notices (`desktop/src/view/detail.rs::submit`, `session::interrupt_label`, `transcript.rs::interrupt_label_of`; PR #83)
fingerprints: none

## Repro

1. Send `Run the shell command \`sleep 40\` and then reply with the single word done.`
2. While it runs, click the Stop disc (`composer-stop`). Or type `stop` + Enter (integration only; stop words are #83).

## Expected

PR #83: an interrupted turn says so — `Stopped by you` (Stop button, stop word). No failed notice, no Retry.

## Actual

Integration: two notices under the prompt: **`no reply from the kernel · Retry`** (drawn as a failed notice) and `Interrupted: stop during model call`. The stop word gives the same pair. `Stopped by you` appeared only once, when Force ("Interrupt now") cut a turn during a tool call (`media/qa-ui/integration-67dcb85/021-stop-notice.png`, top of the transcript).
PR 71 (afa582a): only `no reply from the kernel · Retry`.

The label depends on where the stop landed (tool vs model call), and the "no reply" failure notice is emitted for a stop the user asked for.

## Suspected location

`desktop/src/view/detail.rs` / `kernel.rs`: a turn that ends with no assistant text is treated as a failed turn ("no reply from the kernel") before the `interrupted` line is read. `interrupt_label` only maps the tool-phase reason to `Stopped by you`.

## Evidence

- `media/qa-ui/integration-67dcb85/021-stop-notice.png`, `media/qa-ui/integration-67dcb85/023-stop-word-notice.png`
- `media/qa-ui/pr71-afa582a/021-stop-notice.png`
- state dump: last items `[notice failed=true "no reply from the kernel", notice "Interrupted: stop during model call"]`
