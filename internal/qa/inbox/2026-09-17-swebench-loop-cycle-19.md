---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 19 — first read after two rules landed; the patterns hold out of sample

PR: [#477](https://github.com/unarbos/arbos/pull/477) (kernel proves its label; open). Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 19; data `media/swebench/loop/cycle-19/`.

## The read

#449 landed E1 and E2. Five rollouts each on the carrying instances, kernel `3f114bf50447` (main + #477 plumbing; proves its label; `kernel-identity.json` in every artifact), cut, $7.38. First question, per your afternoon's finding: could the old choice still have been made? Yes in both — four of five sympy-15017 rollouts met the second assertion, all five pylint-6386 rollouts had `--verbose` as a reference. At that point: 15017 kept the root fix 3 of 4 (one verbatim in the rule's words), retreated once — after asking the rule's question and answering it with numpy's semantics; 6386 compared `-v` with `--verbose` after the fix 4 of 5 and fixed the routing 4 of 5; the one that did not had seen `--verbose` before the fix and compared `--verbose` variants with each other afterwards. 8/10 solved is reported and means nothing at n=5.

## Out of sample

The cycle-18 completeness claim tested on 24 non-fetching failures from the cycle 1–5 slices (older kernels, different instances): twin 8, producer 6, maintainers' choice 9, "argues with the request" 1, unclassified 0. Same shape as in sample.

## For QA specifically

- "The agent argues with the request" is now seen twice (declined a ticket on upstream history; permitted `:` in a URL password against the issue's RFC quote). Named as a candidate; a third case makes it a pattern.
- One 15017 rollout solved with no `edit`/`write` tool call — it edited through `sed`. The kernel's edit-path instrumentation (mechanism line, coverage hook) does not see such edits. Worth knowing when transcript reads count edits.
