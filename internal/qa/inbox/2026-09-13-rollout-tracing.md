---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# P-15 Rollout tracing: replay provider + rollout bundles

Branch `cursor/rollout-tracing-b027` (based on `cursor/release-integration-52cd`). Feature agent note for QA before the PR.

## What it does

1. **Replay provider.** `ARBOS_PROVIDER=replay ARBOS_REPLIES=replies.jsonl arbos-kernel run <place> "<prompt>"` (or `arbos-kernel serve <place> --provider replay --replies <file>`) runs turns with **no network**: every model call returns the next scripted reply from the file. A reply line is `{"content": "...", "calls": [{"name": "bash", "arguments": {"command": "echo hi"}}], "agent": "root"?}`. `agent` (optional) pins a reply to one agent so sub-agent calls do not steal it; replies without it go to whoever calls next. When the file runs out, the call returns `(replay: no more scripted replies)` with no tool calls, so the turn ends.
2. **Rollout export.** `arbos-kernel rollout export <place> [--agent ID] [--out DIR]` writes `<out>/<timestamp>-<agent>/` with `meta.json` (place, agent, model, kernel version/git, times, event/tool counts), `agent.md`, `transcript.jsonl`, `plan.jsonl`, `feedback.jsonl` when present, the `trace/` folder when `trace = true` was on, and `replies.jsonl` derived from the trace (exact steps) or, without a trace, from the transcript (one tool call per step).
3. **Rollout replay.** `arbos-kernel rollout replay <bundle> [--out DIR]` builds a fresh place under `--out` (default `/tmp/arbos-replay-<ts>`), copies `agent.md`, feeds the bundle's first user prompt through the replay provider, then prints `replay: N/M tool calls matched (name+args in order)` and exits 0 when all matched, 2 otherwise.

## Attack surface

- Replies with tool names the agent's allowlist forbids: the tool refuses; the next reply is still consumed in order (the model "said" it). Expect a `tool` event with an error, not a hang.
- Replies pinned to an agent that never calls; the file has more lines than the turn uses (leftover count in the replay summary).
- Empty `content` and empty `calls` in one line — ends the turn like a silent model.
- Bundles from a place with sub-agents: `--agent` picks one; `spawn` replies during replay create real children whose calls consume unpinned replies.
- `rollout export` on an agent whose transcript has a 1 MB spilled body (K-14): the bundle copies the transcript, not `results/` — expected; note if that hurts.
- `rollout replay` of a bundle whose transcript starts with a `wake`/inbox line rather than a `user` line: it should refuse with a clear error.
- Replay + compaction: `compact` calls also hit the provider; a bundle exported from a long turn includes the compaction replies from the trace, but a transcript-derived one does not (documented).
- `trace = true` in `~/.config/arbos/config.toml` and the trace files: is the `transcript_line` still right on the integration branch (K-13 deltas do not add lines)?
