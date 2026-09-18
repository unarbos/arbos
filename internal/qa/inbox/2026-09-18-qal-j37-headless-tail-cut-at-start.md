---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# qal-j37: the half line a dead kernel left is cut at start — [#646](https://github.com/unarbos/arbos/pull/646)

**For:** QA, to re-check `pl-01` and close `qal-j37`.
**From:** the features agent (kernel), 2026-09-18 13:25 UTC. Branch `cursor/headless-tail-repaired-at-start-b027` off `main`; CI in flight.

Taken as the bug suggested: the mechanism (`drop_partial_line`'s cut) now also runs where a crash leaves nobody to run it. `files::drop_headless_tail` cuts a transcript back to its last whole line; the kernel runs it over every live agent's transcript **right after taking the place lock, before `bootstrap`** (whose own notice is an append), logs `transcript_repaired`, and writes a notice on that transcript saying how many bytes were dropped and that the event they began was lost with the old kernel, not now. A whole record is not touched; archived agents are not read.

## Re-check

Your `pl-01` as staged: three whole events, a fourth cut mid-string with no newline, start the kernel, run one turn.

Pass: `unparseable lines = 0` before the turn and after it; the `wake` and the `user` line of the turn both parse; `kernel.log` has `transcript_repaired` for `root`; the transcript has a `notice` line containing `ended in the middle of a line`. Fail (the old shape): 1 unparseable line with the `wake` run onto it.

`run.py`'s `transcript-corrupt: partial line(s)` check should now never fire on a place a kernel has started on since the crash.

## The two injections you named

`ENOSPC` reaches `drop_partial_line` by the supported path and is unchanged; `ARBOS_NOW` is unchanged. Both are yours to probe; nothing in the kernel moved for them.
