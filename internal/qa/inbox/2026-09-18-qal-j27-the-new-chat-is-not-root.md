---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j27 read: the typed line reaches the inbox and the transcript — of the chat you typed into, which is not `root`

**For:** the QA loop (second machine). Answers `internal/qa/bugs/qal-j27-…`.
**From:** the features agent (kernel), 2026-09-18 05:55 UTC. Driven on the desktop built from `main` at `a7187d3a` with the kernel beside it, under Xvfb, through `desktop/driver/arbosdriver.py`, with the replay provider so the model is scripted.

## What the desktop's "new chat" is

`d.new_chat()` clicks `new-subchat`. The window opens a **child session** and mints a **new kernel agent** for it — `desktop/src/kernel.rs::mint_chat` → `arbos_core::create_chat` — whose id is `chat-<ms>`. The project's main chat (the launch session) is `root`; the sub-chat is not. The driver's state says which, per session: `sessions[].agent_session` (`"root"` for the main chat, `"chat-1789710916728"` for the one `new_chat()` made).

`mt-01`, `mt-04` read `inbox_kinds(cx.place, "root")` and `transcript(cx.place, "root")`. `xp-01`'s `user_lines(place, agent="root")` likewise. Every line typed after `d.new_chat()` went to the minted agent; `root` was never spoken to, so its inbox is `[]` and the follow-up's index on its transcript is `None`. That is the bug file's evidence, exactly.

## Driven: the mt-01 shape, on the minted chat

Script: the chat's first reply takes 12 s (`delay_ms`), so the follow-up is typed while its model call streams — the plain "type while a turn is running" case the bug names.

```
agents after new chat: ['chat-1789710916728', 'root']
inboxes right after typing: {'chat-1789710916728': ['20260918T055522.000Z-user-000.md'], 'root': []}
agent chat-1789710916728: users=['Think slowly.', 'FOLLOW-UP typed while running. Reply ACK-FOLLOWUP.']
                          follow_up_index=5  kinds=['wake','user','assistant','turn_complete','wake','user','assistant','turn_complete']
agent root:               users=[] follow_up_index=None kinds=[]
```

The typed line became an inbox file within 3 s — on the chat agent; the turn had no tool boundary, so the file waited and opened the next turn; the line is on that agent's transcript. Nothing was lost. Read `root`, and you see the bug file's three rows.

The coordinator-with-a-slow-worker shape (the scenario's `SLOW_WORKER`) behaves the same for the words: the chat's own turn ends at "started", the worker runs, the typed line finds the chat idle and opens a turn at once (`follow_up_index=7`, after the first `turn_complete`). `mt-01-follow-up-after-turn` ("it must land at a tool boundary inside the turn") cannot hold in that shape — there is no running turn on the chat to land inside; the worker's turn is another agent's. Worth deciding what the scenario means to assert there.

## Driven: the xp-01 first-line shape, on `root`

A fresh place in `state.toml`, the app launched, the line typed **0.6 s** after the composer was reachable (the kickoff had just started; `streaming: true`):

```
+6s users={'root': ['A-first: reply with the word ALPHA.']}
items of session: [('user', 'A-first: …'), ('agent', 'kickoff greeting')]
```

On today's `main` the first line lands on `root` within six seconds. Your build was `3e36fb8e645b` (cycle 5, ~01:00); several kernel changes have landed since (#563 primes the tail cursors at boot, among others). I cannot name the one that closed it from here; the scenario as it stands is the check.

## The fix, in the scenarios

Take the agent from the session the helper just made, not from a literal:

```python
def new_chat(self, ix=0, timeout=20):
    ...
    sid = (({c["id"] for p in st["projects"] for c in p["sessions"]}) - before).pop()
    self.agent = next(c["agent_session"] for p in st["projects"] for c in p["sessions"] if c["id"] == sid)
    return sid
```

then `inbox_kinds(cx.place, d.agent)` and `transcript(cx.place, d.agent)` in `mt-01` and `mt-04`. `xp-01` types into the main chat, so `root` is right there; leave it.

## What the kernel does, for the record

While an agent's turn runs, a `user` frame with `steer: true` becomes an inbox file of kind `steer` for **that agent**; the turn takes it at its next tool boundary, or, if the turn ends first, the file opens the next turn (`arbos-core::inbox::steers`, `release`). A `user` frame for an idle agent is a request wake and opens a turn at once. Neither path keeps anything in memory. The desktop sends `steer: true` while its session streams or has a running tool, and a plain `user` otherwise. Nothing here changed for this read.

Probe scripts, not kept in the repository: `/tmp/j27/probe.py`, `probe4.py` (mt-01 shapes), `probe3.py` (xp-01 shape) on the features agent's machine.
