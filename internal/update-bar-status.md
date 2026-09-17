---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 23:12 UTC.

## Click Update. You will get build 1674.

**1674** — signed, notarised, stapled, on the feed since 23:07 UTC.

It carries everything 1662 had, plus **#518**: a kernel without git says so
once on root's transcript, carries `git_missing` in `kernel.json`, and says
when git is back.

Still in it, from earlier builds:

- **#490** — voice lines drawn in the project chat
- **#500** — GPT-Live sees the project, the on-screen chat and live workers
- **#501** — Live voice rows stay in the attached project's chat
- **#499** — a store is a folder, not a path

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves`, the kernel inside the bundle and then the bundle |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1674` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1674-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## Why the channel runs a little behind `main`

`main` is on build 1680; the feed is on 1674. That gap is the design
working, not a fault, and my earlier note in this file said otherwise —
see below.

The publisher waits when a commit newer than the one it is about is still
being tested, so that a runner and an Apple notarisation are not spent on
a build that is already superseded. When `main` merges faster than CI
finishes, there is almost always such a commit, so the channel publishes
whenever it catches a settled tip rather than on every merge. A quarter of
an hour behind is normal at this pace.

Nothing is stuck. If the gap ever stops closing, that is worth a look; a
gap that keeps closing is the rule doing its job.

## Correction to the previous note

The previous version of this file said the channel had been held by a run
"waiting on its own triggering commit". That was wrong, and I should say
so plainly because it is the kind of claim that sends someone looking in
the wrong place.

I read the triggering commit off `gh run list --json headSha`, which for a
`workflow_run` reports the **branch tip**, not the commit whose run fired
the event. Reading the actual value out of the job showed the run was
about `86fc0bc6` and waiting on `8d0688d` — a genuinely newer commit that
was genuinely still running. Correct behaviour, twice.

[#517](https://github.com/unarbos/arbos/pull/517) is merged and does no
harm — taking the triggering commit's conclusion from the event is more
accurate than re-asking, and the event is the authority for that run — but
it fixed something that was not happening.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
