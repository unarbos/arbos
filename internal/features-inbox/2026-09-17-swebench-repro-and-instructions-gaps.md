---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# From the SWE-bench loop: two gaps the cycle-11 smoke exposed

For the features agent. Jacob routed both here (2026-09-17). Evidence bundle: `media/swebench/loop/cycle-11/swe-bench_django__django-11099--1ebfe44b.tgz` (the smoke rollout; both gaps are visible in its `transcript.jsonl`).

## 1. Any failing bash command becomes "the reproduction" (kernel)

`crates/arbos-engine/src/repro.rs`, `note_failing`: every bash command that exits non-zero before the first edit is written as the candidate reproduction, and the gate takes the last one when the agent edits without `repro:true`. This was cycle 5's fix for the agent running the failing snippet without the flag (46 of 146 refusals) and then wandering.

What happened: the agent was told to run `pip download ... django==3.2` first. The network was cut, pip exited non-zero, and that command became reproduction 1. The agent then reproduced the real bug (a Python snippet, exit non-zero too) — but the first edit took the *last* failing command, which by then was the real one, so this rollout got lucky. `changes` later reported "Reproductions (1 recorded before the fix): all pass now. 1. pass — pip download ..." in another ordering of events. The recorded reproduction has no relation to the bug; a passing `pip download` is not evidence the fix works.

What would fix it, for you to judge:
- Only take a failing command as the reproduction when it runs something from the repository (invokes `python`/`pytest`/the project's test runner, or a path under the place), not package managers, `find`, `grep`, `ls`, network tools. A short deny-list of command heads (`pip`, `apt`, `curl`, `wget`, `git`, `find`, `grep`, `ls`, `cat`) would have caught this one.
- Or: keep taking it, but have `changes` say what the reproduction *is* before saying it passes, so a reviewer sees "pass — pip download ..." for what it is.

## 2. `ARBOS_INSTRUCTIONS` replaced the headless rules (harness, fixed on the branch)

`harness/arbos_harness/arbos-swe-run` wrote `instructions.md` from `ARBOS_INSTRUCTIONS` *instead of* the standing headless rules (never ask, do not commit or branch, no network, run the tests). One instruction in, and "do not commit" was gone: the agent committed on a branch, `git diff` against the base saw nothing, the patch was empty, graded failed.

Fixed in the harness on `cursor/swebench-loop-c12-7c9c` (`21067cc5`): the rules are always written and the override is appended under "For this run also:". For you: the kernel's own `instructions.md` is one file with no notion of a standing part and a per-run part. If any other caller writes it (desktop, mobile, `serve`), the same shape — a caller's text silently replacing rules everyone assumes are in force — is possible there; worth a look.
