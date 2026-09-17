# qa-027: a barge-in drops a queued approval question; the caller never hears it

- Feature: call mode, voice gateway narrator (`voice-server/`, PR #100 @ `d9007f8`)
- Severity: medium-high. Combined with qa-026 it is the worst case: an unheard approval, then a "yes" meant for something else.
- Status: fixed in [#111](https://github.com/unarbos/arbos/pull/111); pod gateway redeployed.

## Repro

`python -m tests.run approval-survives-barge-in`

1. The caller says "Clean the build directory." The speech model answers at length.
2. While the model is still talking, the kernel's approval frame arrives; the narrator queues "Arbos wants to run bash: … Allow?" behind the model's speech.
3. The caller barges in: "Hold on a second."

## Expected

The approval question is spoken once the caller stops. The caller's "Yes, allow it." then closes it.

## Actual

`Narrator.interrupted()` empties the whole queue (`heard = False`) — highlights and the approval line alike. "wants to run bash" is never spoken (timed out after 20 s in the harness); the approval sits open until the 45 s timeout denies it, or until any later utterance starting with a yes-word allows it (qa-026).

Rollout: `voice-server/tests/out/approval-survives-barge-in/frames.jsonl` (only `response.done interrupted` after the barge-in; no `narrator.say` of kind approval).

## Fix (#111)

`Line.asks` marks lines that put a question to the caller (approval, re-ask, ask). Those are re-queued on interrupt, never evicted by `QUEUE_CAP`, and skipped once the ask is closed.

Regression scenario: `approval-survives-barge-in`.
