---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-17 21:42 UTC.

## Click Update. You will get build 1624.

**1624** — signed, notarised, stapled, on the feed since 21:37 UTC. It is
the tip of `main`, so the channel is fully caught up.

It carries everything 1616 had, plus **#499**, **#500** and **#501**:

- **#500** — GPT-Live sees the project, the on-screen chat and live workers
- **#501** — Live voice rows stay in the attached project's chat
- **#499** — a store is a folder, not a path; a moved place is never
  recreated at its old path

Voice lines in the project chat came in at 1616 (#490) and are still here.

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1624` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1624-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## Why 1622 never appeared

`c40f39b0` (build 1622) went green on its second try only. Its first CI
run failed on `stop_keeps_follow_up_e2e`, a timing flake rather than a
break, and a red tip means no publish. The steward re-ran it; by then
`main` had moved to `d9e5ccde`, so 1624 is what came out and it contains
1622's work.

Two things are in flight to stop this shape recurring:

- [#504](https://github.com/unarbos/arbos/pull/504) — **now fully green**,
  waiting on the steward. A green build that steps aside for a newer
  commit is picked up again when that commit fails, instead of the channel
  sitting still.
- [#507](https://github.com/unarbos/arbos/pull/507) — the flake itself.
  The test read the transcript once, the instant the reply frame arrived,
  while the file is written as the turn runs. It now polls to a bound —
  and if the line is genuinely missing it still fails, printing the
  transcript, because "wait longer" must not hide a real fold.

## How to check for yourself

The bar reads this feed:

    https://github.com/unarbos/arbos/releases/download/dev/arbos-dev.json

The newest entry's `build` is what the Update button will offer.
