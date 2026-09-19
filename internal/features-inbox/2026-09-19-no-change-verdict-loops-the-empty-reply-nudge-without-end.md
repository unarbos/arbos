---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# A "no change" verdict loops the empty-reply nudge without end

Found by the desktop symmetry loop, cycle 53, on kernel
`arbos-kernel 0.2.0 f02aadae63df protocol 1` (`main` `1a0b17aa`), model
`google/gemini-2.5-flash` with `google/gemini-3.8-flash` as the fallback.
Still: `media/cursor-reference/cycle-53/three/arbos-no-change-loop.png`.

## What happened

The root of `/tmp/tw-proj` had already answered *Run `ls /definitely-not-here`
with bash and tell me the exact error text* once. The same line was typed
again. The turn never ended. In about two minutes the transcript took **50
nudges and 37 fallbacks**, this cycle repeating (verbatim, from
`.arbos/agents/root/transcript.jsonl`):

```
{"kind":"nudge","text":"Your reply was empty. Continue the task, or say what is blocking you — and do not mention the empty reply or this note: …","reason":"em…"}
{"kind":"notice","text":"no change: the tree already does what the request asks.","failed":false}
{"kind":"notice","text":"google/gemini-2.5-flash returned nothing twice, so google/gemini-3.8-flash answers this turn.","failed":false}
{"kind":"notice","text":"no change: the tree already does what the request asks.","failed":false}
```

The window read *Working · 1m 31s* over a column of those lines until the
kernel process was killed by hand; nothing was going to end it.

## Where it comes from

`arbos-engine/src/turn.rs` around line 1280: Jev's `jev_no_change` writes the
notice but the turn goes on to the chat model (only `jev_end` returns).
The chat model, told the tree already does what was asked, returns nothing;
the empty-reply nudge (`turn.rs` ~1563) fires; the retry asks Jev again, who
says *no change* again; after two empties the fallback model takes the turn
and does the same. There is no cap on the nudge, and *no change* never ends
the turn.

## What would hold

- A *no change* verdict should end the turn with that line as the answer
  (Cursor's agent says "no changes needed" and stops), or at least count as
  the reply so the empty-reply nudge does not fire on it.
- The empty-reply nudge needs a ceiling per turn (two, three) after which the
  turn ends with a failed notice, however the models behave.

## Desktop side

The pane drew every line, as it should draw the record; with #666 the nudges
would be hidden but the *no change* / fallback pair would still repeat.
Nothing for the pane to fix — the record is the problem.

## Seen beside it

On a fresh place, the *first* ask of *Run `ls /definitely-not-here` with bash
and tell me the exact error text* also drew *no change: the tree already does
what the request asks.* as a line between the command card and the answer
(`arbos-exit-mark.png`). The verdict is Jev's word to the engine — running
`ls` changes no tree — not a sentence for the person; as a `notice` it lands
in the chat like *nothing to compact yet* did (F-199). A `nudge`-class record,
or none, would keep it off the pane.
