# qal-j10: when the turn-start mark cannot be written, `undo` resets to an older turn's HEAD — destroying committed work — and says "restored"

- **Closed 2026-09-17 05:25 UTC against #392 @ `a0f2a92d`** (`arbos-kernel 0.2.0 a0f2a92dbf3a`). `sw-02` passes in the honest way: the turn's own mark could not be written, the transcript carries *"Checkpoint not written for this turn: could not clear the undo mark … Permission denied"*, and `undo` refuses — *"no checkpoint for this turn: the mark on disk is from the turn at line 2, this turn started at line 8 (its own mark was not written); nothing reset"* — with commit A and `a.txt` intact and HEAD untouched. Controls: #390 @ `9ade320f` and `main` @ `7f6a6b9a` still reset to the stale mark and destroy A. The ordinary path was checked beside it (`sw-04`): a healthy `undo` after a committed turn drops the tracked edit and the turn's own draft, keeps the commit, and says `restored …` on all three kernels — the refusals did not eat it.
- Measured at: `main` @ `7f6a6b9a` (`arbos-kernel 0.2.0 7f6a6b9a06bc`); replay provider, no model. Code read at `main` @ `0f2a8bc6`.
- Family: qal-j08's — a best-effort write that fails silently, a record that then looks valid, and a destructive step that trusts it.
- Severity: **high, destructive.** `undo` exists to drop the current turn's work. With a stale mark it runs `git reset --hard <older HEAD>` and `git clean -fd`, removing commits and files from turns the user meant to keep, and reports success.
- Scenario: `sw-02-stale-undo-mark-resets-past-committed-work`; rollout `internal/qa/rollouts/20260917T044404Z-sw-02-…`.

## Repro

Repository on branch `work`, HEAD0. Turn 1: the agent commits A (`a.txt`). Before turn 2 the mark file `.arbos/runtime/checkpoint` cannot be written (injector: file and `runtime/` made read-only; in life: a full disk — the very condition under which `inflight::start` documents itself as best-effort too — or an unwritable `runtime/`). Turn 2: the agent commits B (`b.txt`), then calls `undo`.

- Turn 2 started at HEAD=A (`3f468eed`); the mark still said HEAD0 (`a1ab039c`), written at turn 1's start.
- `undo` → `git reset --hard a1ab039c` + `git clean -fd`: branch back at HEAD0, `git log` shows `ignore`, `start` — **commit A is gone**, `a.txt` gone. Tool result: `restored a1ab039ce0964c500aeeb04a68fcef740501ce05`.

## Expected

`undo` acts only on a mark it knows belongs to this turn. The mark should carry the turn (transcript line or turn id) it was written for, and `undo` should refuse — "no checkpoint for this turn; nothing reset" — when the mark is missing, stale, or its write failed. The write failure itself should be said once on the transcript.

## Actual

`tools/git.rs::snapshot` writes the mark with `let _ = std::fs::write(...)`; `snapshot_turn` is called with `let _ =` from `turn.rs:562`. `undo` reads whatever is in the file and resets to it.

## Suspected location

- `crates/arbos-engine/src/tools/git.rs::snapshot` (write the turn line beside the sha; surface the failure) and `::undo` (verify the mark's turn before `reset --hard`; never `clean` on doubt).
- In the same family, not driven tonight: `arbos-engine/src/inflight.rs::start` — "a full disk does not stop the tool, it only loses this safety net": with the record unwritten, a kernel death mid-tool re-runs the command (the qal-j02 hole returns silently); `jobs.rs` `killed`/`exit` markers written with `let _` — a job reads as still running for ever when the write fails (misreport).

## Fix

#392 @ `a0f2a92d` (see the closing line). Regression check: `sw-02` (HEAD after `undo` = HEAD at turn 2's start; `a.txt` present; or `undo` refuses with a reason).
