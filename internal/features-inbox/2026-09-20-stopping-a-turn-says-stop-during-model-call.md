# Stopping a turn says "stop during model call"

Filed at cycle 205, from watching a film rather than reading a verdict.

## What happens

Tap the composer's stop square while a turn is running. The turn ends
correctly — the square becomes a microphone again, the kernel's transcript
stops growing, and the record closes with `interrupted` then
`turn_complete`. All of that is right and `scenarios/stop-a-turn.sh` checks
it.

What the person is then shown, just above the composer, is:

```
 116  351  StaticText   stop during model call
```

## Where it comes from

Not the app. Searching the whole iOS source for that phrase, or for "during
model call", returns nothing — so the phone is rendering text the kernel put
in the interrupted frame. This is the same shape as cycle 200's worker lines:
the app faithfully displays a machine-facing string.

## Why it is worth a look

The phrase describes where in the pipeline the stop landed, which is a thing
a developer wants and a person does not. Somebody who has just tapped stop
already knows they stopped it; what "during model call" adds is that it
happened while the model was being called rather than during a tool run.

It also sits oddly next to the rest of the app's voice. The chat says
`Worked 1m 44s`, the workers sheet says `Turn ended`, the away card says
`While you were away`. Those are written for a reader. "stop during model
call" is written for a log.

Against that: it is honest and short, and the distinction it draws may matter
when a stop lands mid-tool and leaves something half-done. If so, the phrase
should say *that*, rather than naming the phase.

## The decision wanted

Either this string should be written for a person — "Stopped" is probably
enough, with the phase kept for the diagnostics — or the phase genuinely
matters to a user and should be said in words they would use. Either way it
is a wording change in the kernel, not the app, since the app is only
drawing what it is handed.

Not changed here: the phone is not where it lives, and this loop does not
ship product guesses.
