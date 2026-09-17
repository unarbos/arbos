---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

**For the desktop call worker and the iPhone loop.** The production voice gateway switched to OpenAI GPT-Live at 18:27 UTC (details: `internal/voice/gpt-live-backend-2026-09-17.md`). Nothing changes on your side of the wire; two things are new and one is different.

## New: `agent.activity` (the working sound)

```json
{"type":"agent.activity","agent":"root","state":"working"|"tool"|"idle","tool":"bash"|null,"detail":"uptime"|null}
```

Sent on every change of what the call's agent is doing, whichever engine runs: `working` when a kernel turn is running (thinking or generating) — emitted at the moment the gateway hands the question to the kernel, before the kernel's own turn frame; `tool` while inside a tool call, with the tool name and one line of what (command, path, brief); `idle` when the turn ends. Play the sound while `state != idle`. It is derived from the kernel's frames, not a timer, and it is the only signal; there is no second one.

## New: `session.ready.engine == "openai"`

`asr`/`tts` read `openai/gpt-live-1`. Everything else in `session.ready` is as before (`project`, `project_info`, `via`, `answerer` — the last is always `model` on this engine, because GPT-Live decides when to delegate).

## Different: who speaks, and when

- Small talk is answered by GPT-Live itself, ~1–1.5 s after you stop talking (slower than the hosted model's ~0.5 s).
- Anything about the project is delegated: GPT-Live says "one sec, let me check" (only when a delegation is really in flight; the gateway backstops the case where it says so without one), `tool.call {name: "delegate"}` and `agent.activity working` go out, the kernel works, `tool.result` carries the kernel's text, and GPT-Live speaks the answer in its own voice when it arrives — even if the caller has said something else since. Barge-ins do not cancel the kernel turn; `response.done {reason: "interrupted"}` still comes for the audio you were playing.
- `transcript.final` is still our Whisper transcript (first word intact). GPT-Live's deltas are not forwarded as `transcript.delta` yet; the live user transcript therefore arrives as the final only.
- Kernel asks and approvals are spoken in GPT-Live's voice (`narrator.say {kind: "ask"}` still precedes them); the caller's yes/no is taken by the gateway as before. Not exercised live yet: the pod's kernel did not ask for approval on `bash uptime`.
- Barge-in measured 147–228 ms to `response.done interrupted`, 0 late frames.

To go back to the hosted model: `VOICE_ARGS="--engine duplex …"` in `/root/arbos-voice/env` on the pod and restart the `voice` session.
