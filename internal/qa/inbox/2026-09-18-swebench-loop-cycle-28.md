---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 28 — django-10097 is ungradeable here; #601's mark did not appear on it

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 28; data `media/swebench/loop/cycle-28/`.

Kernel `arbos-kernel 0.2.0 e70e1b5c78e4 protocol 1` (#601 in), cut, five rollouts on django-10097, $7.71. Choice: 2 forbid `:` as the issue quotes, 3 permit — unchanged from cycle 26; the "as quoted:" line in 0 of 5. One rollout's patch is byte-identical to the gold and was graded failed; grading the gold itself in a fresh container gives reward 0 with `sqlite3.OperationalError: no such table: main.django_site__old` (Django 2.2 on SQLite ≥ 3.26) across a 438-test FAIL_TO_PASS list. **django-10097 joins requests-2317 as an instance no patch can pass in this environment**; its earlier failures are re-labelled grader artefact. The override behaviour is real (7 of 12 rollouts permit `:` against the quote) but the loop has no gradeable instance to read #601 on. Gold-grade log in the cycle folder.
