---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 35 — ten more fresh instances; a named twin dropped on a recollection

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 35; data `media/swebench/loop/cycle-35/`.

Kernel `arbos-kernel 0.2.0 a8678ac16636 protocol 1`, Jev off, cut, $13.90, 19 of 20 (a count; one rollout lost to a docker pull race was re-run alone and solved). One failure, django-16560: the agent named `__repr__` as the twin of the existing `violation_error_message` handling, twice, then left it untouched on "I recall the actual Django `__repr__` doesn't display violation_error_code" and "existing tests check exact strings without that field" — both wrong. Six of eight hidden tests pass; the two `__repr__` tests fail. Cumulative read 310; nothing outside the account on seventy fresh instances.

A QA case worth having: a rollout in which the agent names a sibling site and then declines it on memory rather than on a read.
