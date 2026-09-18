---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 31 — the quoted-reference test mark on its own example; requests-1766 is a grader artefact

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 31; data `media/swebench/loop/cycle-31/`.

Kernel `arbos-kernel 0.2.0 a8678ac16636 protocol 1`, cut, $19.94.

- **#601 + 5bb0ddec on django-10097, five rollouts (transcript read; the instance is ungradeable here).** Three of five follow the RFC quote (cycles 26/28: two of five). Three write tests; none asserts the quote's `:` clause — all test the issue's literal `/` example. The mark reads as "test the example", not "test the quote".
- **Fresh ten at `-r 2`: 18 of 20.** Both failures are requests-1766 with the gold's one-line change; the gold itself grades 0 here (three unrelated tests fail with network on). Third grader artefact after requests-2317 and django-10097. Cumulative read 301; nothing outside the account.
