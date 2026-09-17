# qal-j18: the wait for the checkpoint tree is shown as the command running, and recorded as the command's own time

- Measured at: #419 @ `2daa555d` (`arbos-kernel 0.2.0 2daa555dbcc0`), scenario `rw-10c-a-long-wait-for-the-checkpoint-tree-is-shown-as-the-command-running`, rollout `internal/qa/rollouts/20260917T102336Z-rw-10c-…`; control `5340c0d2` has no wait (the probe reports itself as proving nothing there, as it should).
- Class: misreport, the new behaviour's face. Data is right (`qal-j17` closed); the story the window tells is wrong.
- Feature: the checkpoint wait before a turn's first write (#419 at `2daa555d`, `turn.rs`).

## What the person sees

The kernel's own knob for a slow `add -A` (`ARBOS_TEST_TREE_DELAY_MS=6000`, a large repository) and one turn whose first call is `echo first > f1.txt`. From the moment the message is sent, in order:

- `status … step: "Running echo first > f1.txt"` at +0.01 s, and a tool card `write f1` with `started` set and no `ended` — a running command;
- nothing else for 6.2 s;
- the tool card's `ended` at +6.3 s, body `(no output)`; the turn finishes.

The transcript's tool record says `started`…`ended` = 6.3 s. The only place the truth appears is the kernel's stderr: `root: bash waited 6.0s for the turn's checkpoint tree`.

So it is not the silent stall (a status and a card appear at once) — it is the wrong explanation. A person on a large repository watches `echo` "run" for six seconds and concludes the shell, the machine or the command is slow; a history or duration view charges the six seconds to the tool; anyone reading the transcript later sees a six-second `echo`. The wait itself is honest and worth having; it needs its own name.

## What we expect

Before the tool's `started`: a status step of its own — *"Saving a checkpoint of the working tree before the first change"* (with elapsed time once it passes a second or two) — and the tool's `started` set when the command actually starts, so the record shows the wait and the command as two things. The stderr line can stay; it is for us, not for the person.

## Regression check

`rw-10c` passes when the first status step after the message names the checkpoint (or the wait) rather than the command, and the tool record's `started`…`ended` is the command's own time (under 2 s here). Breaks now on `2daa555d` with the message above; on a build without the wait it reports `probe-did-not-wait`, which is a skip, not a pass.
