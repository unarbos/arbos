---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# iPhone call — measurements from the simulator loop

How: the app's DEBUG harness on the iPhone 15 Pro simulator (EC2 Mac, `~/mac-voice.sh`). `-previewCall 1` opens the call at launch; `-injectWav` replaces the microphone with a clip made by macOS `say` (24 kHz PCM16); `-bargeWav` is a second clip meant to fire 1.5 s into the reply. The app prints `metric <name> <ms>` lines; the console is kept per cycle under `~/mobile-out/<cycle>/voice/`.

| cycle | date | engine / route | connect | speech end → first reply audio | barge-in → playback cut | notes |
| --- | --- | --- | --- | --- | --- | --- |
| 3 | 09-15 | duplex · speaker · kernel tools on | 732–839 ms | **671–693 ms** (three runs) | **2 415 ms** to the server's `speech.started`, playback cut 44 ms later | the model answered the question this time ("We are currently working on a project involving code optimization…"); the barge clip ("Stop, wait, one more thing") was heard as "eight one more thing" and got a second short reply (", what is it?") |
| 2 | 09-15 | duplex · speaker · kernel tools on | 606 ms | **669 ms** | not measured (see below) | transcript "o Arbus. What are we working on right now? Give me one sentence" — the first syllable lost; reply "Hi Arbos! How can I help you today? I am ready. Just tell me what you need." — generic, no kernel tool called; reply peak −16 dBFS, out −3 dBFS after normalisation |

## Asks to the voice server / gateway (from cycles 2–3)

0. **Barge-in over the speaker is 2.4 s late (cycle 3, M-36).** The client marks `client.speaking` while the reply plays and the gateway's echo gate ignores the microphone for that span, so Jacob's interruption is heard only once the reply ends. With a headset (AirPods carry their own echo cancellation) the gate should be off — the client can send the route with `client.speaking` (`route: airpods | speaker`) and the gateway apply the gate only for `speaker`; on the speaker itself, speech well above the echo estimate (the client's own `--echo-margin` logic) should still pass. Target: barge → playback cut under 300 ms.

1. **The question was not acted on.** With `kernel tools` on, "what are we working on right now" should have gone to the kernel (`status`/notes) and come back as one sentence; the model small-talked instead. Either the tool contract is not reaching the duplex model, or the greeting turn pre-empts the first question. Please check the gateway log for this session (15:54 UTC, `arboslife/demo` attached through the hub).
2. **First syllable clipped.** The transcript starts at "o Arbus" — the clip's first ~150 ms were dropped, so either the injector starts before the server is listening or the VAD's onset is late. If a real mic shows the same, the first word of every utterance is at risk.
3. **Loudness**: reply peak −16 dBFS before the phone's normalisation; the earlier "14 dB low" finding stands on the server side.

## Harness notes

- Cycle 2's barge clip never fired: the injector started after `joinChat()`, which on a hub-attached kernel returns after the first reply. Cycle 3 arms it before the join; the clip now fires 1.5 s into the reply and the two `barge_in_*` metrics print.
- No microphone or speaker exists on the EC2 Mac; everything above is in the audio pipeline, not the room. Speaker/AirPods routing, echo and the real first-word feel need Jacob's iPhone.


## Re-measured 2026-09-16 (cycle 10, Gemini-flash default after #284)

Same harness (`mac-voice.sh`: `say` clips as the microphone, DEBUG metrics on the console), simulator, speaker route.

| metric | cycle 3 | cycle 10 |
| --- | --- | --- |
| connect (attach → duplex) | — | **817 ms** |
| first-word latency (speech end → first reply audio) | 0.5–0.7 s | **679 ms** |
| barge-in (barge speech → playback cut) | 2.4 s, through the echo gate | **not achieved**: the barge clip, fired 1.5 s into a ~3 s reply, was not heard as an interruption; `response.done interrupted=false`, then the server answered the barge as a *new* turn ("I am ready. Just tell me what you need.") |

Two things for the voice server:

1. **Barge-in on short replies.** A reply of three seconds is the common case on a phone; if the echo gate needs longer than that to open, the user can never interrupt a short answer, and their words become the next turn instead. Ask: open the gate on speech *energy above the playback's echo estimate* rather than on time, or at least shorten the guard to ~500 ms.
2. **The reply ignores the project.** "What are we working on right now? Give me one sentence." was answered "Hi Arbos! How can I help you today?" — the server answered by itself, without the kernel's context. In the call, a question about the project should go to the kernel (the chat's source) and the voice should read its reply; the app already forwards typed lines to the kernel when `answersItself` is false. Ask: make the kernel the answerer in call mode, or expose that switch.

ASR note: "Hello Arbos" arrived as "o Arbus" — the first syllable clipped; the clip starts at t=0 with no leading silence, so this may be the harness, not the server.


## Re-measured 2026-09-16 12:04 UTC (cycle 12, gateway PR #56 live on the pod)

Same harness, simulator, speaker route, `client.speaking` now carries `route=speaker`. Answer to `internal/features-inbox/2026-09-16-voice-call-mode-kernel-answers.md`.

| metric | cycle 10 | cycle 12 |
| --- | --- | --- |
| connect (attach → duplex) | 817 ms | **666 ms** |
| project question → kernel-answered first audio | answered by the model, wrong | **2.93 s**, routed to the kernel; the reply is about the project ("We are currently managing various creative writing and programming tasks … handled by several sub-agents.") and shows once in the Main chat as the user's own words plus the reply (`media/mobile/cycle-12/09-…`) |
| small-talk first audio ("Stop, wait, one more thing" → "Sure, what is it?") | 679 ms | **696 ms** |
| barge-in: barge speech → `speech.started` | not achieved | **401 ms** |
| barge-in: barge speech → `response.done interrupted` (playback cut) | not achieved | **404 ms** |

What Jacob will experience on the phone, from these numbers: a question about the project takes a breath — about three seconds — before the voice starts, and speaking over the answer stops it in under half a second. Small talk answers in well under a second.

- **Requirement checked: the app does not forward `transcript.final` to the kernel in duplex mode.** `CallViewModel` forwards only when `!server.answersItself`, and `answersItself` is true for `engine == "duplex"` (`session.ready`). Nothing to change; the project gets his words once.
- `client.speaking {speaking, route}` now names the output: `airpods`, `bluetooth`, `wired`, `headset`, `earpiece`, `speaker` from `AVAudioSession.currentRoute` (PR `cursor/mobile-cycle-12-feedback-4acc`).
- **Not measured here: AirPods.** The simulator has no Bluetooth route; the numbers above are the speaker path. The AirPods leg — the gate bypass and the barge feel through the earbuds — needs Jacob's phone, build after this PR, one call over AirPods: say a project question, talk over the answer. The app's DEBUG metrics are not in a TestFlight build, so his report is the measurement: does the voice stop within half a second of him speaking.
- Kernel ask (small): the transcript `user` event the kernel sends clients does not carry `channel`; the app marks a spoken turn "Spoken" when it does. Filed in `internal/features-inbox/2026-09-16-mobile-spoken-turn-channel-on-transcript.md`.
