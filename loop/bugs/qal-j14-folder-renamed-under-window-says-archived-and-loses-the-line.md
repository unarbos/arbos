# qal-j14: rename the project folder under a running window and the chat says "this agent is archived" — nothing was archived, and the typed line is lost without a word

- Measured at: #392 @ `ac831935` (`arbos-kernel 0.2.0 ac831935e1de`) with the `efcab58f`-era desktop build (`target-desktop-pr329`, driver from `main`); the kernel half also on `main` @ `7f6a6b9a`.
- Class: misreport with a loss under it — the after-something-went-wrong family. A folder renamed on disk (Finder, `mv`, a sync tool moving a project) is ordinary; the kernel's response is right (#377: a kernel whose `.arbos/` is gone from under it stops itself, so no ghost folder is minted at the old path); what the person is told is wrong, and the line they typed is gone.
- Scenarios: `af-01-folder-renamed-under-a-running-kernel` (kernel half; rollout `internal/qa/rollouts/20260917T061739Z-af-01-…`), `af-03-desktop-folder-renamed-under-the-window` (desktop half; `20260917T062613Z-af-03-…`). `af-02-two-windows-on-one-place` beside them passes clean: one kernel, one line, both windows see it, Stop from either ends the turn.

## Repro

Desktop window open on `moving-project`, kernel serving, one turn done. On disk: `mv moving-project moving-project-renamed`. Type a line.

- Kernel (stderr): *"arbos-kernel stopping: the place's .arbos store is gone (…/moving-project/.arbos); its jobs end with this kernel"* — correct, and no ghost `.arbos/` at the old path.
- Window: connection `lost`; one new chat item, a notice: **"this agent is archived: its history stays, but it takes no more messages."** Nothing was archived. The typed line is on no transcript — not in the renamed folder, not at the old path.
- Headless (`af-01`): the line typed after the rename is dropped; the only word is the kernel's stderr line, which no window shows.

## Expected

The window says what happened — *"The project folder moved or was renamed (it is no longer at …/moving-project). Open it from its new place to continue."* — and the typed line is either delivered into the renamed folder's transcript (the kernel knows its inode; the desktop knows the old path) or handed back to the composer, not dropped. Nothing about archiving.

## Actual

The kernel's exit on a lost store reaches the desktop as a closed connection; the desktop's explanation for "kernel gone, no error frame" is the archived-agent notice. The line in flight is neither delivered nor returned.

## Suspected location

- `desktop/src/model/session.rs` — where a lost connection with no error frame is rendered as "this agent is archived"; distinguish "the folder is gone from this path" (`std::fs::metadata(place).is_err()`) and say so.
- `crates/arbos-kernel/src/serve.rs` — the stop on a lost store: a last `error` frame to attached clients ("the place's folder is gone from …") before exit, so windows can say something true; and a hand-back of any in-flight user line.

## Fix

Not started. Regression checks: `af-03` (`af-03-wrong-explanation`: a notice that says archived without saying moved fails) and `af-01` (`af-01-line-lost-in-silence`).
