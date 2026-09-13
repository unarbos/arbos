# qa-005: an `answer` or `approve` frame for an unknown agent creates a ghost agent folder

status: confirmed
severity: low (junk folders; but `list_agents` skips them silently, so nobody sees the junk)
scenario: malformed-frames
rollout: /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa/rollouts/20260912T223723Z-malformed-frames
fingerprints: e6102ae107 a2776c674a

## Repro

Attach and send `{"type":"answer","agent":"nobody","text":"42"}`.

## Expected

The frame is rejected (no such agent) and nothing is written.

## Actual

`.arbos/agents/nobody/transcript.jsonl` appears with one `answer` line and no `agent.md` (`state-after/agents/nobody/`). The consistency checker flags it as `agent-md-missing`.

## Suspected location

- `crates/arbos-kernel/src/serve.rs:365-373` (`Answer`) and `384-390` (`Approve`) call `append_event` on `Layout::new(place, &agent).transcript()` without checking the agent exists.
- `crates/arbos-core/src/files.rs:156-158` `append_events` does `create_dir_all(parent)`, so any path becomes a folder.

## Fix idea

Check `place.agent_dir(&agent).join("agent.md").exists()` in `handle_frame` before any write, and return an error frame (see design gap 1).

## Also seen

- ArbosLife cycle 2026-09-12T23:45Z, scenario `rapid-create-delete` with a model key: a chat deleted while its turn ran came back as a folder with a transcript and no `agent.md` (fingerprint a2776c674a). Same cause: `append_events` recreates the folder for any writer, here the turn itself.
