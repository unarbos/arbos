---
cursor:
  subagentId: "bc-85782d4e-78b3-55d7-80ba-a25615cd0690"
---

# Desktop top bar follow-up (#654)

New PR from current main (has #649). Branch `cursor/desktop-topbar-0690`. Commit `1768ec83`.

## Documents

- Updated plan: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/desktop-chrome-2019.md` — six-rule list, #654 link, Linux stills.
- New stills dir: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/` (`01`–`06`).
- First-pass stills stay under `media/desktop-chrome-2019/`.
- First-pass report stays at `internal/desktop-chrome-2019.md` (not overwritten).

## Jacob's shots

Named `0524ce21-…png` and `511da708-…png`. Path `/home/ubuntu/.cursor/projects/workspace/assets/` does not exist on this VM. Not copied.

## Code (already on the branch)

- Chat column always `flex_1`; empty-chat spacer gone; composer at the foot.
- Main chat has no header band and no Clear button. Typed `clear` / `/clear` still call `hide_chat_view`.
- `window-expand` is pinned on the tab strip. `panel-close` / `panel-expand` stay gone.
- `show_project` still opens panel tab 0. Pane stays `chat`.

## Linux check

Build `0.2.0 2037 1768ec8`. Driver: no `chat-clear`, no `panel-close`, no `panel-expand`. `toggle-panel` and `window-expand` on `tab-bar`. After expand, `window-expand` stayed at `(1060, 6)`.

## PR

https://github.com/unarbos/arbos/pull/654 — draft until CI is green. Do not publish `v0.2.0`.
