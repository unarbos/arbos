# qal-j11: `undo` deleted every untracked file in the project — including the user's own files from before Arbos ever ran

- Measured at: `main` @ `7f6a6b9a` (`arbos-kernel 0.2.0 7f6a6b9a06bc`) — fails; #390 @ `9ade320f` (`arbos-kernel 0.2.0 9ade320f958a`) — passes. Replay provider, no model.
- Found by #390's author beside qal-j08 ("the fourth hole"); pinned here as the QA loop's own scenario because it needs no rewind, no fresh place and no destructive request from the user — a model calling `undo` after an ordinary turn was enough.
- Severity: **the worst of the family.** The user's notes, a scratch folder, a photo — anything untracked in the project directory — gone, with `restored <sha>` as the only word said.
- Scenario: `sw-03-undo-keeps-the-users-own-untracked-files`; rollout `internal/qa/rollouts/20260917T045255Z-sw-03-…` (fails: `my-notes.txt`, `scratch/ideas.md`, `holiday.jpg` deleted) and `20260917T045214Z-sw-03-…` (#390: all three survive).
- **Closed 2026-09-17 04:55 UTC against #390 @ `9ade320f`.** The mark now records whether the tree was saved, clean, or failed (`head\\n<work|clean|error:…>`); `undo` restores tracked files and, without a known tree, leaves untracked files alone and says so.

## Repro

A project with untracked files the user made before opening it in Arbos: `my-notes.txt`, `scratch/ideas.md`, `holiday.jpg`. The agent runs one turn (`echo draft > draft.txt`) and calls `undo`.

`main` @ `7f6a6b9a`: `undo` → `git reset --hard <mark>` + `git clean -fd -e .arbos` → the three files are deleted; tool result `restored 6843bf99…`.
#390 @ `9ade320f`: the three files survive; `draft.txt` (the turn's own untracked work) is handled by the checkpoint's record.

## Expected

`undo` removes what the turn did, never what the user had. When the kernel does not know the tree the turn started from, it must not `clean`.

## Actual (before #390)

The mark held HEAD alone; `undo` trusted it and cleaned the whole tree.

## Suspected location

`crates/arbos-engine/src/tools/git.rs::undo` (fixed in #390).

## Fix

#390 @ `9ade320f`. Regression check: `sw-03`.
