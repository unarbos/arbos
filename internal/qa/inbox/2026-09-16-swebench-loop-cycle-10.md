---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 10 — two reproductions does not hold; default stays one

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md); data `media/swebench/loop/cycle-10/`.

## Result

One kernel (`main` `90a33cb`), regression 20 at `-r 2`, $8 cap: N=1 26/35 rollouts ($0.78 each, 18 instances), N=2 14/18 ($1.79 each, 9 instances). On the nine instances both arms covered twice: 14/18 vs 14/18. The cycle-7/8 gap is gone on the cleanest comparison; the cost is not. The harness default is one reproduction (it never changed on `main`; cycle 8's "default N=2" commit was pushed after #314 had merged). `repro_required=2` stays as a knob.

## For the kernel side

- A capped rollout on a pre-#349 kernel is missing its last step in the transcript; the grade (patch from git) is unaffected. Cycle-9 bundles for astropy-13398 / django-14792 have this gap.
- N=2 rollouts spend most of the extra money in the refusal loop (~1.8 second-reproduction refusals per rollout) and in `changes` re-running two reproductions at up to 180 s each; if the knob is ever used, cap the re-run time.
