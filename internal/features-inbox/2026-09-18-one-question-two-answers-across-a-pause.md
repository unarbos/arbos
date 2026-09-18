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

---

## Re-checked against #562 (`1e0e9cde`), 09-18 07:05 UTC — half of this is fixed

Six runs of `deploy/mobile/scenarios/one-breath-one-answer.sh`, driving
`pause.wav` down the capture path against the live gateway:

| | result |
| --- | --- |
| runs whose **transcript** split | **0 of 6** |
| runs **answered more than once** | **6 of 6** (2, 3, 4, 3, 2, 3 replies with audio) |
| frames captured vs sent | **900 / 900**, every run |

**Fixed:** the breath no longer splits the transcript. Every run returns the
whole question as one final — `Hello Arbus. What are we working on right
now? Give me one sentence.` That was the first half of this report and it is
gone.

**Still open:** one transcript still draws more than one spoken answer. From
run 2, with only that single transcript in the log:

```
reply: Right now, nothing's in progress; all recent workers have finished.
event response.done reason=completed playing=true
phase listening
reply: We just finished having two workers each run a timed sleep command (296 and 304…
```

No second transcript precedes the second reply, so this is no longer a
segmentation fault — the gateway hears one question and answers it twice.
The caller gets a complete answer, then a second one over the top of it.

**The phone is still not in it.** 900 of 900 frames captured and sent on
every run, so the stream the gateway received was continuous.

This loop has a committed re-check now, so ask for it rather than a fresh
investigation: `one-breath-one-answer.sh <cycle> <runs>` prints both counts
separately.

**Confirmed again at 07:12 UTC**, two more runs on the same gateway build:
transcript split **0 of 2**, answered more than once **2 of 2** (2 and 4
replies), 900/900 frames. Eight runs in total now — **0 of 8** split,
**8 of 8** answered more than once.

---

## The breath is not involved in what is left, 09-18 07:20 UTC

The same counter, pointed at a clip with **no pause in it** —
`acceptance.wav`, which asks two separate things:

| clip | what it asks | transcripts | answers with audio |
| --- | --- | --- | --- |
| `pause.wav` | **1** question, one breath in the middle | **1**, every run (8 runs) | **2–4**, every run |
| `acceptance.wav` | **2** utterances, no pause | **2**, every run (3 runs) | **3–5**, every run |

Transcription is correct in both: one question transcribes as one final, two
utterances as two. **Answers are over in both**, by roughly one per question,
whether or not a breath is involved.

So the pause is a red herring for the half that remains. This item is filed
under a pause because that is how it was first met at cycle 43, and the
transcript half really was a pause fault — #562 fixed it. What is left is
simply that **the gateway answers a question more than once**, and it does
it to any question.

The replies are distinct answers to the same question, not one reply in
parts. From `pause.wav` run 2, with a single transcript in the log:

```
reply: Right now, nothing's in progress; all recent workers have finished.
event response.done reason=completed playing=true
phase listening
reply: We just finished having two workers each run a timed sleep command…
```

The phone is not in it in either case: 900 of 900 frames on all eleven runs.

Re-run either side with
`CLIP=<wav> SAYS=<how many things it asks> one-breath-one-answer.sh <cycle> <runs>`.

---

## Closed, 09-18 07:54 UTC

Six runs of `pause.wav` between 07:50 and 07:54: **one transcript and one
answer in every one**, 900/900 frames throughout.

```
runs with more than 1 transcript(s): 0 of 4
runs with more than 1 answer(s):    0 of 4
VERDICT: one breath, one transcript, one answer. M-146 does not reproduce.
```

Eleven runs earlier the same evening — 07:05 and 07:12 — gave two to four
answers each, so the remaining half was fixed between 07:12 and 07:50.

Both halves are now done: #562 stopped the breath splitting the transcript,
and this change stopped one question being answered twice. Nothing further
is asked of the gateway here. Thank you — and the re-scoping seems to have
been the useful part: while this was filed as a pause fault it sat, and once
it was shown to happen without a pause it moved within the hour.
