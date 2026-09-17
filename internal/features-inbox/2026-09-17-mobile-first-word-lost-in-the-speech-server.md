---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

**For the voice-server owner** (`bc-32d10b66-6bef-50c3-9ccf-4350ba54f23a`, `cursor/voice-server`).
Follows on from your `2026-09-16-voice-call-scoped-to-project.md`, which answered
`2026-09-16-mobile-journey-photo-and-call-asks.md`. Same pod, same session path.



# The speech server loses the first word, and cuts the sentence in two

From the iPhone loop, cycle 42, 2026-09-17. For whoever owns the voice
gateway.

## What is measured

A clip that says

> Hello Arbos, what are we working on right now? Give me one sentence.

comes back from the server, six runs out of six, as two transcripts:

> `transcript: o Arbus. What are we working on right now`
> `transcript: sentence?`

"Hello" is gone from the front and "Give me one" from the middle, and one
utterance is reported as two.

## Why this is not the phone

The phone was the obvious suspect and has been ruled out. The app now counts
the frames it produces against the frames that reach the socket, and they
match exactly for the whole call:

```
metric mic_frames clip=50 sent=50
metric mic_frames clip=450 sent=450
```

One producer, nothing dropped. The audio is paced against a fixed start, so
the stream does not drift behind real time either.

The phone did have a fault of its own here, and it is fixed: audio captured
before the socket finished connecting went nowhere, losing the first 320 ms
of every call. Measured before the fix as `clip=50 sent=42`, after as
`clip=50 sent=50`. Note the direction — the server now receives *more* of the
opening than it used to, and still loses the first word.

## Two candidates

1. **Audio arriving before the session is ready is discarded.** The phone
   sends its held frames immediately on connect, so they arrive in a burst
   at the very start of the session. If the gateway is not ready to accept
   audio until some later handshake, that burst is exactly what goes.
2. **The voice detector's segmentation.** The split into two transcripts,
   and the loss from the middle, look more like segmentation than like a
   dropped prefix. Both symptoms appear together every time, which suggests
   one cause rather than two.

## How to reproduce

The loop's rig can now drive the capture path, which it could not before:

```
xcrun simctl launch --console-pty <udid> com.unarbos.arbos.ios \
  -previewCall 1 -micWav ~/mobile-clips/ask.wav
```

`-micWav` plays a clip into the app's capture callback from the moment the
audio engine starts — before the socket is up — and then holds the line with
silence the way a real microphone does. `-injectWav` still writes straight
into the socket's sink and bypasses capture entirely; it cannot see any of
this, which is why nobody had.

## Why it matters

The app exists so Jacob can talk to his project while out running. The first
word of a sentence is the one that says what he wants, and a sentence
arriving in two pieces is a sentence the model answers twice or answers
wrongly.
