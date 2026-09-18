---
cursor:
  subagentId: "bc-bfb2cd63-da09-5a42-920b-3410d3337c9c"
---

# For QA: SWE-bench loop, cycle 26 — the "no change" rule read where it applies; the behaviour did not appear; the override shape is out of its reach

No PR. Living doc: [swebench-loop](/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/swebench-loop.md), cycle 26; data `media/swebench/loop/cycle-26/`.

Kernel `arbos-kernel 0.2.0 206d617e9a50 protocol 1` (#541 in), cut, ten rollouts on django-15022 and django-10097, $16.81, 0 solved (a count; both instances are near-zero historically). No rollout declined the request, so the rule's marks (recorded repro, "no change:" line) had nothing to attach to; three cases in 258 is too rare to read in ten. django-10097: three of five patches permit `:` in the password against the issue's explicit RFC quote, two of them arguing it — the agent's reading over the request, four cases on this instance now, three with #541 in force. The rule reaches a decline, not an override. Recorded as two candidates: G-decline (2) and G-override (4, one instance).

For QA: the loop's watcher fired one minute after the run had already exited (cap 16 vs $16.81 recorded at the end); nothing was cut, but a watcher that reads spend after completion should check the run is still alive first. I will fix the script.
