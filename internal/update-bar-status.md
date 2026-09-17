---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 22:26 UTC.

## Click Update. You will get build 1657.

**1657** — signed, notarised, stapled, on the feed since 22:19 UTC. It is
the tip of `main`.

It carries everything 1624 had, and 1624's contents are still in it:

- **#490** — voice lines drawn in the project chat (first in 1616)
- **#500** — GPT-Live sees the project, the on-screen chat and live workers
- **#501** — Live voice rows stay in the attached project's chat
- **#499** — a store is a folder, not a path

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1657` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1657-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## The channel sat still between 21:37 and 22:19

Worth recording, because it looked like a fault and mostly was not.

`main` merged eleven builds in that window — 1624 to 1657 — faster than CI
could finish. Each commit gets two CI runs about forty seconds apart, so
when the publisher asked "is anything newer still deciding" the answer was
almost always yes, and it waited. It published the moment it caught a tip
whose CI had settled.

That is not new behaviour: the old rule skipped a run whose commit was no
longer the tip, which under the same churn published just as rarely. But
it is now visible, and one part of it is mine to tighten — a run can wait
on the very commit whose CI completion triggered it, which is a wait for
an event that has already happened. That is being fixed.

The good news from the same window: [#504](https://github.com/unarbos/arbos/pull/504)
is live and the first-parent walk is what published 1657, and the two-tier
retention is visibly working — ten releases kept, seven of them kernel-only.

Two flake fixes are in review, both of which cost the channel time today:

- [#507](https://github.com/unarbos/arbos/pull/507) — a test read the
  transcript once, the instant the reply arrived, while the file is
  written as the turn runs.
- [#510](https://github.com/unarbos/arbos/pull/510) — a binary that was
  only just written is briefly unrunnable (`ETXTBSY`) when another
  thread's fork holds a descriptor to it. Retried, and only for that
  error.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
