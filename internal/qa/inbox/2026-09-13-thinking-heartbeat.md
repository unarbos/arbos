---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Thinking heartbeat: "Thinking for Ns" during a silent model call

Branch `cursor/thinking-heartbeat-b027` (on integration `a8cabfc`). Jacob's Fable 5.1 calls ran 60–100 s with nothing on screen.

Finding first: OpenRouter **does** stream `reasoning` for `anthropic/claude-fable-5.1`, but only after the thinking is over — measured: first delta at 3.6 s (effort low) / 13.9 s (high), i.e. right before the content. Streamed reasoning cannot fill the gap; a heartbeat can.

- Provider: every 5 s (`HEARTBEAT`) with no bytes — waiting for headers or between chunks — `Delta::Waiting(elapsed since the call began)`; the idle limit (`stream_idle_ms`) is unchanged and still fails the call.
- Kernel: `Hooks::working(secs)` → `Frame::Working {agent, secs}`, live only, never on the transcript.
- Desktop: `chat.working` = (secs, heard-at); the heartbeat row under the turn becomes braille spinner + **"Thinking for Ns"** ticking every second (secs + time since the frame); any real delta, tool event, or `turn_complete` clears it. The finished-turn footer stays hidden until `turn_complete` (`a8cabfc`).
- Force is now labelled **"Interrupt now: stop the running turn and send this"** (composer disc tooltip) and **"Interrupt now"** on the queued row, so it is not taken for Send.

Verified: a stub model silent for 12 s → kernel frames `working 5`, `working 10`, then deltas; desktop row present from 5.3 s to 12.0 s, screenshot at 8.3 s reads "Thinking for 7s"; answer lands, row gone. Fable 5.1 (high) live: first delta at 5.7 s, i.e. faster than the first heartbeat in that run — the row appears only for calls quieter than 5 s.

Attack surface: a call that streams one delta then goes silent for 30 s (heartbeat resumes from the last progress? No — `Waiting` carries time since the call began; check the label is not misleading); heartbeat during compaction calls (`purpose: compact` — the same `working` frame; the desktop shows "Thinking" — fine?); two agents in one window; a phone client that does not know `working` (it is `unknown` → skipped, #75); `stream_idle_ms` shorter than 5 s; the heartbeat row after Stop (cleared by `turn_complete`); replay provider (no heartbeats).
