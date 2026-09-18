---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-18 08:46 UTC.

## Click Update. You will get build 1910.

**1910** — signed, notarised, stapled, on the feed since 08:44 UTC.

Checked, not assumed: Apple's notary said Accepted, the ticket is stapled
into the bundle, Gatekeeper reads it as a Notarized Developer ID, the zip's
own Ed25519 signature verifies against the key in the app, and its size and
hash are the ones the feed states. All four payloads (Mac app and kernel,
Linux app and kernel) are on the release.

New since 1904 (builds 1905–1910):

- **#600** — the loop reaches a worker's chat from the sheet, and
  `page_back` walks toward older lines
- **#601** — a request that quotes its reference fixes what done means:
  the as-quoted line in the reply, never a looser check

And new since 1886 (builds 1887–1904, which 1910 also carries):

- **#594** — voice: one question, one answer. The model's first word waits
  for our transcript; a work question is the kernel's, small talk the
  model's
- **#593** — the loop pages a list until it stops growing, and refuses to
  count one whose labels repeat
- **#595** — the loop reads the settings sheet where its contents actually
  are
- **#596** — dev channel: a newer commit holds the channel only while it
  looks like finishing
- **#598** — the loop tags this run's workers so the sheet can be asked
  about them
- **#599** — iOS: the composer's send and microphone say their own names,
  and no script taps a symbol name any more

And new since 1875 (builds 1876–1886):

- **#589** — the checkpoint_refs test says what the record and the repo
  held when it goes red
- **#590** — the iOS call screen tells VoiceOver what it is doing
- **#591** — a notice that opens a turn is drawn once
- **#592** — re-check of the pause split

And new since 1833 (builds 1834–1875). The two worth naming:

- **#575** (build 1843) — the installer no longer loses its atomic swap to
  a passing error, so a reader can never catch the app's path missing
- **#573** (build 1848)

And from 1833 itself:

- **#570** — the producer rule's step is reading the producer before the
  first edit

And new since 1805:

- **#563** — the tail cursors stand at each record's end at boot
- **#566**, **#571** — iOS: say why the link is down in the phone's own
  words; the round buttons say what they are for
- **#567** — a stopped turn books what its completed steps cost
- **#568** — the git guard takes its base from `[git]` in `project.toml`
- **#569** — the mode is the tree's: a child takes its parent's

And new since 1799:

- **#561** — a loop scenario for the dictation promise
- **#564** — a GitHub poll stays in flight until its subscription is done
- **#565** — an attached file is a token in the sentence

And new since 1789:

- **#558** — `save` frame: a person's editor saves a project file
- **#559** — browser screencast to a window
- **#560** — a recorded line the pane already holds is held whatever happens
- **#562** — voice: a pause mid-question is one question (the join window)

And new since 1784:

- **#555** — `claim` frame: a path a person is editing, held in a client
- **#557** — iOS: a slow first connect keeps the words already spoken

And new since 1768:

- **#549** — the desktop's turn-end probe reads archived transcripts
- **#550** — `turn_changes` frame: each turn's files from the rewind
  checkpoint
- **#552**, **#553**, **#554** — loop and restart-state fixes
- **#556** — iOS: a sleeping machine reads as asleep, not as Off

**Jev is on in this build.** Absent from config it turns itself on when
the provider is OpenRouter and a key is present, which is your setup;
`jev = false` in `config.toml` keeps the old one-model loop. An empty
`jev_model` does the same, and `jev = true` forces it on for any provider
with a key.

Also in it, from 1765 and 1768: **#551** loop measuring tools, **#541**–
**#548**, **#537**–**#540**, **#531**, **#469**, **#527**, **#530**, and
from earlier **#490** voice lines in the project chat, **#500**/**#501**
GPT-Live and voice rows, **#499** a store is a folder, **#518**–**#522**.

## What was checked

| | |
|---|---|
| Signed | `Developer ID Application: Jacob Steeves` — the build records `0.2.0 (1886, 17c5232e2189) signed by` it |
| Notarised | Apple returned `status: Accepted` |
| Stapled | ticket present in the downloaded zip — 1674 bytes, signed by Apple System Integration CA for "Software Ticket Signing" |
| Gatekeeper | `source=Notarized Developer ID` |
| Bundle | `Info.plist` reads `0.2.0 build 1886` |
| Feed | every download it names is really on the tag |

The ticket was read out of `Arbos-0.2.0-1886-macos-arm64.zip` as
downloaded, not from the build log, so it is the copy you will get.

## How far behind the channel runs, and why

The channel is a sawtooth, not a queue. Through the evening it published
1616, 1624, 1657, 1662, 1674, 1688, 1718, 1733, 1741, 1765, 1768, 1784, 1789,
1799, 1805, 1831, 1833, 1875, 1886 — every few minutes to eighty
more,
catching up in a jump each time.

The publisher declines to build a commit while a newer one is still being
tested, so that a runner and an Apple notarisation are not spent on a
build that is already superseded. When `main` merges faster than CI
finishes there is nearly always such a commit, so it publishes when
merging pauses long enough for a tip to settle — Both 1718 and 1741 went out
because `main` paused for about a quarter of an hour and a tip settled.
1765 took the longest yet — sixty-six minutes, ten merges, and two
commits whose CI failed on the way (`58105882`, `dd7814fc`) before
`9bb49c61` went green and the deferral chain resolved. 1768 was the
quickest: one merge on a quiet `main`, out in twelve minutes.

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
