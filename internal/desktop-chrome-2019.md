---
cursor:
  subagentId: "bc-85782d4e-78b3-55d7-80ba-a25615cd0690"
---

# Desktop chrome 2019 — what landed

PR: https://github.com/unarbos/arbos/pull/649 — ready for review; CI green. `v0.2.0` stays unpublished.
Branch: `cursor/desktop-chrome-2019-0690`
Commit: `8d6cb643`

Driver stills on Linux (`arbos-desktop` debug, private XDG, no kernel):

- `toggle-panel` is a child of `tab-bar`
- With the drawer open, `panel-tabs` / `panel-tab-0` / `panel-new-tab` are children of `tab-bar` too
- `pane` and `showing` stay `chat` after opening Project
- `panel-close`, `panel-expand`, `new-subchat`, `page-back-to-chat` are absent
- Panel foot is `search-chats` only

Fullscreen is macOS AppKit (`keep_macos_glass`: title hidden, full-size content, zero extra safe-area inset). Not exercised on this Linux box. #542 is the prior pass; see [fullscreen-macos.md](fullscreen-macos.md).

Media: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/desktop-chrome-2019/`
