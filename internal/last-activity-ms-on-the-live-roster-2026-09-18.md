---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# `last_activity_ms` on the live roster: why it was absent, and where it shows now

Mesh worker, 2026-09-18 02:10 UTC. For the iOS loop and the coordinator.

## What iOS saw was true, and not a hub fault

At 02:05 every project on the live `GET /list` had `last_activity_ms` ABSENT.
Checked against the running process, not the intent: the hub's `MainPID`
executes a file whose md5 equals the `dd7814fc` build made here
(`b397ef93`), and the rows carry `online` from #545, so the hub *is* the
build it is supposed to be.

The field is **kernel-fed**. #538's hub half stores what a kernel reports at
turn start and end (and, on registration, what the kernel's disk knows); the
roster skips the key while the value is zero. So a row shows the field only
when the kernel behind it is built at or past `11a01d84`. At 02:05 not one
ArbosLife kernel was: `demo`/`feedback` on `0f2a8bc6`, `subnet120`/QA/parity
on `30eef166`, `phone` on `efcab58f`, `const` on `cbbe9922` — all older than
#538. The hub had heard nothing to show. (Proved the other way at 01:01: a
kernel built from `c3247332` registered as a scratch machine showed the field
on registration and moved it after one turn.)

## What was done (02:06–02:08 UTC)

The dev channel published build 1741 (`c3247332`, carries #538) at 01:11.
Under the day's rules — idle first, swap first, then every process on the
file, by path never by name, nothing of Jacob's disturbed:

- `~/arbos-hub/bin/arbos-kernel` → `c3247332` by the kernel's own
  `update --install`; then **all three** units on that file restarted
  (`arbos-mesh-worker`, `arbos-mesh-feedback`, `arbos-mesh-kernel`), each back
  in 6 s on the new inode.
- `~/.cargo/bin/arbos-kernel` → `c3247332` the same way; `subnet120` (idle
  since 06:42, no clients, the only process on that file) stopped and
  relaunched detached with its original command line; confirmed with a real
  turn (`ready on computeinstance-u00sd1yvtcwsc091pb`).

Roster at 02:08:13:

| project | `last_activity_ms` | source |
|---|---|---|
| `demo` | 09-17 19:57:01 | disk, on registration |
| `feedback` | 09-16 18:26:56 | disk, on registration |
| `subnet120` | 09-18 02:08:09 | the confirming turn's end, live |
| `phone` | absent | kernel `efcab58f`; **left alone on purpose**: Jacob was using it (a turn 01:58:43–01:58:52, the phone attaching every minute at 02:06) and a restart would have cut him. Update it when idle; it is under `arbos-phone.service`, so that is `update --install` on `~/arbos-hub/phone/bin/arbos-kernel` then one `systemctl --user restart`. |
| `const` | absent | his desktop's own kernel (`cbbe9922`), 21 turns; not mine to restart |
| `qa-cycle-11-demo`, `parity-proj…` | absent | QA's and the parity loop's kernels on `30eef166`; theirs to update (their binaries are their own now: `~/arbos-qa/bin/`, `bin/` relative) |

No process on ArbosLife runs a deleted image; no place is double-served.

## Phone kernel moved too (02:16–02:17 UTC)

Idle check first: no turn since 01:58:52 (18 min), every job exited, the
kernel's only child a headless Chrome from yesterday, the phone app merely
attached (an `attach_open`/`attach_close` a minute). Then swap first —
`update --install` on `~/arbos-hub/phone/bin/arbos-kernel` → `c3247332`, the
phone unit the only process on that file — and `systemctl --user restart
arbos-phone.service`: back in 9 s on the new inode, the phone app re-attached
at 02:17:04 with its asks replayed, the voice gateway logged `kernel ws
closed` at 02:17:01 and `attached to kernel` at 02:17:03, `/healthz` 200.
Roster at 02:17:15: **`phone last_activity_ms = 01:58:52`** (the last turn,
read from disk on registration). Jacob was not in a turn; two seconds of
link, no work touched.

Rows still without the field, and whose they are: `const` (his desktop's
kernel, `cbbe9922`), `qa-cycle-11-demo` (QA's, `30eef166`),
`parity-proj…` (the parity loop's, `30eef166`).

## What the phone should expect

Rows show the field as their kernels move to a #538 build; `phone` will the
next time it is idle for a minute and I am awake. A row without the field is
"the kernel has never told the hub", not "no activity" — the client should
render it as unknown, not as idle-forever.

`v0.2.0` was not published.
