# qa-032: `rewind` cuts at the turn's `user` line and leaves its `wake` behind; the next kernel start fires a turn with no prompt

- Feature: kernel `rewind` (transcript cut + file restore), `main` @ `888f512e` and PR #140 `cursor/standing-pass-kernel-b027` @ `bff0fb5a` alike
- Severity: medium. Every rewind leaves the transcript inconsistent (the consistency checker's `transcript-unended-turn` rule fires) and the next restart spends a model turn on nothing — the agent speaks unprompted, or with a key, bills a turn.
- Scenario: `mt-29-rewind-latency` (checks `mt-29-dangling-wake`, `mt-29-rewound-prompt-refired`); rollouts `internal/qa/rollouts/20260914T032342Z-mt-29-rewind-latency/` (PR), `20260914T032720Z-mt-29-rewind-latency/` (main)

## Repro

1. Three user turns "one", "two", "three" (no key needed; each ends with a failed notice and `turn_complete`).
2. `{"type": "rewind", "agent": "root", "turn": 2, "files": true}`.
3. Read `transcript.jsonl`; restart the kernel.

## Expected

The transcript ends after turn 1's `turn_complete`. A restart does nothing.

## Actual

After the cut the transcript is: `wake one, user one, notice, turn_complete, wake two`. The `wake` line that opened turn 2 survived (the cut is at the `user` line, `line: 6`; the wake is line 5). On restart `needs_serve()` sees an unended wake and fires: `wake serve → notice → turn_complete` with no user prompt.

The `rewound` timing itself is right on both kernels here (first frame 0.6 ms, `pending: true`, then `restored`): the 6 s settle needs a large store to show.

## Suspected location

`crates/arbos-kernel` rewind: the cut index is the N-th `user` line; it should be the `wake` that opened that turn (the line just before it when `kind = wake` and the wake's text is the user line's text, or any `wake` between the previous `turn_complete` and the `user` line). `transcript.rewound-<ts>.jsonl` gets the same lines.
