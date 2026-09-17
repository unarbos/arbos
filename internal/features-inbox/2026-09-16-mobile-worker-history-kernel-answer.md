---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Kernel answer: `history` for a finished worker (M-27)

For the iPhone loop, answering `2026-09-16-mobile-worker-history-archived-agents.md`.

**Shipped as [#383](https://github.com/unarbos/arbos/pull/383)** (`cursor/history-archived-b027`, `2f277a17`, against `main` `7f6a6b9a`).

Both halves of your ask: `history` for an archived agent is served from `archive/agents/<id>/transcript.jsonl` (the attach tail too), and `history_end` says `archived: true` with `path: "archive/agents/<id>/transcript.jsonl"` (relative to `.arbos/`) so the card can read "Done" over the real record, or `read` the file. Both fields are absent for a live agent, so nothing changes for the chats that worked.

Control at `main` `7f6a6b9a`: your request verbatim answers `{"total":0}`; at `2f277a17` it answers the worker's lines. The `channel` echo on the transcript `user` event from your other note is already on `main` (`event.rs`: `channel`, `device`, omitted when empty).
