---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 13 — baseline 24/40 (60%); the clean failures are mostly not the agent's; the mechanism gate accepts "placeholder"

PRs: [#393](https://github.com/unarbos/arbos/pull/393) (merged: refusal, sweep, reopen for grading), [#397](https://github.com/unarbos/arbos/pull/397) (sweep tells zombies from live survivors). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 13; data `media/swebench/loop/cycle-13/`.

## The number

Regression 20 at `-r 2`, network cut, kernel `864d6b00`: **24 of 40 rollouts (60%)**, complete. Cycle 12's 36 plus sympy-20590 2/2 and sympy-13878 1/2. Every rollout: egress closed, no fetch, no live process left when the network reopened for the verifier.

## The twelve clean failures

Read against the gold patches and FAIL_TO_PASS lists. Wrong mechanism: 0 of 12. Wrong layer: 2 (django-14792 — the agent wrote the root cause in its summary and fixed the three consumers). Preserved behaviour the maintainers changed: 2 (django-15022). Hidden tests grade something the issue does not determine: 8 (pylint-8898's exact error string on malformed input; sphinx-7590's ID-mangling scheme; xarray-6992's 12-test redesign behind a one-line symptom; astropy-13398's refraction and topocentric ITRS). Table in the loop doc.

## For the kernel side

- **The mechanism gate accepts a placeholder.** django-14792 rollout `da33e301`: `mechanism: "placeholder"` passed and the edit went through. The gate checks length (`MIN_LEN`), not content. A mechanism should at least name a symbol or a file that exists in the tree, and one that is a single word or matches a stop-list (`placeholder`, `tbd`, `fix`, `n/a`) should be refused with the same message as a missing one. Bundle: `media/swebench/loop/cycle-12/swe-bench_django__django-14792--da33e301.tgz`.
- **"Fix at the root" does not hold when the root is named.** Both django-14792 rollouts state "`_get_timezone_name()` changed in 3.2 to return the full name" as the root cause and then edit `_prepare_tzname_delta` in three backends. If the kernel ever checks the mechanism against the edit, this is the case: the mechanism names a function the diff does not touch. Not a proposal for a gate yet; a pattern to know.
- The sweep found 0 live survivors in 44 normal rollouts and 1 zombie in each (the exited kernel; PID 1 is `sleep infinity`). The kernel leaves nothing running after `run` exits in this configuration.

## The set

Regression 20 is decided before the run: ten instances at 100% clean across 11–27 rollouts each, five at 0%, two near 0, two with variance. A replacement set built from the audit's variance instances is proposed in the loop doc for cycle 14.
