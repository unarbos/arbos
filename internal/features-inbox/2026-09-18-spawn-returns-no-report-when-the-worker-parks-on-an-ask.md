---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# `spawn` returns "no report" when the worker parks on an `ask`

Found by the desktop symmetry loop, cycle 49, long-form d20 (`d20_drive.py`,
stills under `media/cursor-reference/cycle-49/d20/`), kernel
`arbos-kernel 0.2.0 f02aadae63df protocol 1` on `main` `1a0b17aa`.

## What happened

The root was told: *Delegate one worker with this exact brief: "Before doing
anything, use the ask tool to ask the user which colour the banner should be
(options: red, blue). Then write banner.txt with that colour and report."
Wait for the worker.*

The worker did as told and parked on its `ask` (*Which color should the banner
be?*). The root's `spawn` call then returned, from its transcript:

```
"kind":"tool","name":"spawn", … "body":"write-banner reports:\n(the child's turn ended without a report)"
```

The root read that as failure. First run: *The worker did not return a result.
It may have failed or ca…*. Second run: the kernel nudged the root (*Your reply
was empty. Continue the task…*), the root then said *The "write banner" worker
is still idle. I will try to steer* and sent the worker *Please continue with
your task*, to which the worker answered *I have already asked the user for
the banner colour…* — a loop of two agents talking past a question only the
person can answer.

## What the parent should be told

`spawn` (and `await`) should distinguish three ends of a child's turn:

1. it reported (`done <report>`) — as today;
2. it ended without a report — as today;
3. **it is waiting on the user** — a new shape, e.g.
   `write-banner is waiting on the user: "Which color should the banner be?"`
   and the parent's own status set to *waiting on <child> — a question for
   you*, the way it already reads *waiting on Implement todo commands —
   starting*.

With (3) the root has no reason to steer or apologise, and the desktop can say
the same thing in the root's pane and the panel row (the desktop half — the
panel row reading *asking* with the question as its line — is F-207 in #703).

## Where the desktop stands

The worker's own chat draws the ask card correctly, and after F-207 its panel
row reads *asking* with the question. What no pane can say today is *why* the
root thinks the worker failed: that sentence is the model's reading of the
kernel's `(the child's turn ended without a report)`.

## Seen beside it: the model repeats the `[kernel]` aside

In the worker's own transcript (`archive/agents/write-banner/transcript.jsonl`):

```
"kind":"assistant","text":"I have already asked the user for the banner color. I am waiting for their response to proceed.\n\n[kernel] The user provided the following answer to your question: red\n[kernel] The user provided the following answer to your question: red"
```

Gemini echoed the kernel's answer line twice into its reply. The desktop now cuts `[kernel] …` lines from prose (F-211, #703); the kernel might want the answer delivered as a `user`-shaped line the model does not feel it must quote, or a prompt note that `[kernel]` lines are never repeated.
