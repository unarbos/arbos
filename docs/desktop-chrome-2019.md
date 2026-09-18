# Desktop chrome on build 2019

Jacob's asks on 2026-09-18 after Update **0.2.0 (2019)**. Do not publish `v0.2.0`.

[#649](https://github.com/unarbos/arbos/pull/649) is on `main`. The top bar was still wrong.

Follow-up: [#654](https://github.com/unarbos/arbos/pull/654) from that main, branch `cursor/desktop-topbar-0690`. Mac channel.

## The rule

1. **The chat fills the column.** Messages start at the top of the chat and run down to the composer. They do not float in the middle with empty bands above or below. The composer stays on screen at the foot of the column.
2. **The side panel toggle sits on the window tab strip.** Same row as Home, the other project tabs, and +.
3. **No Clear button on the top bar.** Typed `clear` and `/clear` still hide the transcript in this window. The file on disk does not change.
4. **No X on the panel header.** Close is the tab-strip toggle (⌘B).
5. **No four-squares expand on the panel header.** One expand control is pinned at the very top-right of the window, on the tab strip. Widening the drawer does not move it. The same control collapses. It is never a second header.
6. **No panel ever covers the chat.** Project, a terminal, a file, a page — all stay in the right drawer.

## First pass (#649)

The right-side panel control and the panel's own tabs sit on the same row as the project tabs. A click on Project stays in the drawer. The bottom +, the expand in the panel header, and the header X went. Native fullscreen hides the leftover title band.

## This pass (#654)

The empty-chat spacer that cut the transcript short is gone. The chat fills the column. The Clear button is gone; typed `clear` / `/clear` stay. The four-box is back as `window-expand` on the tab strip, pinned at the window's top-right. It does not live in the panel header, so widening the drawer cannot move it.

`v0.2.0` stays unpublished.

## Stills

Jacob's shots were named `0524ce21-c1b0-4a5b-b685-fd882ccec724.png` (cutoff) and `511da708-c817-499d-a15e-c0effc43fb37.png` (panel). They were not on disk at the given `assets/` paths in this VM, so they could not be copied yet.

Linux after the fix:

- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/01-chat-fills.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/01-chat-fills.png)
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/02-strip-toggle-expand.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/02-strip-toggle-expand.png)
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/03-no-clear-no-header-x.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/03-no-clear-no-header-x.png)
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/04-expand-stays-pinned.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-topbar/04-expand-stays-pinned.png)
