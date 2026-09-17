---
cursor:
  subagentId: "bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a"
---

# GPT Live in the iPhone app

**Short answer: yes, it already works. The phone dials the same gateway, and the gateway is GPT-Live now.** Three small phone-side gaps remained; they are on [PR #485](https://github.com/unarbos/arbos/pull/485).

## How the phone reaches GPT-Live (checked 2026-09-17 18:50Z)

Terms first. The *gateway* is our voice server (`voice-server/`). The *engine* is the speech model behind it. `--engine openai` means GPT-Live speaks; the Arbos kernel answers project questions through *client delegation* (GPT-Live hands the question to us, we ask the kernel, we hand the answer back for it to speak).

1. The phone reads its voice URL from `https://raw.githubusercontent.com/unarbos/arbos/qa-results/voice-endpoint.txt` (`ios/Arbos/Settings/EndpointDirectory.swift`). The gateway's supervisor keeps that file fresh. The token comes from the vault item the phone already holds. Nothing changed there.
2. I opened a session on that exact URL with that token. The gateway answered:
   `session.ready { engine: "openai", asr: "openai/gpt-live-1", tts: "openai/gpt-live-1", reply: "openrouter/google/gemini-2.5-flash", tools: [send_agent, agent_status, ask_arbos], kernel: true, answerer: "model", via: "gateway" }`
3. The phone's `answersItself` flag (`ios/Arbos/Voice/VoiceSession.swift`) was already true for this reply, because `reply` is not `none`. So the phone does **not** forward transcripts to the kernel on its own, and there is no double answer.
4. Barge-in, first word, token, and URL are all gateway-side properties. They were measured to survive the move to GPT-Live (see `internal/gpt-live-status.md`). The phone sends `interrupt` and `client.speaking` exactly as before.

So Jacob's acceptance exchange ("hey" → "hey"; "what's the status on the project" → "one sec let me check" → … → "the status is …") runs on the phone with no client change.

## What I built anyway (PR #485, branch `cursor/ios-gpt-live-f23a`, off `main`)

| Gap | What happened | Fix |
|---|---|---|
| Project line under the orb was blank | The gateway now sends `project: "machine/project"` (a string) with the roster's name and icon in `project_info`. The phone read only the old dict shape. | `SelfHostedVoiceSession` reads `project_info`, then a dict in `project`, then the string. |
| `openai` engine not named | `answersItself` was true only by way of the `reply` field. A gateway started with `--reply none` would have made the phone answer twice. | `answersItself` now also checks `engine == "openai"`. |
| No working sound | During the "one sec let me check" silence, the phone was silent. The desktop plays a sound from the gateway's `agent.activity` frames; the phone ignored the frame. | New `ios/Arbos/Audio/WorkSound.swift` and a handler in `CallViewModel`. |

The working sound, in one paragraph. The gateway sends `agent.activity { agent, state, tool?, detail? }` on every change. `state` is `working` (a turn is running), `tool` (inside a tool call), or `idle`. While any agent is not idle the phone plays a soft tick: 880 Hz, 45 ms, damped, every 2.5 s — the same tick the desktop's `ticks` mode uses (`desktop/src/voice_ws.rs`), so both surfaces sound like one product. A command starting on the main agent adds one tick at once. The tool's name and detail go on the note under the label. The sound waits while a reply plays or while the caller talks. It stops after 12 s without a frame, so a dropped link cannot tick forever. It plays through its own `AVAudioPlayer`, not the reply path, so it is never reported to the gateway as `client.speaking` and a barge-in never cuts it.

## What I could not do here

This worker has no Xcode, so the PR's first compile is CI's "iPhone (build, simulator)" job on macOS. The change is small and additive: one new file, one new `VoiceEvent` case handled in the single exhaustive switch (`CallViewModel`); every other switch over `VoiceEvent` has a `default`. The mobile loop's rig is the right place to hear the tick on a real phone.

## For the mobile and desktop loops

- The tick's shape is copied from the desktop's `ticks` mode. If the desktop changes its sound, change `WorkSound` too, or the two surfaces drift.
- `agent.activity` is the only source of the sound on both surfaces. Do not add a timer.
