# qal-j26: this machine is paused while the agent driving it is idle, so every wall-clock duration is inflated — one click was reported as a 1528-second UI stall that was really the pause

- Measured 2026-09-18 00:03 on `qa-vm2`, kernel `arbos-kernel 0.2.0 d24875af9109`, scenario `desktop-rapid-session-switch` in cycle `20260917T205531Z`, rollout `internal/qa/rollouts/20260917T233752Z-desktop-rapid-session-switch`.
- Class: a rig fault that manufactures product breaks, and the most corrosive kind — it produces a large, specific, plausible number.
- Feature: `desktop_scenarios.Desktop.timed` (which asserts `limit=`), `run.py`'s `duration_s`, and any scenario timing taken with `time.time()`.

## What was reported, and what was true

```
BREAK ui-stall: switch to panel-agent-7 took 1528.7s (limit 3.0s)
```

Everything else in that run was right: 6 sessions, 6 panel rows, **30 switches, 0 failures**, 6 `chat-*`
agent folders. One click out of thirty appeared to take 25 minutes and then completed.

It did not. The stall ran 23:37:54 → 00:03:28, and **it ended at the second the agent resumed from a
2400-second wait**. The same scenario run by hand, before and after, takes 16–20 s.

The machine's own clocks say why:

| reading | at 18:13 | at 00:30 |
|---|---|---|
| wall clock | — | 6.28 h later |
| `uptime` | 2:23 | 6:02 — only **3.65 h** later |

**2.63 hours unaccounted**, and `CLOCK_MONOTONIC` equals `CLOCK_BOOTTIME` exactly, which is the signature
of a VM *paused* rather than a guest suspended: both monotonic clocks freeze with the machine, and only
the wall clock jumps when the host resumes it. `Desktop.timed` measured with `time.time()`, so the pause
landed inside one click's elapsed time.

The loop has known since 2026-09-14 that this VM is suspended while its worker is idle — the finding then
was a two-hour hole in the kernel log and the recorded frames. What was not drawn out is the consequence
for **assertions on duration**: they do not go quiet, they go loud and wrong.

## The fix

`time.monotonic()` for every measured interval — it stops with the machine, so it measures the work and
not the pause. Changed: `Desktop.timed` (the one that asserts a limit), `Desktop`'s `launch_s`, and
`run.py`'s `duration_s`. Verified after the change: the same scenario passes in 19.6 s with 30 switches
and 0 failures.

## What it costs backwards

Any duration this machine reported while its agent was idle is suspect, and two already read oddly:

- cycle 3's `sq-02-desktop-stop-holds-follow-up` at **9142.7 s** (2.5 h) — its `BlockingIOError` on the
  store-mounted driver was real (`qal-j23`), but that duration is mostly pause.
- `desktop-rapid-session-switch` at **1544.9 s** in cycle 4 — the whole excess.

Neither number should be cited. Durations recorded from now on are monotonic and mean what they say.

## The rule this earns

Beside "record the commit and the build a measurement was taken on": **record durations on a clock that
stops when the machine does, and treat any wall-clock interval measured across an idle agent as
unmeasured.** A limit assertion on a wall clock, on a machine that can be paused, is an assertion bounding
something the code under test does not control — the sixth review step ("an assertion must not bound a
race") arriving from the infrastructure rather than from the product.
