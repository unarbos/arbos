---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 20:58 UTC.

## Click Update. You will get build 1616.

**1616** — signed, notarised, stapled, on the feed since 20:53 UTC.

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

## Why it took from 20:27 to 20:53

Worth knowing, because for half an hour it looked like nothing was
happening when CI was in fact green.

| build | commit | what happened |
|---|---|---|
| 1609 | `0922559d` (#490) | CI **passed** at 20:27. Publish deferred to the newer commit, by design. |
| 1611 | `862eaf26` (#493) | CI **failed**. Nothing published — and 1609 had already stepped aside for it. |
| 1616 | `aaf3dbe9` | CI passed at 20:49, published 20:53. Contains #490. **This is the one.** |

So a green build carrying the change you wanted gave way to a newer one
that then went red, and the channel sat still until the next green commit
came along. That is a gap in the publish rules rather than bad luck: a
commit that defers has no way back if the one it deferred to fails. Worth
fixing so a green build is never stranded behind a red one.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
