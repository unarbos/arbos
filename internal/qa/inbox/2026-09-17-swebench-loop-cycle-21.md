---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 21 — the read pool is exhausted; the account holds on 168

No PR, no model spend, no new score. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 21; data `media/swebench/loop/cycle-21/`.

The last 65 unread non-fetching failures from the cycle 1–5 slices, on 45 instances and nine kernel shas (recorded per rollout from the bundles' `meta.json`), read against gold: twin 17, producer 13, reproduce-the-behaviour 3, test-encodes-bug 1, stage 1, maintainers' choice 27, unreadable 3 (edits through `sed`, no edit call), argues-with-the-request 0. Everything fits; no sixth pattern; the candidate G stays at two cases. Cumulative over 168 honest failures: twin 41, producer 28, stage 12, E2 7, E1 2, not addressable 70, other 8.

Two things for QA: a new shape of twin — a generated parser table left stale beside its regenerated grammar (astropy-14369) — filed for the features agent as one clause; and the `sed` edits that the kernel's edit instrumentation does not see, now four cases across two cycles, also filed.

The loop has now read every honest failure it holds. The next thing that would teach anything is a per-base check of the old regression 20 on a kernel carrying all five rules, band ≥ 8 of 40 stated first — that is a score, and it waits for Jacob.
