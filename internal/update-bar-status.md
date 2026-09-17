---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 21:12 UTC.

## Click Update. You will get build 1616.

**1616** — signed, notarised, stapled, on the feed since 20:53 UTC. It is
also the tip of `main`, so the channel is fully caught up.

**It draws voice lines in the project chat.** #490 (`0922559d`) is in it,
so the call appears in the chat rather than only being audible. You are on
1588, so this is twenty-eight builds forward.

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1616` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1616-macos-arm64.zip` as
downloaded, not just from the build log, so it is the copy you will get.

## Why it took from 20:27 to 20:53, and what has been done about it

| build | commit | what happened |
|---|---|---|
| 1609 | `0922559d` (#490) | CI **passed** at 20:27. Publish stepped aside for the newer commit, by design. |
| 1611 | `862eaf26` (#493) | CI **failed**. Nothing published — and 1609 had already given up its turn. |
| 1616 | `aaf3dbe9` | CI passed at 20:49, published 20:53. Contains #490. |

A green build carrying the change you wanted gave way to a newer one that
then went red, and the channel sat still until the next green commit came
along. Stepping aside is only safe if somebody is actually going to go.

Fixed in [#504](https://github.com/unarbos/arbos/pull/504), waiting on
review: a failed CI run now brings the publisher back, and instead of
asking "was I superseded" it asks "what is the newest commit on `main`
worth publishing" — so a green build that stood aside is picked up again
when the commit it deferred to fails. Seven cases are checked by
`.github/dev-channel-plan-test.sh`, including this one.

Until that merges, the same stall can happen again: if a green build
defers to a commit whose CI then fails, the channel waits for the next
green merge rather than going back. It comes right on its own, just not
promptly.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
