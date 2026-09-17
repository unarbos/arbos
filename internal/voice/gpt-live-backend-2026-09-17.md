---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# GPT-Live as the voice backend: what runs, what it costs, what it means for the pod

Decision executed 2026-09-17 18:27 UTC: the production gateway on the pod (`wss://…trycloudflare.com/ws`, the same URL the phone and desktop use) now runs `--engine openai` (OpenAI GPT-Live 1, client delegation, the Arbos kernel as backend). The hosted NemotronLabs model still runs on the pod and is one env edit away (`VOICE_ARGS="--engine duplex …"` in `/root/arbos-voice/env`, restart the `voice` tmux session). Code: `cursor/voice-server` commit `eef9455`, in PR #421. The app's side of the wire protocol is unchanged; `session.ready.engine` says `openai`.

## The key

Found by inventory, not by name: vault item `6k3kuhvc3xxwpgao4mblskj45e` ("New New OpenAI", a secure note, `notesPlain` = an `sk-proj-…` key, created 2026-08-06, present in the Arbos vault today though the 13 September inventory did not list it). Verified against `GET /v1/models` (200; `gpt-live-1` and the `gpt-realtime-*` family are on it) and by the sessions below. It lives on the pod as `OPENAI_API_KEY` in `/root/arbos-voice/env` (mode 600). Never printed.

## Jacob's exchange, literally, on the production gateway through the public tunnel

| line | what happened | measured |
| --- | --- | --- |
| "Hey." → "hey" | GPT-Live answered itself: "Hey! What's up?" No kernel round trip. | first reply audio 1.3–1.6 s after speech end (its own endpointing; the hosted model did 0.4–0.5 s) |
| "What's the status on the project?" | `session.delegation.created` from GPT-Live; the gateway asked the kernel with our Whisper transcript (first word intact) | delegation 0.69–0.77 s after question end |
| "one sec let me check" | spoken by GPT-Live *because* it delegated; if it ever says such a thing without a delegation, the gateway waits 2.5 s and delegates the utterance itself (`_watch_orphan_ack`) | filler audio 0.97–1.10 s after question end |
| the pause | `agent.activity {state: working}` at delegation, `{state: tool, tool: bash, detail: uptime}` while the kernel ran a command, `{state: idle}` when done; the desktop plays its sound on these. Talking over the pause ("Actually, also tell me the time") stopped the filler, was answered, and **did not cancel the kernel turn** (activity stayed `working`, the result arrived) | barge-in to `response.done interrupted`: 147–228 ms, 0 late frames |
| "the status is …" | the kernel's answer appended as `session.commentary.append` and spoken unprompted, after the interruption and the side question: "Everything's done and nothing's running." | 8.7–12.3 s after the question, almost all of it the kernel (the pod's phone kernel carries a 153k-token context and answers in 5–8 s) |

Approvals: the kernel's `ask` frames are spoken in GPT-Live's voice and the caller's yes/no is taken by our narrator (only-asks mode), never by the provider. Not exercised live today: the pod's phone kernel ran `bash uptime` without asking (its permission mode allows it). The 16 harness scenarios, five of them approval scenarios, pass on this tree with the hosted-model path.

## Does it meet the bar we set this morning?

| | hosted duplex (control, same client, same path) | GPT-Live |
| --- | --- | --- |
| caller's first word | kept (our VAD + Whisper) | kept (same Whisper path; GPT-Live's own transcript also had "Hello") |
| barge-in → playback cut | 306 / 333 ms | **147 / 228 ms**, 0 late frames |
| small talk first audio (from our transcript) | 418–422 ms | 230–430 ms; from speech end ~1.0–1.2 s vs ~1.0 s |
| project question reaches the kernel | yes (gateway routing) | yes (model delegates; gateway backstops a filler without delegation) |
| answer spoken | Kokoro voice, after the kernel | GPT-Live's voice, unprompted, survives barge-in |
| model invents things | filler for tool calls it never made (the morning fault) | invented a clock time when asked the time mid-delegation; prompt says never invent status; small talk is its own |

## Cost, from OpenAI's model page and OpenRouter list prices

- Voice: **$0.05 per minute of session, billed per second, no rounding** (`session.closed` returned `{"seconds": 31.0}` for a 31 s call = $0.026). Idle listening time counts: a 20-minute run with the call open costs $1.00 whether he talks or not.
- Backend: the kernel's own model calls, unchanged by this move. gemini-2.5-flash via OpenRouter is $0.30 / M input and $2.50 / M output; one delegated question costs ~$0.005 with a compact 10k-token context and ~$0.05 with the phone kernel's current 153k-token context. Whisper for transcripts runs on our own hardware.
- Jacob's exchange (hey; status question; one delegation; ~35 s of session): about **$0.03 voice + $0.005–0.05 backend**.
- Crossover with the pod: $860 / month buys **17,200 minutes = 287 hours** of open session per month, about **9.5 hours every day**. Below that, GPT-Live is cheaper; a heavy day of two hours on the call is ~$6.

## What it means for the pod

The pod exists for the 87 GB NemotronLabs model. With GPT-Live as the voice, the gateway's remaining GPU use is faster-whisper large-v3-turbo for the transcript (2.6 GB; 45–90 ms per utterance on the L40-class GPU; on ArbosLife's 128 CPU cores small.en int8 takes ~1–2 s, or GPT-Live's own input transcript can be used, which kept "Hello" in today's runs). Kokoro is only a fallback voice now. So the pod can go when the phone kernel and hub cutover to ArbosLife lands: the gateway moves to ArbosLife next to them, Whisper on CPU or GPT-Live transcripts, and the NemotronLabs container is simply not started. Keep the pod one more week as insurance, as asked; `--engine duplex` puts the hosted model back.

## What we give up, plainly

- Running without a provider. Every call now needs OpenAI reachable and a paid key; the open-source path stays behind a flag and stops being exercised.
- The audio leaves our machines. OpenAI's API data-use terms apply (no training on API data by default; retention per their policy); with the hosted model nothing left the pod.
- Control of turn-taking and the reply voice: GPT-Live decides when to speak, when to yield and how to phrase the kernel's answer. It paraphrases (500-token appends), so long kernel replies are compressed; it can also volunteer things (a made-up clock time). Our own barge-in, first-word and merged-question work are now behaviours we can only wrap, not tune.
- Latency profile: small talk about a second from speech end (the hosted model was ~0.5 s faster on its first word); barge-in and delegation are faster than ours.

The stated preference for self-hosting was a means to a full-duplex conversation with his agents. On today's numbers GPT-Live delivers that conversation better than the hosted model on interruption, on not inventing tool results, and on cost at any realistic usage, at the price of the provider dependency above.
