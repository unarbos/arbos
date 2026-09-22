---
cursor:
  subagentId: "bc-6a5c5d28-822a-512e-96cb-b1dbc447e73d"
---

# Conversation bar chrome — 2026-09-22

## Follow-up: + on the quiet bar

Draft PR: https://github.com/unarbos/arbos/pull/815
Branch: `cursor/conversation-plus-e73d`
Repo: unarbos/arbos
Not merged. Not published. No paused loop was resumed.

#814 is on `main` (merge `7aaa4630`) and in Mac 2410. This pass adds a `+` on that same quiet bar.

The `+` on the dark project strip still opens a project tab (`⌘T` / `NewTab`).
The `+` on the quiet conversation bar starts another chat in the project in front (`⌘N` / `NewSession`). It does not open a project tab.

Overflow width now reserves room for this `+`, so `+N` still appears on a narrow window.

## What landed (#814, already on main)

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

## Proof of the +

Driver on a live Linux window, Xvfb :99, isolated HOME/XDG, two tabs (Home and Demo).
`cd desktop && cargo build --locked` passed.

Click `conversation-new` (the quiet-bar `+`):

- sessions in the front project: 1 → 2 → 3
- child sessions: 0 → 1 → 2
- project count stayed 2
- tab ids stayed `tab-0` and `tab-1`
- project picker (`opener_open`) stayed closed
- agent chips `conversation-agent-2` then `conversation-agent-3` appeared
- `conversation-new` and `new-tab` both stayed on screen
- narrow window still showed `conversation-overflow` (`+8`) and the conversation `+`

Old chrome stayed gone: no `toggle-panel`, `panel`, `pill-agents`, `composer-call`, `chat-header`.

Stills from #814:

- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/01-main-under-projects.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/01-main-under-projects.png) — Home · Demo · + on the dark strip; ● Main on the quiet bar; no panel, no handset
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/02-agents-after-divider.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/02-agents-after-divider.png) — ● Main, divider, ⚙ Delegate 1–4
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/03-agent-selected.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/03-agent-selected.png) — a click on an agent chip; no chat-header crumbs
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/04-overflow.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/04-overflow.png) — +8 after the visible chips
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/05-overflow-open.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/05-overflow-open.png) — +N menu lists the hidden chats
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/06-home-tab.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/06-home-tab.png) — Home in front; same quiet bar; still no panel

Stills from this + pass:

- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/07-plus-on-quiet-bar.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/07-plus-on-quiet-bar.png) — ● Main and a `+` on the quiet bar; Home · Demo · + still on the dark strip
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/08-plus-clicked-new-chat.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/08-plus-clicked-new-chat.png) — click `conversation-new`; Delegate 1 appears; tooltip says New conversation ⌘N; still two project tabs
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/09-plus-again.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/09-plus-again.png) — second click; Delegate 2 selected; still Home and Demo only
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/10-overflow-with-plus.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/10-overflow-with-plus.png) — narrow window: Delegate chips, +8 overflow, then the conversation `+`

This machine had no kernel binary, so the composer said reconnecting. That does not change the chrome.

## Follow-up: new project opens on an empty chat

Same draft PR: https://github.com/unarbos/arbos/pull/815
Not merged. Not published. No paused loop was resumed.

A new project no longer shows the kickoff splash (project glyph, blurb, View Project Page). It opens on the empty chat: title and composer in the middle, the same as typing `clear`.

A missing kernel used to leave a connection notice on the transcript, which kept the project-head splash up. Notices are not a conversation. The first turn is the person's. The kernel is no longer asked for a setup greeting.

## Proof of empty chat on a new project

Driver on a live Linux window, Xvfb :99, isolated HOME/XDG.

Seeded a fresh folder as a project tab, then created another through the picker (`⌘T` → Create).

Both landings:

- present: `chat-title`, `composer-field`, `conversation-bar`, `conversation-main`, `conversation-new`
- absent: `kickoff`, `kickoff-greeting`, `project-head`, `project-head-page`, `project-page`
- pane stayed `chat`

Opener created `/tmp/arbos-empty-chat-new-project/home/from-opener` and added a third tab. Still the empty chat.

Stills:

- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/11-new-project-empty-chat.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/11-new-project-empty-chat.png) — fresh project: New chat + composer in the middle; no project-head splash
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/12-opener-create.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/12-opener-create.png) — picker offering Create on a new folder
- [/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/13-opener-new-project-empty-chat.png](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/conversation-bar/13-opener-new-project-empty-chat.png) — after Create: from-opener tab in front; same empty chat

This machine had no kernel binary, so the composer said reconnecting. That does not change the chrome.

## Left parked

Drawer, terminal, browser, call view, and the kickoff splash stay in the crate behind `allow(dead_code)`. A later pass can restore them without a hunt.
