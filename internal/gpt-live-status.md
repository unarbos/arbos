---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# GPT-Live voice backend: status

Updated 2026-09-17 18:45 UTC (moved here from the private store at 18:44; that path was wrong). Owner: voice-server agent. Code: `cursor/voice-server` (PR #421 → main). Full evaluation: Project store `internal/voice/gpt-live-backend-2026-09-17.md`; note to the desktop and iPhone loops: `internal/features-inbox/2026-09-17-voice-gpt-live-and-activity-frame.md`.

## State: DONE and live on the production gateway (since 18:27 UTC; redeployed 18:41)

- Engine `--engine openai` (flag; `--engine duplex` puts the hosted model back, one env line on the pod). App interface unchanged; `session.ready.engine` = `openai`.
- Key: vault item `6k3kuhvc3xxwpgao4mblskj45e` ("New New OpenAI"), found by inventory, verified against `/v1/models`, on the pod as `OPENAI_API_KEY` (mode 600). Never printed.
- Transport: `wss://api.openai.com/v1/live/sessions`, `session.start` with `delegation: {type: client}`, PCM16 24 kHz; the kernel is the backend (`session.delegation.created` → kernel turn → `session.thinking.append` while working → `session.commentary.append` with the answer).

## Acceptance conversation (production gateway, public tunnel, 18:42 UTC)

| step | result |
| --- | --- |
| "hey" | answered locally by GPT-Live ("Hey."), no kernel; first audio 1.3–1.8 s after speech end |
| "what's the status on the project" | delegated 0.7–0.8 s after question end; kernel asked with our Whisper transcript (first word intact) |
| "one sec let me check" | spoken only with a delegation in flight; gateway backstop: a filler or any non-small-talk utterance without a delegation within 2.5 s is delegated by the gateway itself |
| talk through the pause | "Actually, also tell me the time" cut the filler, did not cancel the kernel turn; was itself delegated (real system time came back) |
| working sound | `agent.activity {working → tool(bash, uptime) → idle}` frames on every change; desktop plays on them; no timer |
| answer unprompted | "This project's fully done. The poems, sorting algorithms, and nine J-series fixes are all done, tested, and committed." spoken 9.2 s after the question, after the barge-in |

## Measurements vs the hosted model (same client, same path)

- Barge-in → `response.done interrupted`: GPT-Live **147–228 ms**, 0 late frames; hosted 306/333 ms.
- First word of the caller: kept on both (our VAD + Whisper; GPT-Live's own transcript also kept "Hello").
- Small-talk first reply audio from speech end: GPT-Live ~1.0–1.8 s; hosted ~1.0 s (its own reply ~0.4 s after our transcript).
- Delegated answer: dominated by the kernel (5–9 s on the pod's phone kernel, 153k-token context).

## Cost (OpenAI model page; OpenRouter list prices)

- Voice $0.05/min of open session, per second, no rounding (`session.closed` reported 31 s = $0.026). Idle listening counts.
- Backend separate: gemini-2.5-flash via OpenRouter ≈ $0.005 per delegated question at 10k context, ≈ $0.05 at the phone kernel's current 153k.
- Jacob's exchange ≈ $0.03 voice + $0.005–0.05 backend. Pod $860/month = 287 hours of open call/month (9.5 h/day) → GPT-Live cheaper at any realistic usage.

## Gain / lose

Gain: faster, cleaner barge-in; no invented tool results (the hosted model's filler-without-tool fault); the kernel's answer in the same voice, unprompted, surviving interruptions; the pod's 87 GB model no longer needed (Whisper 2.6 GB can move to ArbosLife CPU or be replaced by GPT-Live's transcript).
Lose: running without a provider; audio leaves our machines (OpenAI API terms); control of turn-taking and phrasing (it paraphrases, ~1 s slower on small talk; invented a clock time once before the gateway guarantee routed such questions to the kernel).

## Open items / blockers

- None blocking. Approvals path (kernel asks spoken in GPT-Live's voice, caller's yes/no taken by our narrator) exists but was not exercised live: the pod's phone kernel runs `bash` without asking. Harness approval scenarios pass on the hosted path.
- GPT-Live's `session.input_transcript.delta` is not forwarded as `transcript.delta` (the live caption); `transcript.final` comes from Whisper. Small follow-up if the apps want live captions on this engine.
- Pod: keep one week as insurance; the gateway can move to ArbosLife with the hub/phone-kernel cutover.
