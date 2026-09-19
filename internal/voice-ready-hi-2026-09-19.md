---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# The call says "hi" when it is ready — evidence (2026-09-19)

Jacob's word: GPT Live should announce it is ready to talk by saying **hi**. PR: [#772](https://github.com/unarbos/arbos/pull/772) (`cursor/live-ready-hi-f23a`, off `main` `3bccf924`). Live SHA on the production gateway: **`354413e0`** (the #772 merge; deployed 15:22Z, see the end of this page).

## What "ready" means here

The gateway opens the GPT Live session on the first microphone frame. When OpenAI answers `session.started`, the caller's audio is flowing and the model can hear him. That moment, and nothing else, is the ready signal. The gateway then:

1. sends `narrator.say {kind: "ready", text: "Hi."}` to the client (the row for the chat);
2. sends GPT Live an instruction (`session.instructions.append`): speak right now, exactly one word, "Hi.", nothing more, then stay silent until the caller speaks;
3. sends a one-word commentary (`session.commentary.append`, "Hi.") so the model speaks first.

This is OpenAI's own recipe for a greeting. It is not a timer. It is not the working line ("Yeah, one sec.", #769), which still waits for the kernel to actually be running. Once per call. Code: `voice_server/openai_live.py`, `_say_ready`, `READY_LINE`, `READY_QUIET_S`.

## What the real model did

Side instance of the branch on the pod (port 8799; the production gateway on 8765 was not touched), real GPT Live, project `arboslife/demo`. The reply audio was captured and transcribed with the gateway's own Whisper (`large-v3-turbo`).

| Attempt | Wording | What GPT Live said (Whisper on the captured audio) | First audio after `session.started` |
|---|---|---|---|
| 1 | "Your first spoken words, right now and exactly, are: 'Hi.'" | "Hi. I'm here." | 1,455 ms |
| 2 (3 runs) | "exactly one word and nothing more" | run a: read the quiet context snapshot out ("Project update: no agents are running…"); run b: "Hi."; run c: "Hi. Quick update: all workers are finished…" | 1,153–2,224 ms |
| 3 (7 runs), final | as 2, plus: quiet-context appends held 6 s after the greeting; instructions say `Project update:` lines are never read aloud unprompted; the greeting's early transcript words kept | **"Hi."** in 7 of 7; one run added "Mm." after it | 1,171–1,444 ms |

The fault in attempt 2 was ours, not the model's: the first `Project update:` thinking append (`_context_snapshot`) landed while the model was producing its opening word, and it voiced the two together. Holding quiet context until the greeting is out of the way fixed it.

One more thing the runs showed: GPT Live does not reliably emit an output transcript for a one-word commentary (`response.transcript` was empty in most runs though the audio said "Hi."). The `narrator.say kind=ready` frame is therefore the text a client should show; the `response.*` frames carry the audio.

## Barge-in

A barge-in over the greeting works as over any reply: the client flushes, sends `interrupt`, the model yields. Found while testing: the gateway's mute after a barge-in lifted only on a quiet frame, so with a greeting to barge into, the model's reply to the caller's small talk could be swallowed whole. The mute now ends when our transcript releases small talk. Nothing in the kernel is cancelled by a barge-in (unchanged; `on_interrupt` touches no delegation).

## Harness

`voice-server/tests/scenarios/live-ready-hi.toml`: the mock model speaks the commentary; expects `narrator.say` "Hi.", the model spoke "Hi.", one `response.done`, kernel not asked. The three existing Live scenarios now count the greeting. 33 of 33 pass.

## Not done, on instruction

Jev is not on the gateway. GPT Live stays `--engine openai`. Orb colour is not guessed or touched. Jacob was not contacted. No `ios/` change, so no TestFlight.

## Deployment

#772 merged as `354413e0` (2026-09-19 15:21Z). `voice-server/` from that commit was deployed to the production gateway on the Lium pod the same way as #758 and #769 (tar over ssh into `/root/arbos-voice/src`, the `voice` tmux session restarted by the supervisor). `/healthz` → **200** at 15:22:24Z; the log shows `engine=openai`. Recorded on the pod at `/root/arbos-voice/src/DEPLOYED_SHA`.

**Live SHA: `354413e0`.**

First session on production after the deploy (15:23:05Z, `arboslife/demo`): `narrator.say kind=ready "Hi."` at 1.57 s after the mic opened, the model's reply audio 1,614 ms after that, 1.2 s of audio, one `response.done`, no errors; the gateway log reads `ready: said 'Hi.'`.
