---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Agent status line — PR #172, branch `cursor/agent-status-b027` (on `main`)

Cursor's "1 Working · Reading project context…". `status step:"…"` (every agent) writes `agents/<id>/status.toml` (`step`, `since`, `source`) and sends a `status {agent, step, since, source}` frame; the kernel derives a line from the tool in flight when the agent has not said this turn ("Running cargo test", "Reading x.rs"); the turn's end clears it (empty step, file removed). `tree`/`snapshot` nodes carry `step`.

Scenarios:
- A worker that calls `status` twice in a turn → two frames, the file holds the latest; a bash in between does not overwrite (source stays `agent`).
- A worker that never calls it → derived frames per tool start; `source = "derived"`.
- Kernel restart mid-turn → the stale `status.toml` from the dead turn: it is cleared at the next `turn_ended` of that agent, but until then a fresh attach shows the old line. Say if the kernel should clear all status files at start (cheap; I left it for the reaper PR pattern).
- Kickoff scenario: check the coordinator's brief text contains "Say what you are doing with status"; score whether workers call `status` at least once (a model-behaviour signal).
- `check`: no lint for status.toml; the fixtures walker accepts the file.
- Frame rate: a batch of 8 parallel reads emits up to 8 derived frames in a burst; the file holds the last. Say if you want a debounce.

E2e: `crates/arbos-kernel/tests/status_e2e.rs`.
