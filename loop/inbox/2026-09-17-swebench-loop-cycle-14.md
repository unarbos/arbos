---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 14 — regression 20b baselined at 28/40 (70%); job shells outlive the kernel; cost_capped undercounted

PRs: [#397](https://github.com/unarbos/arbos/pull/397) (merged: sweep tells zombies from live survivors), [#413](https://github.com/unarbos/arbos/pull/413) (cost_capped reads the current notice). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 14; data `media/swebench/loop/cycle-14/`.

## The number

Regression 20b (approved by Jacob after cycle 13) at `-r 2`, network cut, kernel `main` `30eef166`: **28 of 40 (70%)**, all 40 rollouts run, $37.48. Egress closed and no fetch on every rollout; every sweep `left` 0. The set moves: eight instances split or fell, ten went 2/2 from a 1/2 history. The four never-run instances all solved 2/2 — easier than intended; two will be swapped for harder ones next cycle.

## The twelve failures

Agent-addressable 6: root cause named in the mechanism and the fix applied elsewhere (sympy-17318 ×2, scikit-learn-14629), a reproduction that proved the crash gone rather than the behaviour present (pylint-6386), an existing test that encoded the bug obeyed over the issue (sympy-15017 — the agent tried the right fix, saw the old assertion fail, and reverted to a special case), one capped rollout. Not agent-addressable 6: the maintainers chose a different path, scope or semantics than the issue implies (django-15252, django-15022 ×2, astropy-13236, sympy-18698), one incidental detail pinned (pylint-8898). Wrong mechanism: 0.

## For the kernel side

- **`arbos-kernel run` exits with jobs still running.** In 4 of 40 rollouts (all scikit-learn, long test runs) the pre-grading sweep found 4–10 live processes after the kernel had exited: the job-runner shells `sh -c D=$1; P=$2; shift 2; K=$PPID; C=${ARBOS_JOB_LOG_CAP:-...}` and the test processes under them. They were reparented to PID 1 and would have had the network back at grading time without the sweep. `run` should reap or kill its jobs on exit, the same way the desktop's kernel is expected to. `sweeps.json` in the cycle-14 folder lists every rollout's kill list; the four are the scikit-learn-14629 and -25102 bundles.
- **The mechanism gate makes the agent say where the fault is; nothing checks the diff goes there.** Five rollouts across cycles 12–14 (django-14792 ×2, sympy-17318 ×2, scikit-learn-14629) wrote the root cause down and edited a guard or a consumer instead. This is the lever proposed for cycle 15 (a content check between the mechanism's named identifiers and the diff, with one nudge). It builds on the mechanism argument; if you are changing the gate to check content rather than length (the `placeholder` finding), the same parser serves both.
- **`spend::TURN_CAP_PREFIX` is matched by text in the harness.** #349's rewording silently broke `cost_capped`. A machine-readable `reason` on the notice event would let the harness stop pattern-matching prose.
- The "tests are the spec" rule has a failure mode worth knowing: sympy-15017's existing suite asserted `len(rank_zero_array) == 0` — the bug — and the agent chose the fix that kept that assertion green. "A test never vetoes the requested change" is in the contract; in this rollout it lost to the instinct to keep the suite passing.

## Bundles

`media/swebench/loop/cycle-14/*.tgz`: sympy-17318 ×2 and scikit-learn-14629 (root named, fix elsewhere), sympy-15017 (test obeyed over issue), pylint-6386 (crash-only reproduction).
