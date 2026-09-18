---
cursor:
  subagentId: "bc-37bdb830-611b-5b03-8528-e12912f71b59"
---

# Update bar: what is on the dev channel

Last checked 2026-09-18 16:40 UTC.

## Click Update. You will get build 2069. Jev is in it.

**2069** — signed, notarised, stapled, on the feed since 16:38 UTC.

Checked the same way as every number here: notary Accepted, ticket stapled,
Gatekeeper reads a Notarized Developer ID, the Ed25519 signature verifies
against the key in the app, size and hash are the feed's, all four payloads
on the release.

New since 2064 (builds 2065–2069):

- **#670** — pill-count-vs-sheet's decline can end, and it no longer
  accuses the app of what the instrument could not read

## Before that: 2064

New since 2060 (builds 2061–2064):

- **#668** — reading the projects list needs the list as much as tapping
  it does
- **#669** — a chat minted while root's kickoff runs takes its own place

## Before that: 2060, where Jev's Decisions API arrived

**2060** — signed, notarised, stapled, on the feed since 16:03 UTC.

Checked the same way as every number here: notary Accepted, ticket stapled,
Gatekeeper reads a Notarized Developer ID, the Ed25519 signature verifies
against the key in the app, size and hash are the feed's, all four payloads
on the release. #667's commit was checked to be an ancestor of the published
one, not assumed from the build number.

New since 2056 (builds 2057–2060):

- **#667** — **Jev posts OpenRouter Decisions** (`/api/alpha/decisions`,
  with state and typed questions) rather than chat completions
- **#665** — `list-search-filter-refresh` logs what the fixture answered

With #664 from 2056 behind it, a Jev failure, timeout or junk reply now
ends the turn in the open with a fault rather than hanging.

## Before that: 2056

New since 2054 (builds 2055–2056):

- **#664** — a Jev failure, timeout, or junk reply ends the turn in the
  open with a fault, rather than leaving it hanging

## Before that: 2054

New since 2049 (builds 2050–2054):

- **#659** — mobile: a worker line is one accessibility element that reads
  its words
- **#662** — `check-tools.sh` asks whether the reach-the-list step happens
  before the tap

## Before that: 2049

New since 2046 (builds 2047–2049):

- **#658** — a screen `check-names.sh` could not open is not a screen it
  cleared

## Before that: 2046

New since 2039 (builds 2040–2046):

- **#657** — Jev's first-byte wait is 1.5 s, not the chat model's 15 s
- **#654** — the chat fills the column and the Clear button goes; one
  expand control
- **#656** — the git-branch chip under the composer goes, and the status
  row is omitted when it has nothing to show

## Before that: 2039

New since 2036 (builds 2037–2039):

- **#651** — every loop scenario that taps a project row reaches the list
  first, and refuses to answer blind

## Before that: 2036, which is where Jev arrived

New since 2025 (builds 2026–2036):

- **#647** — **Jev is the controller.** The brief lives at
  `.arbos/voice-brief.md`, with overflow pointers and which LLM to invoke;
  the gateway only reads the file
- **#649** — panel tabs sit on the window tab strip; Project stays in the
  drawer and never covers the chat; the bottom +, expand and header X are
  gone; native fullscreen fills the Space
- **#648** — the refusal check reads the kernel's record
- **#646** — a half-written last line left by a dead kernel is cut at
  start, before anything appends, and said

## Before that: 2025

New since 2023 (builds 2024–2025):

- **#645** — the voice-note check reads the words against the clip's
  sentence, not just the counts

## Before that: 2023

New since 2019 (builds 2020–2023):

- **#643** — one-breath-one-answer reads the kernel's own record, so one
  spoken answer is proven to be one kernel answer
- **#644** — a place file that does not parse stops the walk there

## Before that: 2019

New since 2010 (builds 2011–2019):

- **#636** — archived workers with no chat here sit in the one archived
  list, at its indent, with their last words
- **#639** — the producer rule's mark is the read call, not a reply line
- **#641** — `list-rows.sh` checks what each row of the projects list says
- **#642** — mobile: a scroll by hand cannot rest under the pill

## Before that: 2010

New since 2006 (builds 2007–2010):

- **#638** — mobile: a chat opens at its true end
- **#637** — `reply_links_e2e` waits for the worker's report after root's
  dispatch turn (the fold-race family's last unpinned member)

## Before that: 2006

New since 2000 (builds 2001–2006):

- **#635** — iOS: the front project recorded for a cold start is the pushed
  one
- **#631** — the kernel's own failed reason replaces the window's "no reply
  from the kernel" guess when it lands right after it
- **#634** — `mac-cycle.sh` resets its rig checkout and stops if it is not
  on the branch it names

## Before that: 2000

New since 1987 (builds 1988–2000):

- **#632** — `check-names.sh` walks every screen and catches one-word
  symbols
- **#633** — mobile: the transcript ends above the workers pill, and two
  decorations stop speaking to VoiceOver
- **#627** — the quoted-reference rule's mark is a test that asserts the
  quote, not a reply line

## Before that: 1987

New since 1978 (builds 1979–1987). The desktop ones are the visible half:

- **#623** — a "key: value — rule" notice keeps its value on the line
- **#629** — no This Mac picker, a Cursor-like Project page (⌘2), and a
  typed `clear` hides the transcript in this window only
- **#630** — two timing pins in the end-to-end tests

## Before that: 1978

New since 1973 (builds 1974–1978):

- **#628** — files open as an editor, Browse files is a tree, Browser
  opens a page, and a new Terminal has no stray `%`
- **#626** — `check-names.sh` flags controls named after their SF Symbol,
  and self-tests the detector before trusting a pass

## Before that: 1973

New since 1962 (builds 1963–1973):

- **#620**, **#622** — four loop scenarios that measured without concluding
  now say what they found
- **#621** — a fork holds the copied checkpoints' tree commits under its
  own refs, so a rewind in a fork survives the source's cut
- **#625** — row pitch means nothing on a chat, so `style-pair.py` says so
- **#624** — notifications concludes with a VERDICT line and joins the
  sweep

## Before that: 1962

New since 1952 (builds 1953–1962):

- **#616** — `binary_gone_e2e` waits on the second `kernel_start` rather
  than reading it once
- **#617** — the loop clears a trigger a previous run left behind
- **#618** — the one-wording check counted a coincidence as a violation;
  it now counts the answer's rows
- **#619** — the loop runs its scenarios together (`sweep.sh`), which
  found a rotted check on its first run

## Before that: 1952

New since 1946 (builds 1947–1952):

- **#614** — dev channel test: a queue of young commits does not outlast a
  green that has waited
- **#615** — iOS: a cold start comes back to the chat that was in front, as
  the desktop reopens its active tab

## Before that: 1946

New since 1936 (builds 1937–1946):

- **#611** — a `project.toml` the kernel wrote names nothing: Home keeps
  its house, and a tab keeps the colour its path picks
- **#613** — a place file that does not parse is said on root's
  transcript, and the machine's file is not used in its place
- **#612** — the running-worker line was there all along; three patterns
  matched on the animated spinner and hid it

## Before that: 1936

New since 1930 (builds 1931–1936):

- **#609** — the headless BUILDING list names `libnotify-bin` and
  `python3-pil`, with the one apt line
- **#610** — iOS: the chip's × reads "Remove <file>", and the attachments
  row's other half is measured

## Before that: 1930

New since 1916 (builds 1917–1930):

- **#603** — the loop's pill and sheet agree; every apparent disagreement
  was the instrument
- **#604** — dev channel: the deferral clock runs on the green build, not
  on the commit still under test. This is the one that keeps the Update
  button moving while `main` merges faster than CI finishes
- **#605** — every script runs the tool beside it, and `check-tools.sh`
  keeps it that way
- **#606** — `say_title_rename_e2e` waits for the worker's first report
- **#607** — the loop reads the orb's phases
- **#608** — when you can name the wrong predicate, change the predicate

## Before that: 1916

New since 1910 (builds 1911–1916):

- **#602** — the loop cannot blame the app for what the model decided:
  it reads back before sending, and asks the kernel whether a worker ran

## Before that: 1910

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
