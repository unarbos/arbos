---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Rewind pressed in a forked chat: the kernel log says it rewound `root`

**From:** Jacob's desktop feedback `2026-09-17-18` — *Rewing to here does not work on a forked chat.* Build 0.2.0 (1335), kernel `f9b6089c0b80`, place Arena-GPS. Report: `media/desktop-feedback/2026-09-17-18/`. For the rewind owner (#405's author); the desktop half is in the cycle-34 PR.

## The line that worries me (quoted from the report's kernel log)

```
{"agent":"root","event":"rewind","level":"info",
 "detail":"target=Line(75) line=75 dropped=21 files=true
           archive=/Users/const/Arena-GPS/.arbos/agents/root/transcript.rewound-1789640368932.jsonl"}
```

Jacob was in the **forked** chat (`agent: chat-1789640318831` in the report's header) when he pressed *Rewind here*. The kernel's rewind event names `root` and archived `agents/root/transcript…` — 21 lines dropped from root's transcript, `files=true`. If the desktop's frame carried the copy's agent id (it sends the rewind on the copy's own socket; see below), the kernel rewound the wrong agent. If the fork's socket is in fact root's, that is the same bug one layer down.

## What I can say from the desktop's side

- `ChatSession::rewind` (desktop/src/model/session.rs) sends `Frame::Rewind` on `self.connection` — the chat's own attach — with the line count of *that* chat's items. A fork is its own `ChatSession` with `agent_session` = the clone's kernel id, attached to the kernel as that agent.
- On the rig with main's kernel `7e19f9e90947`, a rewind clicked in a fork copy produced **no kernel `rewind` event at all** and no state change; the copy was not yet attached (`connection: idle` — the fork opens from the transcript file and attaches on its first send), so the desktop's guard refused before any frame went. That half is mine and is being fixed (attach, then rewind; say so meanwhile). It does not explain Jacob's line, where a rewind *did* run — on root.

## Ask

Read `rewind` in the kernel against a forked agent: does a fork's checkpoint set (copied on fork, #405 / F-103) point at root's transcript path, or does the rewind handler resolve the agent from the place rather than from the attached stream? The archive file name in the log is the place to start. If root really lost 21 lines to a click in a copy, Jacob's root transcript on Arena-GPS is short by that much and the `.rewound-1789640368932.jsonl` archive is where they are.
