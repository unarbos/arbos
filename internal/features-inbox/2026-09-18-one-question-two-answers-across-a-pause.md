---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

# One question, two spoken answers, when the caller pauses

**For:** whoever owns the voice gateway.
**From:** the iPhone loop, cycle 68, 2026-09-18.

This is M-146, first seen at cycle 43 and still here. Re-filing rather than
leaving it in the ledger, because the earlier report predates both the
Silero/Whisper change and the phone's six-second capture hold, and either
could reasonably have been assumed to close it. Neither did.

## What happens

`pause.wav` is the loop's standard question cut in half by a silence —
`p1.wav` + a gap + `p2.wav`, four seconds in all. One request, as a person
would say it with a breath in the middle.

Played down the real capture path (`-micWav`, so the app's microphone route,
not an injected reply), the gateway returns:

```
transcript: Hello Arbus. What are we working on right now?
transcript: Give me one sentence.

event response.done reason=completed playing=true reply peak=-3dBFS rms=-15dBFS
event response.done reason=completed playing=true reply peak=-3dBFS rms=-16dBFS
```

Two transcripts, two `response.done`, both `completed`, both with audio
played. **The caller hears two replies to one question.**

## It is not the phone

Counted at both ends on the same run: **900 frames captured, 900 sent**. No
loss, no gap, nothing dropped at the socket. The stream the gateway received
was continuous; the split is its own segmentation of it.

That was worth establishing before filing, because the phone has been the
culprit for this family before — cycle 42's first-word loss was ours, and
the six-second hold landed today for a related reason (#557).

## Why it matters more than it sounds

A pause mid-question is how people actually talk, especially the first time
they use a voice interface. The failure is not a wrong answer; it is being
answered twice, the second reply arriving over the top of the first. On the
phone the orb settles and re-fires, which reads as the app having lost
track.

## Reproducing

On the loop's Mac:

```
xcrun simctl launch --console-pty <udid> com.unarbos.arbos.ios \
  -previewCall 1 -micWav ~/mobile-clips/pause.wav
```

then `grep -E "transcript:|^event response.done"` the console. Two of each
is the fault; one of each is fixed.
