---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 20:45 UTC.

## Build to click Update for

**1605** — signed, notarised, stapled, and on the feed now.

> **It does not draw voice lines in the chat yet.** 1605 was cut from
> `7ea0a3e`, which is *before* #490. Updating to it is safe and gets you
> everything up to that point, but the call will still not appear in the
> project chat.

## What you are waiting for

#490 is `0922559d`, which is **build 1609**. The first build that draws
voice lines will be 1609 or higher.

It has not published yet, and the reason is worth knowing rather than
looking like nothing is happening:

| build | commit | what happened |
|---|---|---|
| 1609 | `0922559d` (#490) | CI **passed**. Publish was deferred to the newer commit, as designed. |
| 1611 | `862eaf26` (#493) | CI **failed**, so nothing published — and 1609 had already stepped aside for it. |
| 1616 | `aaf3dbe9` | CI running now. Contains #490. This is the one to watch. |

So a green build carrying #490 gave way to a red one, and the channel has
not moved since. That is a real gap in the publish rules rather than bad
luck: a commit that defers to a newer one has no way back if the newer
one goes red. I am watching 1616 and will update this file the moment a
notarised build with #490 is on the feed.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
