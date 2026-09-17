---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 23:33 UTC.

## Click Update. You will get build 1688.

**1688** — signed, notarised, stapled, on the feed since 23:27 UTC.

New since 1674:

- **#519** — `ps -ww` for the argv proof of a pid, so BSD `ps` does not cut
  the job folder's path at 79 columns
- **#520** — a folder you cannot write is refused with the situation and
  what to do, not "cannot lock"
- **#521** — the wait for the checkpoint tree is bounded at 20 s
- **#522** — the kernel removes only its own lock

Still in it, from earlier builds: **#490** voice lines in the project chat,
**#500** GPT-Live sees the project and live workers, **#501** Live voice
rows stay in the attached project's chat, **#499** a store is a folder,
**#518** a kernel without git says so.

**#469 (feedback over ssh) is not in this one.** It landed as build 1696,
after 1688 was cut. It arrives on the next publish.

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` — the build log records `0.2.0 (1688, 21030b971862) signed by` it |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1688` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1688-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## Why the channel runs a little behind `main`

`main` is on build 1701; the feed is on 1688. That gap is the design
working, not a fault.

The publisher waits when a commit newer than the one it is about is still
being tested, so that a runner and an Apple notarisation are not spent on
a build that is already superseded. When `main` merges faster than CI
finishes, there is almost always such a commit, so the channel publishes
whenever it catches a settled tip rather than on every merge.

Nothing is stuck. A gap that keeps closing is the rule doing its job; a
gap that stops closing would be worth a look.

## Correction to an earlier note

An earlier version of this file said the channel had been held by a run
"waiting on its own triggering commit". That was wrong.

I read the triggering commit off `gh run list --json headSha`, which for a
`workflow_run` reports the **branch tip**, not the commit whose run fired
the event. Reading the real value out of the job showed each wait was on a
genuinely newer commit that was genuinely still running — correct
behaviour every time.

[#517](https://github.com/unarbos/arbos/pull/517) is merged and does no
harm — taking the triggering commit's conclusion from the event is more
accurate than re-asking it — but it fixed something that was not
happening.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
