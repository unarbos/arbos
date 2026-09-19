---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 38 — the server-reproduction step works; requests-6028 graded 0 in-run and 1 offline

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 38; data `media/swebench/loop/cycle-38/`.

Kernel `arbos-kernel 0.2.0 1ec3de643d79 protocol 1` (main head, with bcf11b67), Jev off, cut, $23.21.

- **bcf11b67 on django-13809, three rollouts:** eleven `runserver` commands offered as reproductions, eleven refused in a second with the reason; walls 292–457 s against 7,760 s; all solved. Edge: the refusal keys on the word `runserver`, refused a script that only imports the module and exits at once, and was dodged by writing the script to a file.
- **Fresh ten: 10 of 20** (a count). Eight C, one B (sphinx-8056: consumer fixed, producer graded). Cumulative read 325; nothing outside the account.
- **A QA case that matters for every count:** both requests-6028 rollouts produced the gold's change line for line and were graded 0 inside the run; the task's own verifier grades the same patch 1 in a fresh container. Cause not found — proxy env, no-network, and tree state ruled out. The harness does not keep the in-run verifier log; that is the loop's next harness change.
- Second host pause in two cycles (~03:38–04:23); one rollout killed at 143 by the 2,400 s harness timeout, re-run alone.
