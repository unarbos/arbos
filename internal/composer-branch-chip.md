---
cursor:
  subagentId: "bc-97ab6331-b8f3-5844-b20d-2004eb4e2b9d"
---

# Composer branch chip gone (#656)

New PR from current main (`1beec0a1` at branch time). Branch `cursor/remove-composer-branch-chip-2b9d`. Head `3b3d515`. Merged as `93e335e2`.

## Documents

- One line on `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/desktop-project-page-composer.md`: the git-branch chip under the composer is gone. The old “branch pill stays” bullet is also gone so the page does not contradict itself.
- Stills: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/composer-branch-chip/` (`02`–`05`).

## Jacob's shot

Named `60d96653-8e54-4e18-8ede-e2ca0131ce77.png`. Path `/home/ubuntu/.cursor/projects/workspace/assets/` has no such file on this VM. `01-branch-chip.png` was not copied.

## Code

- `composer-branch` removed from `context_row` in `desktop/src/view/detail.rs`.
- The row is omitted when it has nothing else (no reconnect text, no voice/call, no spinner). Idle empty composer has no leftover strip.
- `This Mac` / `This Computer` stay gone. Typed `clear` / `/clear` stay. Jev untouched. `branch_of` still feeds the project panel header.

## Linux check

Build `0.2.0 2040 3b3d515` on a repo checked out as `fix/initial-workspace-layout`. Driver: no `composer-branch`, no `composer-machine`, no `composer-context`. Typed `clear` recentered the composer. Stills:

- `02-project-composer.png` / `04-project-composer-closeup.png` — composer at the foot, no chip
- `03-empty-centered.png` / `05-empty-centered-closeup.png` — “Plan, search, build anything” in the middle, no chip

## PR

https://github.com/unarbos/arbos/pull/656 — ready, then merged. CI green on the PR. Do not publish `v0.2.0`. Mac / dev channel publishes from main after that CI.
