---
cursor:
  subagentId: "bc-7c66cfa8-381e-5700-9d78-3129f338a4fa"
---

# A barge-in starts a turn that never finishes

**For:** whoever owns the voice gateway.
**From:** the iPhone loop, 2026-09-19, measured on `main` `e5e66f71`.

## What happens

Speak over Arbos while it is talking. The interrupt itself works, and works
quickly — `barge_in_speech_started` **217–330 ms**, `barge_in_response_done`
**263–381 ms**, across every run that could barge in (cycle 124, cycle 145).

Then nothing. The app forwards the interrupting speech, sets `kernelBusy`,
and the call sits at `thinking` for as long as you leave it — sixteen
seconds in the run below, and it only moves when the caller speaks again.

```
10s speaking      barge at 288 ms, gateway confirmed at 330 ms
12s listening     event response.done reason=interrupted
14s thinking
...
32s thinking      — no further frames at all
```

The app's console ends at `response.done reason=interrupted`. The kernel's
own record shows **nothing from the call**.

## What is not wrong

The orb. I spent twenty cycles treating this as a display fault — the app
promising a reply that would never come — and changed the phase handling to
settle back to `listening`. It changed nothing, because `settle()`'s guard
was blocking it: a turn genuinely is pending. The phase is honest.

## The ask

After an interrupt, the utterance that did the interrupting should either
get an answer or the turn should end. Today it does neither, and the caller
is left in front of a circle that will wait forever.

A stall line exists for the chat — "Still working, but nothing has happened
for 5m" — and there is no equivalent in a call. If the turn cannot be made
to finish, saying so after a few seconds would at least let the caller act.

Nothing is asked of the phone until that is decided; the orb already draws
whatever the state says.
