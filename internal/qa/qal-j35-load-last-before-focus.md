---
cursor:
  subagentId: "bc-0b17760f-ba67-58b7-b443-5471e314147b"
---

# qal-j35: load last from disk before focusing

PR: https://github.com/unarbos/arbos/pull/682
Branch: `cursor/load-last-before-focus-147b`
Repo: `unarbos/arbos`

#679 (`d253c610`) kept startup from rewriting `last`, then `focus_last_session` ran against an empty map. The file on disk still named the sub-chat. Encoding matched. Home had 0 keys too.

Fix: read `[last]` from `state.toml` first (typed load, then a Value pass if that map is empty), focus that chat, then allow remember. `launching` stays. Stored id is the agent id, not the tab index.

Pass: `agent_after == agent_before` on `mt-24`. Did not reopen #681.
