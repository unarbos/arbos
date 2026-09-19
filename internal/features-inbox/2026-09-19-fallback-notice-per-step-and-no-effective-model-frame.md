---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# The fallback notice lands once per step, and the window never learns which model answered

Found by the desktop symmetry loop, cycle 54, long-form d25 (`/tmp/d25-proj`,
`media/cursor-reference/cycle-54/d25/`), kernel
`arbos-kernel 0.2.0 f02aadae63df protocol 1` on `main` `1a0b17aa`. Config:

```toml
model = "google/gemini-does-not-exist-9"
fallback_models = ["google/gemini-2.5-flash"]
```

## What happened

The kickoff turn wrote, verbatim from `.arbos/agents/root/transcript.jsonl`:

```
{"kind":"notice","text":"google/gemini-does-not-exist-9 rejected the request, so google/gemini-2.5-flash answers this turn.","failed":false}
```

**three times in the one turn** — once per model call (the kickoff's steps),
with the empty-reply nudge between two of them — and once more on the next
turn. `turn.rs` says of the blocked note *the user hears why in one plain
sentence, once per turn*; the per-step path (`Models` in `step.rs`) does not
keep that promise, and the pane draws every copy: the first stands above the
turn's *Worked 13s* header, two more inside it.

The composer's model chip read `google/gemini-does-not-exist-9` throughout —
the configured id — while `gemini-2.5-flash` did every turn. The window has
no frame that says which model actually answered: the `Provider` frame at
the handshake carries the configured model, and nothing follows when a
fallback takes the turn. Cursor's chip names the model that is answering.

## What would hold

- One fallback notice per turn (the first step's), or one per turn *and*
  model change.
- A frame, or a field on `turn_complete` / the status frame, with the
  **effective model** of the turn, so the chip can say *gemini-2.5-flash
  (fallback)* while the dead one is configured. The desktop shows what the
  kernel tells it; today it is told only the configuration.

## Desktop side

The pane draws the record as written; a same-words dedupe inside one turn
would hide the symptom, not the cause, so it is not added. The chip reads the
configured model because that is the only model it is told.
