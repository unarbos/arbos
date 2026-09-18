---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-18 01:14 UTC.

## Click Update. You will get build 1741.

**1741** — signed, notarised, stapled, on the feed since 01:11 UTC.

New since 1733:

- **#537** — iOS Settings stands on the app's own ground
- **#538** — per-project `last_activity_ms` on the hub's roster
- **#539** — a loop scenario for the four rows that had gone longest
  unchecked
- **#540** — a failed Stop records the kernel's side of the moment

Still in it: **#531** a worker's report is captioned; **#469** feedback
over ssh for a tab that never attached; **#527** unreachable checkpoint
refs dropped, so `.git` stops holding every turn's tree; **#530** no ghost
`.arbos/` at a renamed path; and from earlier **#490** voice lines in the
project chat, **#500**/**#501** GPT-Live and voice rows, **#499** a store
is a folder, **#518**–**#522**.

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` — the build records `0.2.0 (1741, c3247332dc4e) signed by` it |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1741` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1741-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## How far behind the channel runs, and why

The channel is a sawtooth, not a queue. Through the evening it published
1616, 1624, 1657, 1662, 1674, 1688, 1718, 1733, 1741 — every twenty to
forty minutes,
catching up in a jump each time.

The publisher declines to build a commit while a newer one is still being
tested, so that a runner and an Apple notarisation are not spent on a
build that is already superseded. When `main` merges faster than CI
finishes there is nearly always such a commit, so it publishes when
merging pauses long enough for a tip to settle — Both 1718 and 1741 went out
because `main` paused for about a quarter of an hour and a tip settled;
1733 followed a thirty-minute wait in which four runs each deferred to a
commit still under test.

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
