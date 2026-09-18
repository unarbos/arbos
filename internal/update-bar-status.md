---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-18 02:30 UTC.

## Click Update. You will get build 1765.

**1765** — signed, notarised, stapled, on the feed since 02:26 UTC.

New since 1741, ten merges' worth:

- **#541** — "no change needed" takes a recorded repro run and a note
- **#542** — macOS Enter Full Screen is a native Space; **#544** — Project
  panel matches Terminal's width, default closed
- **#543**, **#547** — iOS: the list says what the hub said when a token
  is refused; leaving a call you never started no longer kills the app
- **#545** — a machine whose last registrant left stays on the roster
- **#546** — Jev routes mechanical steps in the engine
- **#548** — a window stops a job through the kernel's `job_stop` frame

Still in it: **#537**–**#540**, **#531**, **#469**, **#527**, **#530**,
and from earlier **#490** voice lines in the project chat, **#500**/**#501**
GPT-Live and voice rows, **#499** a store is a folder, **#518**–**#522**.

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` — the build records `0.2.0 (1765, 9bb49c61d844) signed by` it |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1765` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1765-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## How far behind the channel runs, and why

The channel is a sawtooth, not a queue. Through the evening it published
1616, 1624, 1657, 1662, 1674, 1688, 1718, 1733, 1741, 1765 — every twenty
minutes to an hour,
catching up in a jump each time.

The publisher declines to build a commit while a newer one is still being
tested, so that a runner and an Apple notarisation are not spent on a
build that is already superseded. When `main` merges faster than CI
finishes there is nearly always such a commit, so it publishes when
merging pauses long enough for a tip to settle — Both 1718 and 1741 went out
because `main` paused for about a quarter of an hour and a tip settled.
1765 took the longest yet — sixty-six minutes, ten merges, and two
commits whose CI failed on the way (`58105882`, `dd7814fc`) before
`9bb49c61` went green and the deferral chain resolved.

Nothing is stuck, and every wait checked this evening was on a genuinely
newer commit genuinely still running. But the latency is real: twenty to
forty minutes, and up to thirty builds, between a merge and a clickable
build.

**If that is too slow, the lever is that wait.** Publishing the newest
green commit without waiting for pending newer ones would track `main`
closely, at the cost of a macOS build and an Apple notarisation per green
merge, and of the feed briefly offering a build that is one behind. That
is a trade worth deciding deliberately rather than drifting into.

## Correction to an earlier note

An earlier version of this file blamed a stall on a run "waiting on its
own triggering commit". That was wrong. I had read the trigger off
`gh run list --json headSha`, which for a `workflow_run` reports the
**branch tip** rather than the commit whose run fired the event. Every
wait, re-checked against the real value, was on a genuinely newer commit.

[#517](https://github.com/unarbos/arbos/pull/517) is merged and harmless —
taking that commit's conclusion from the event is more accurate than
re-asking — but it fixed something that was not happening.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
