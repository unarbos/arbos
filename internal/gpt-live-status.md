---
cursor:
  subagentId: "bc-4e8d547d-fa8a-535d-a931-0d11d7b4fdbb"
---

# GPT-Live session context: status

Updated 2026-09-17. Owner: this pass (expand what Live can see). Branched from `origin/main` at `aaf3dbe9` (after #495 / #492 / #490). PR: [#500](https://github.com/unarbos/arbos/pull/500) on `cursor/live-session-context-6983`. Do not reopen those merged branches. Do not publish v0.2.0.

## Ask

Jacob's GPT Live voice (`--engine openai`) needed the project he is in, the chat he is looking at, and what his sub-agents are doing. After #492 it only had the last 12 user/Arbos kernel lines plus worker start/finish events. That is not enough.

## Change (gateway + the desktop snapshot)

- Instructions brief now has three stores: PROJECT IDENTITY, ON-SCREEN CHAT, WORKERS AND ACTIVITY.
- On-screen chat is the client's `session.start.project.context` (user, Arbos, workers, tool labels, notices, asks, live thinking). The kernel transcript fills older lines. Cap 40 lines.
- Tool lines are names and a short detail only. Diffs, file bodies and repos are dropped.
- Live updates include worker reports, the main agent's tools and turns, and a status snapshot at start.
- Desktop `call_context` sends 40 visible rows (was 12) and includes notice / asked / in-progress thinking. Still no tool output or diff.
- Voice rows stay display-only. Typed composer lines still go to the kernel. That split is unchanged.

## Check

Verified on this VM against the PR head:

- `python -m tests.run live-session-context` — PASS, 21 checks. Session instructions and `session.input` see PROJECT IDENTITY, ON-SCREEN CHAT, WORKERS AND ACTIVITY, the on-screen rows, `rewrite-readme` / writing the README, the older kernel line, and `cargo test --offline`. They do not see `SECRET_DIFF_BODY`. `user_frames = 0` (the seed does not wake the kernel).
- `typed-during-call` — PASS. A typed line is still a `text` steer; a spoken line is still `voice`.
- Also PASS: `path-binds-own-kernel`, `hub-attach-reports-own-folder`, `work-sound-activity`, `narrator-only-voice`, `bare-project-name-refused`.
- Desktop `cargo check --offline` could not run here (no crate cache). The snapshot change is more match arms on `ChatItem`.

What-Live-can-see: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/gpt-live-context.md`

## Not done here

- Deploy to the production gateway (needs a merge, then the voice unit).
- `hub.toml` on Jacob's Mac (still required for a Mac call).
- Publishing v0.2.0.
