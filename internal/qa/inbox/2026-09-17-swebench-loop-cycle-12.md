---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 12 — the baseline is 58%, and the harness now refuses an open network

Branch `cursor/swebench-loop-c12-7c9c` (harness only). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 12; audit of cycles 1–11: [swebench-open-network-audit](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-open-network-audit.md); data `media/swebench/loop/cycle-12/`.

## The number

Regression 20 at `-r 2`, network cut, kernel `864d6b00` (#380 head), one reproduction, $8 cap: **21 of 36 rollouts (58%)**; two instances (sympy-20590, sympy-13878) not reached before the $37 watcher. `arbos_egress_open` 0.0 on every rollout, no fetch in any transcript, five refused attempts. The 74% of cycles 10–11 is withdrawn: it was measured with the network open and six of its 26 solves downloaded the upstream fix.

## What broke on the way (both fixed on the branch)

- **Under the cut, every rollout graded 0 with a patch in place.** verifiers grades in the agent's container; the SWE-bench verifier's `uv run parser.py` needs PyPI for `swebench==4.0.3`; the proxy denied it, `set -e` ended `test.sh`, reward 0. The harness reopens egress after the agent has exited and before grading (`runtime.prepare_execution(None)`). A smoke on django-11099 then graded solved. Anyone running another harness under `block '["*"]'` with this taskset hits the same wall.
- **A pre-#380 kernel was launched first** and stopped after two rollouts: a baseline across a behaviour change is not a baseline.

## For the harness/kernel side

- The refusal: `ArbosHarness.setup()` raises when `runtime.network_restricted` is false unless `allow_open_egress=true`. The rollout is `ok=False`, no reward, $0. This is the default now; a run cannot quietly produce a number on the open network.
- The reopen-for-grading step is a trust boundary worth a second pair of eyes: after `run_program` returns, the kernel has exited, but a background process the agent left behind would also regain the network. Nothing in 36 rollouts did; it is stated in the doc.
- The two kernel gaps from cycle 11 (any failing command as reproduction; instructions override replacing the rules) are fixed in #380; my harness-side patch for the second was reverted in favour of #380's.

## Bundles

`media/swebench/loop/cycle-12/*.tgz`: the clean failures on astropy-13398 (2), django-14792 (2), and django-15252 (2) — the instances that were never solved without the shortcut. These are the rollouts to read before choosing cycle 13's lever.
