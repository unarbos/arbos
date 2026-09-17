---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 9 — the per-turn dollar cap and `changes` before done

PR [#347](https://github.com/unarbos/arbos/pull/347) (`cursor/swebench-loop-c9`, base `main`). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md).

## What changed (attack these)

1. `max_turn_cost_usd` (config) / `ARBOS_MAX_TURN_COST` (env): the turn ends with a failed notice when its summed model cost passes the cap. Probe: (a) the notice prints the cap with two decimals — `$0.00` for a 0.002 test cap; (b) the cap is per *turn*, reset by every user message — a steer mid-task resets nothing, a follow-up message does; (c) cost comes from the provider's `usage.cost` — a provider that does not price calls never trips it (Prime Inference? custom endpoints); (d) a capped turn leaves the working tree half-edited with kernel exit 2 — the desktop should show why.
2. `ARBOS_CHANGES_BEFORE_DONE=1`: one nudge per turn when a final reply follows edits without a `changes`. Probe: the nudge after a `write` of a non-code file (a doc) is noise; `changes` re-runs reproductions with a 180 s cap each — a nudged `changes` can take minutes.

## Numbers

Regression `-r 2`, same kernel: A (N=2, cap $4) 14/25; B (+changes before done) 15/22; shared 10×2: 12 vs 14. Three capped rollouts, all hard instances that had solved at $6–14 before, all lost → harness cap default $8. N=2's gain from cycle 8 did not reproduce on this kernel at $4 (12/18 on the first nine vs 15/18).

## Infra

Arm B's verifiers eval hung 30 min after its 22nd rollout with two idle containers (no processes inside); stopped politely. Also: a `pgrep -f`/`kill` loop whose pattern appears in the calling shell's own command line kills the caller — happened twice this week; match on the binary path.
