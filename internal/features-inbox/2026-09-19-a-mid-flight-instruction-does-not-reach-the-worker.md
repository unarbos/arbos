# A mid-flight instruction does not reach the worker doing the work

Filed at cycle 194. Not a fault: the instruction is honoured and the file
ends up right. The question is *who* honours it.

## What happens

The acceptance journey's J4 sends a follow-up while a delegated worker is
still running:

> Also add a line to the CHANGELOG saying who asked for this: QA-J213738.

The kernel does not pass this to the running worker. It makes the edit
itself. From the transcript of the run at `0919-213738`:

```
 4979 tool          spawn fix J213738 mathlib challenge
 4981 turn_complete
 4983 user          Also add a line to the CHANGELOG saying who asked...
 4985 tool          read journey_J213738/CHANGELOG.md
 4987 tool          edit journey_J213738/CHANGELOG.md
```

No second worker is spawned — that would be wrong and the journey already
fails it. The kernel reads and edits the file directly, four frames after
delegating the same project to a worker.

## Why it may not matter

The outcome is correct every time. J7 checks the CHANGELOG on the worker's
branch, by asking the kernel to run git commands and paste raw output, and it
carries `QA-<id>` in this run and in the twenty-three before it.

## Why it may

The worker is editing the same project at the same time. Two writers on one
CHANGELOG is a conflict waiting for a slower worker or a larger edit. Nothing
in the journey would catch it: J7 reads the end state, and the end state has
been right so far.

There is also the question of what a person expects. Typing a follow-up while
work is running reads as "tell whoever is doing this" — not "do it yourself
alongside them".

## What has been changed

Nothing in the product. The journey's J4 verdict now *names* this outcome
instead of misreporting it, which is the real finding of cycle 194: the old
verdict counted `turn_complete` frames between the challenge and the
follow-up and called any of them "the turn had ended". The kernel's own turn
completes one frame after it delegates, every run — so J4 could never pass.
Twenty-four runs on record: twenty-two unverified, one fail, one unexercised,
no passes.

It now distinguishes three outcomes: a second worker (fail), a kernel-side
edit (unverified, with the tool frames quoted), and the running work taking
it (pass).

## The decision wanted

Should a follow-up typed while work is running be routed to the running
worker? If yes, this is a kernel change and J4 starts passing. If no, J4's
"pass" case is unreachable by design and should be retired rather than left
looking like a target nobody hits.
