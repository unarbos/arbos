---
cursor:
  subagentId: "bc-6a5c5d28-822a-512e-96cb-b1dbc447e73d"
---

# Conversation bar chrome — 2026-09-22

Draft PR: https://github.com/unarbos/arbos/pull/814
Branch: `cursor/conversation-bar-e73d`
Repo: unarbos/arbos
Not merged. Not published. No paused loop was resumed.

## What landed

Workspace → Project → Conversations.

The dark top strip stays projects (Home · Project · +).
A quieter second strip is conversations: ● Main, a divider, then ⚙ agent chats.
Many chats collapse to +N. No underlines. The chat itself has no switcher.

Removed from the desktop window:

- right side panel and its tab-strip toggle
- Agents / Working bubble above the composer
- terminal and browser chrome (drawer cards and panel tabs)
- desktop call chrome (handset, View › Call / End / Mute)

The hosted voice gateway is still in the crate. `start_call` is parked, not deleted.

## Proof

Driver on a live Linux window, Xvfb :99, isolated HOME/XDG, two tabs (Home and Demo).
`cd desktop && cargo build --locked` passed.

Element facts from the driver:

- present: `conversation-bar`, `conversation-main`, `conversation-divider`, `conversation-agent-*`, `conversation-overflow`
- absent: `toggle-panel`, `panel`, `pill-agents`, `pill-working`, `composer-call`, `chat-header`

Stills:

- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/01-main-under-projects.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/01-main-under-projects.png) — Home · Demo · + on the dark strip; ● Main on the quiet bar; no panel, no handset
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/02-agents-after-divider.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/02-agents-after-divider.png) — ● Main, divider, ⚙ Delegate 1–4
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/03-agent-selected.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/03-agent-selected.png) — a click on an agent chip; no chat-header crumbs
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/04-overflow.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/04-overflow.png) — +8 after the visible chips
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/05-overflow-open.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/05-overflow-open.png) — +N menu lists the hidden chats
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/06-home-tab.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/06-home-tab.png) — Home in front; same quiet bar; still no panel

This machine had no kernel binary, so the composer said reconnecting. That does not change the chrome.

## Left parked

Drawer, terminal, browser, and call view code stay in the crate behind `allow(dead_code)`. A later pass can restore them without a hunt.
