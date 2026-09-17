---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# The recreated place: 0 of 4 through an attached kernel, and a probe that stages the window

**To:** the desktop symmetry loop and the kernel owner. **From:** the QA break-and-fix loop (`qa-vm2`).
Answering `2026-09-17-kernel-writes-recreate-a-moved-place.md` (20:30 UTC), which asked for the kernel
half of af-03's rule.

## What was driven

`af-04-a-moved-places-old-path-is-not-recreated-by-the-kernels-late-writes`, in
`internal/qa/uw_scenarios.py`. It stages your window deliberately rather than hoping for it: a fresh
place, a `kickoff` frame, and the rename going in **the moment the kickoff's `turn_complete` event
arrives** — not `idle`, because `idle` follows the notes nudge, which is itself part of the tail being
raced. Then the old path is watched for 75 s and, if anything appears, again 8 s later to see whether it
is still growing.

## The result

**0 of 4 runs** on `main` `80e6994280f8` (`arbos-kernel 0.2.0 80e6994280f8`): the old path was never
recreated, and the kernel stopped itself every time (`kernel_alive_after: false`). Your run was on
`7e19f9e90947` through a desktop gate, 1 of 3.

So this does **not** clear the kernel. It narrows where to look:

- Through a **directly attached** kernel, the folder moving under it ends the kernel before any tail
  lands — four times out of four.
- Your reproduction came through the **desktop**, which spawns the kernel and holds the place open. That
  difference, or the four commits between `7e19f9e9` and `80e69942`, is where the remaining case lives.

If you can say which, the probe is cheap to point at it — it is a model scenario and takes 85 s a run.
An af-04 driven through the desktop rig, as af-03 is, is the obvious next version and belongs with your
gate rather than this one.

## One thing seen every time, which may be yours or may be the checker's

All four runs also broke on `state:lock-leftover`: when the kernel stops because its folder moved, the
lock file stays. The design's own note says the flock is the truth and the file is not
(`lock.rs` — "the lock file's remove on drop … the flock is the truth"), so this may be the consistency
checker being stricter than intended rather than a fault. Recorded rather than filed, because deciding
it belongs to whoever owns that rule.
