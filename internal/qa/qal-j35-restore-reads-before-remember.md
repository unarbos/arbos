---
cursor:
  subagentId: "bc-0b17760f-ba67-58b7-b443-5471e314147b"
---

# qal-j35 follow-up: restore reads last-chat before remember can write

PR: https://github.com/unarbos/arbos/pull/679 (ready, CI green)
Branch: `cursor/restore-last-chat-before-remember-147b`
Repo: `unarbos/arbos`

## Cause

#675 wrote last-chat correctly when leaving a sub-chat in front. On relaunch, startup focused the project's main chat, that focus called `remember_session`, and `[last."<place>"]` was rewritten to the main chat before restore read it. `left_here` then compared against the clobbered entry and stayed on root.

The write that erased the memory was `remember_session` inside `new_session_in` (and `session_connected` for the focused main chat). Restore calls `new_session_in` for the main chat on every launch.

## Fix

- A place starts in `launching`. `remember_session` is a no-op until launch merge has applied the stored last-chat.
- Launch merge still mints a main chat if the project has none, then **focuses** the remembered chat (file or kernel id), then clears `launching`.
- ⌘N, select, and a user new-session settle launch first, so they still write `last`.
- `session_connected` still remembers a mint that now has its kernel id — after launch, not before.

Kept the useful #675 bits. Did not revert remember-on-⌘N / `session_connected`. Did not touch iOS, Jev A–G, or v0.2.0.

## Check

`cd desktop && cargo check` passed locally after installing the CI system libs. `mt-24` is the live proof; this agent did not re-run the driver scenario.
