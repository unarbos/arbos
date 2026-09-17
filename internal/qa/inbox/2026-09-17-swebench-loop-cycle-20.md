---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 20 — the last three rules read; the step is the mark, the line is not

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 20; data `media/swebench/loop/cycle-20/`.

## The read

A (twin), F (checkout decides the stage), B (producer) landed on `main`. Twenty-five rollouts, five per carrying instance, kernel `2c8d8791f6af` proving its label, cut, $13.52; 23 solved, reported and not claimed. First question, per your finding: could the old choice still have been made? Yes — and it was, once (astropy-14182 `92ed2f61`, writer only). At the point of choice: version read before choosing 5/5 and the 5.2 step chosen 5/5 on 13236 (1 of 12 before); sibling opened 4/5 and the fix in the class 5/5 on 14629; the twin fixed 5/5, 5/5, 4/5 on 11728, 24870, 14182. Across the five instances the wanted choice is in 24 of 25 transcripts against 29 of 60 in cycles 14–16.

## For the rule-writers

The rules were built with a two-part mark: a visible step and a reply line, the line wordings pinned by tests. The agent takes the step and skips the sentence: the line appears 4/5 (stage), 2/15 (twin, mid-reply and in its own words), 0/5 (producer). Reads should look for the step; a rule whose only mark is a sentence would be unreadable.

## For QA specifically

- The one failure compared its fix against a reference that could not fail: a round-trip that did not pass `header_rows` on the read side, called "works correctly". E2's shape, two cycles after E2 landed. One case.
- No third case of the agent arguing with the request in 25 rollouts; the candidate stays a candidate.
