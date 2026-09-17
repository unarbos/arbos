---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 22:45 UTC.

## Click Update. You will get build 1662.

**1662** — signed, notarised, stapled, on the feed since 22:41 UTC. It is
the tip of `main`, so the channel is caught up.

It contains everything you were waiting for:

- **#490** — voice lines drawn in the project chat
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
| Bundle | `Info.plist` reads `0.2.0 build 1662` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1662-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## The stall that held it, and the fix

The channel sat on 1657 from 22:19 to 22:41 for the same reason it sat
from 21:37 to 22:19. At 22:29 the publisher said:

> `8d0688d is newer and still running. Its run decides, and comes back
> here whichever way it goes.`

`8d0688d` was that run's **own** triggering commit. Every commit gets two
CI runs about forty seconds apart; the first to finish fires the event,
the second is still going when the publisher asks the API, so it reads
"still deciding" and waits for an event that has already happened. It
published only once a later CI completion came along.

[#517](https://github.com/unarbos/arbos/pull/517) is the fix and is
**green, waiting on the steward**: the commit that started a run takes its
conclusion from the event rather than re-asking, so it can never block. A
green sibling still counts; the answer for that one commit is simply never
"wait".

## Also since the last note

- The transcript-race flake that took `main` red earlier is **fixed on
  `main`** (`87bedb25`), by its owner and better than my version: it waits
  through the shared `common::wait_for` and prints both the transcript and
  the kernel log on failure. My [#507](https://github.com/unarbos/arbos/pull/507)
  stays closed rather than landing a worse duplicate.
- [#510](https://github.com/unarbos/arbos/pull/510) is **merged** — a
  binary that was only just written is briefly unrunnable when another
  thread's fork holds a descriptor to it. That was a product fault, not
  only a test one: the installer runs a binary moments after copying it.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
