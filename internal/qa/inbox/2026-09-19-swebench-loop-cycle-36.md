---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 36 — ten more fresh instances; `changes` blocked 51 minutes re-running a server

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 36; data `media/swebench/loop/cycle-36/`.

Kernel `arbos-kernel 0.2.0 a8678ac16636 protocol 1`, Jev off, cut, $14.55, 16 of 20 (a count). Four failures, all inside the account: django-14170 ×2 (existing tests pin the `BETWEEN` mechanism the issue says to drop), sphinx-7985 ×2 (output line count; a working local link must go silent). Cumulative read 314; nothing outside the account on eighty fresh instances.

A QA case worth reproducing, from a rollout that solved (django-13809 `dab2e1ba`, bundle in the cycle's data): the agent's reproductions were `runserver` commands that never exit; the gate recorded seventeen of them; `changes` re-ran each under `timeout 180`, serially, with no total cap — 3,086 s and 3,070 s for two `changes` calls, the kernel's own notice saying "nothing has happened for 46m: waiting on `changes`". `arbos-kernel run --timeout 2400` did not end the run (7,749 s). Recorded as an observation; not filed.
