# Desktop chrome on build 2019

Jacob's asks on 2026-09-18 after Update **0.2.0 (2019)**. One PR on `unarbos/arbos`: [#649](https://github.com/unarbos/arbos/pull/649). Do not publish `v0.2.0`.

Shots: `/home/ubuntu/.cursor/projects/workspace/assets/ffe81426-804b-46f1-9507-2cad8ae7f502.png` (centered chat), `f9c07d08-5be6-4c7a-a6ee-c2c2504c56d6.png` (right list), `387a4bb5-4b45-4f9b-8a7a-ac280f9f23ef.png` (Project over the chat), `1fe1b31d-2574-4bc4-ad41-059e9ec39dfd.png` (bottom +), `17de697f-9126-4dd8-a2bd-b83d6fbab0ae.png` and `4b658afb-6438-410f-b948-4e93e412db97.png` (expand and X).

## Align with the tab strip

The right-side panel control sits on the **same row** as the project tabs (Home, other projects, +).

The right panel's own tabs (the panel's Home / +) sit on that **same row** too. Not a second band under the window tabs.

## Project stays in the panel

A click on **Project** (or any other right-panel item) must **not** open a page in the main column. The chat stays. No panel content covers the chat. No "Back to chat" takeover.

## Remove three controls

- The **+** at the bottom of the side panel
- The **expand panel** button (2×2 grid)
- The panel header **X**. People close the panel with the one remaining panel toggle

## Fullscreen

Native fullscreen must make the chat fill the Space. #542 was not enough. No leftover title bar, extra top inset, or fake maximize.

## PR

[#649](https://github.com/unarbos/arbos/pull/649) on `cursor/desktop-chrome-2019-0690`. Ready for review; CI green. `v0.2.0` stays unpublished.

## Stills

- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/01-tab-strip-closed.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/01-tab-strip-closed.png) — panel toggle on the project tab row, drawer closed
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/02-one-row-open.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/02-one-row-open.png) — panel tabs and toggle on that same row; chat stays
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/03-project-stays-in-panel.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/03-project-stays-in-panel.png) — Project click keeps the chat
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/04-no-extra-controls.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/04-no-extra-controls.png) — no bottom +, no expand grid, no header X
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/05-tab-strip-closeup.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/05-tab-strip-closeup.png) — one row: Home / Place / + / Project / + / toggle