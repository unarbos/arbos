---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# V-01 step 3: speech-server text channel and agent mirror in the desktop

Branch `cursor/voice-mirror-b027` (on `cursor/release-integration-52cd`).

- `/voice <words>` in the composer sends `text.input` to the speech server (connecting if needed). Its answer streams into the status row (`text.delta`) and lands in the chat as a notice `voice answered: …` (`text.done`). It needs a server-side answerer: `voice_reply = "kernel"` (the server's own kernel agent) or `"openrouter"` in config.toml; default `none`, and then `/voice` says so in a failed notice instead of going nowhere.
- Agent mirror (`voice_mirror`, default on): `tool.call`/`tool.result`, `agent.turn`, `agent.event` (`user`, `tool`, `say`, `notice`, `assistant_final`…), `agent.done` arrive as notices `voice · <agent> …` in the active chat, drained every 400 ms while the session is live. `agent.tree` and per-token `assistant` deltas are not shown. Notices are window-local: not on the kernel transcript, gone on reopen.
- `server_answers()` (from #67) now also counts a non-`none` `session.ready.reply`: with a reply backend the server answers dictation itself, so the desktop neither sends the words to its kernel nor speaks the answer.

Verified against the live duplex server with `voice_reply = "kernel"`: `/voice Which agents are running…` → `voice runs agent_status {}` → `voice got agent_status → …` → `voice answered: Right now, the main agent is idle…` in 3.3 s; `/voice Send the agent this task: … haiku` → `send_agent` → `root is running` → `root tool: spawn` → `writeatwo-linehaikuabout is running/idle` → `root assistant_final: …` → `writeatwo-linehaikuabout finished: Gentle rain whispers, Earth drinks deep in quiet grace.` in 7.7 s. Capture `media/features/voice-mirror-after.png`.

Attack surface: `/voice` with `voice_reply` unset → the failed notice names the key; a server without a kernel (`kernel: false`) and `voice_reply = "kernel"` — what does `session.ready` say, and does `/voice` get an error frame?; a very long tool output (mirror clips at 200 chars, tool args at 120); 200+ mirror lines while no window drains them (cap 200, oldest dropped); switching the active chat while events arrive (they land in whichever chat is active at drain time — say if they should stick to the chat that sent `/voice`); a `/voice` while the mic is open; the notice rows for very long lines are single-line and clip at the window edge (existing notice style).
