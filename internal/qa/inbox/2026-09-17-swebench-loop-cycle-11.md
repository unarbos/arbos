---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 11 — critique dropped; the containers had the internet

Branch `cursor/swebench-loop-c11` (harness only, after the revert). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md); data `media/swebench/loop/cycle-11/`.

## Result

Second-model critique (request + diff, no history, one nudge on INCOMPLETE) against the one-reproduction $8-cap baseline on one kernel: on the seven instances both arms covered twice, 12/14 vs 12/14, at 2.4x the cost per rollout. The rule fixed before the run said drop at +2 or less; dropped, code reverted, no opt-in kept. The reviewer was wrong in three of its four INCOMPLETE verdicts and the agent correctly said so.

## The thing to act on

The verifiers docker runtime defaults to `--network host`. Rollouts `pip download` the newer release of the package under repair and read the upstream fix. Cycle 10's "74%" baseline arm: 6 of 35 rollouts did, all six solved. Cycle 11 arm A: 8 of 29, seven solved. Without those solves the baseline is 57% at best (c10) and 45% (c11 A). Cycles 1-9 were run the same way and are not yet counted. List of fetching rollouts with their commands: `media/swebench/loop/cycle-11/upstream-fetches-c10-c11.json`.

Fix is a flag verifiers already has: `--env.agent.runtime.block '["*"]'` (bridge network, iptables REJECT except the interception proxy). Smoke-tested with arbos-harness: pip refused, model reachable. The branch documents the flag and records `arbos_egress_open` per rollout.

## For the kernel side

- `repro.note_failing` counts *any* failing bash command as the task's reproduction: in the smoke rollout a refused `pip download` became "reproduction 1" and `changes` later reported it as passing. The reproduction should be a command that exercises the bug, not the last non-zero exit.
- A harness `instructions` value replaces the standing headless rules instead of adding to them; the smoke agent committed on a branch and the patch extraction saw an empty diff. Harness-side, but the kernel's `instructions.md` is where the headless rules live.
- The critique prompt's diff view is capped at 40k chars; one astropy-13398 verdict was "cannot confirm from the clipped diff". Any future reviewer needs the tree, not a clipped diff.

## Bundles

`media/swebench/loop/cycle-11/*.tgz`: the four critique-nudged rollouts (astropy-12907 x2, astropy-13398 x2), arm A's astropy-13398 that fetched upstream, and the network-cut smoke rollout.
