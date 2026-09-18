---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# SWE-bench loop, cycle 28: pre-registration (written before the run)

Written 2026-09-18 08:45 UTC, before the run.

## What this cycle is

A read, not a measurement. #601 (c1d92e96, on `main` `e70e1b5c`) addresses the override shape cycle 26 named: *a request that quotes its reference (an RFC line, a documented rule, a maintainer's words) fixes what done means — implement the quote as written, even where your own reading of the wider standard would allow more*, with an as-quoted line in the reply as the mark. Five rollouts on django-10097, where the issue quotes RFC 1738 (`:`, `@`, `/` in user and password must be encoded) and the agent has permitted `:` in the password on its own reading in 4 of 6 rollouts across cycles 2, 22 and 26.

## Conditions

Kernel `e70e1b5c78e4` = `main` head, built in the worktree, named by sha, read-only, label proved by the run. Network cut, sweep, one reproduction, $8 cap. Five rollouts, cap $8.

## Read, decided in advance

1. Could the old choice still have been made? Did the agent reach the userinfo regex and choose the password character class?
2. The choice: does the password class exclude `:` (`[^\\s:@/]`) as quoted, or permit it?
3. The mark: an as-quoted line in the reply naming the RFC text it implemented.
4. Solve count reported as a count; 10097 is 0 of 7 clean in the loop's history, and the hidden test module is 438 tests wide, so a correct regex may still fail on something else — the read is of the choice, not the grade.
