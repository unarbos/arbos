# qal-j20: after a rewind, a cut turn's checkpoint sidecar is taken for the new turn at the same line — the next rewind brings back files the person had rewound away and loses the new session's

- Measured at: #419 @ `a5072074` (`arbos-kernel 0.2.0 a50720741978`), scenario `fm-01-stale-checkpoint-sidecar-from-a-cut-turn-is-taken-for-the-new-turn-at-the-same-line`, rollout `internal/qa/rollouts/20260917T1216*-fm-01-…`. The reader is the same on `main` (`settle_tree`, `tools/git.rs`); the probe forces the pending state with #419's `ARBOS_TEST_TREE_DELAY_MS` knob, which `main` lacks, so `main` reports "proves nothing" rather than a pass. In the wild the state is a rewind pressed soon after a turn that wrote nothing, on a repository whose `add -A` takes seconds.
- Class: destructive with a false "restored" — the qal-j08 family, and the first member of the **first-match** family found by staging it: a reader that trusts *where* it found a record (`checkpoints.d/<line>.json`) plus one weak fact (HEAD equal) instead of the fact that identifies the record (its `ts`).
- Feature: checkpoint sidecars and `settle_tree` (#405/#419).

## What happens

Line numbers are transcript lines. A rewind cuts turns 3–5 and removes the used checkpoint's sidecar (`checkpoints.d/14.json`) — and leaves the cut turns' sidecars `20.json`, `26.json` in place. New turns then reuse those lines. New turn 4' starts at line 20; its record is written pending; its tree is still being saved (here: delayed 8 s; on a large repository: however long `add -A` takes). A rewind to 4' arrives. `settle_tree` reads `checkpoints.d/20.json`, finds a record whose HEAD equals the pending record's HEAD (HEAD has not moved all session — the common case), accepts it, and restores **the cut turn 4's "before" tree**: `{f1, f2, f3}`. The true tree before 4' was `{f1, f2, g3}`.

Measured: after the second rewind the files are `f1.txt f2.txt f3.txt` — `f3.txt`, which the first rewind had removed, is back; `g3.txt`, written by the new turn 3', is gone. The report says `restored f1a649ec77e9 + working tree 0a423548233d` — the stale sidecar's tree, named as a success. The stale sidecar's `ts` (1789647411837) differs from the new record's (1789647421659); the line and HEAD matched.

In a person's words: "I rewound past a bad turn, worked on, rewound one step, and the bad turn's file came back while my new one vanished — and it said restored."

## What we expect

Two independent fixes, either sufficient, both good:

1. `settle_tree` matches the sidecar on the record's `ts` (or a nonce written in both), not on line and HEAD. The identifying fact, not the location.
2. The rewind's cut removes `checkpoints.d/<line>.json` for every cut line, as it removes the records — so no cut turn leaves a sidecar for a later turn to inherit.

## Regression check

`fm-01`: passes when, after the second rewind, `f3.txt` is absent and `g3.txt` present. It reports itself as proving nothing on a build where the tree cannot be held pending (no delay knob and a fast disk).

## The family this opens (the audit's list, staged the same way)

For each first-match reader: put a stale copy where it looks first and a live one where it looks second, and assert it takes the live one. `fm-01` is the checkpoint sidecar. Designed next: the history lookup (`transcript_for_history`: live `agents/<id>/agent.md` first, then `archive/agents/<id>` — stage a live folder with `agent.md` but no transcript beside an archived one with the full transcript, as a kernel killed mid-archive would leave); the roster's per-machine files; the leash pointer beside the job folder.
