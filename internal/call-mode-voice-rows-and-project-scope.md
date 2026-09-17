---
cursor:
  subagentId: "bc-5691d9b7-e5a5-578a-a19f-a7be750d7671"
---

# Voice rows in the project chat — verification of #490 on main, and the gap PR

Fetched `origin/main` first. Head is `aaf3dbe9` (Merge #495). #490 is on that history (`0922559d`). Remote is `https://github.com/unarbos/arbos`. Did not reopen the merged branch. Did not touch `hub.toml` or `v0.2.0`.

The earlier brief on this path (from `bc-32d10b66`) is the gateway contract. This file is now the desktop-acceptance status for Jacob's ask.

## Jacob's acceptance (not weakened)

1. User speech and Live speech appear as chat rows in **that** project's chat (the project the call is attached to).
2. Those voice rows are **display-only**. They must not wake or steer the kernel.
3. A line **typed** in the composer during the call **does** wake or steer the kernel.
4. Home (Main) and other projects must **not** receive the rows.

## What #490 on main already does (kept)

| Acceptance | On `aaf3dbe9` | Where |
|---|---|---|
| User speech → a row | `transcript.final` → `caller.said` → `ChatSession::voice_prompt` | `desktop/src/voice_ws.rs`, `desktop/src/view/root.rs` `start_call_mirror` |
| Live speech → a row | accumulated `response.transcript` on `response.done` → `model.reply` → `voice ·` notice, unless `speaker == "narrator"` or it repeats a recent `narrator.say` | same |
| Display-only | `voice_prompt` pushes a local `User` card and `flush()`s the desktop record. It does not call `prompt` / `steer` / `session.prompt`. | `desktop/src/model/session.rs` |
| Typed line wakes the kernel | Composer `Submit` → `workspace.send` → `ChatSession::send` → `prompt` or `steer` → `Frame::User`. Only `/voice` goes to the gateway. | `desktop/src/view/detail.rs`, `session.rs` |
| Same spoken line not doubled | `awaiting_echo` merges the kernel's later `user` event | `session.rs` `foreign_prompt` |

Gateway half (`typed-during-call.toml`): spoken line is `channel: voice`; typed line is `channel: text`, `kind: steer`. Desktop does not send `text.input` for a normal composer send.

## Gaps on merged #490 (why a new PR)

1. **Short Live replies were dropped.** `call_line` treated any `model.reply` of three words or fewer as an ack and drew nothing. "hey" → "hey" (the documented small-talk case) would not appear. That weakens acceptance 1.
2. **Home / other projects could receive the rows.** `start_voice_mirror` (Fn dictation or `/voice`) keeps draining the same queue while a call is live, because a call keeps `phase` set. It writes to `active_id()` — whichever tab is in front. After a dictation, or if the user switches to Home mid-call, spoken lines could land there. `start_call` also bound `call.session` to `focused_agent()`, so a worker chat could take the rows instead of the project's main chat.

## New PR (off latest main)

[#501](https://github.com/unarbos/arbos/pull/501) — branch `cursor/live-voice-rows-scope-6983`

Fixes on that branch:

- Draw every `model.reply` as `voice ·`, including short Live small talk. Narrator "On it." stays `narrator.say/ack` (heard, not read).
- A call writes only to the front **project's main chat** (`project.main_session()`).
- Dictation mirror does not start, and stops, while a call is live. It does not drain the queue then. Home and other projects cannot receive call frames.

Typed composer send is unchanged: still `ChatSession::send` to that chat's kernel.

## Proof I ran

- `cd desktop && cargo check` — clean (one unused-mut warning in `voice_ws.rs` that predates this).
- No desktop binary / Xvfb harness this turn (would need `ARBOS_DESKTOP_BIN` and the mock gateway). The two harnesses on main still cover the #490 path: `tests.desktop_call` (spoken cards + kernel user-frame cap) and `tests.desktop_call_refused` (other project's kernel got 0 user frames).

## Display rules

Updated only the display-rules section of `docs/gpt-live-context.md` in this store: short Live replies are drawn; rows stay in the attached project's main chat.
