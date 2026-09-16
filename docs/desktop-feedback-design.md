# In-app feedback for the Arbos desktop app

Jacob's ask, in his words: *"can we create the same feedback system in the
actual app too which records information from the app like the current logs
+ trajectory?"* — and he means the desktop app.

"The same" is the loop his iPhone already has. He taps a screenshot into
TestFlight, a timer reads new submissions every fifteen minutes, each one
lands in the Project store with his words and his pictures, and it is fixed
in the cycle it arrives. Eighteen reports have gone through it today.

This design gives the desktop app the same loop. It is deliberately short
on new mechanism, because most of the parts already exist.

---

## 1. What already exists

Nothing below is being invented. Reading this table first is the fastest way
to see how small the new code is.

| Part | Already there | Where |
| --- | --- | --- |
| A per-turn feedback control | **Yes** — thumbs up, thumbs down, copy, fork, time, in Cursor's own order | `desktop/src/view/component/transcript.rs` ~4035–4078 |
| The trajectory and the kernel log, redacted and bounded | **Yes** — a `feedback` frame answered with a bundle | [#328](https://github.com/unarbos/arbos/pull/328), `crates/arbos-kernel/src/feedback.rs` |
| Credential redaction by value and by shape | **Yes** | `arbos_engine::secrets::redact`, `arbos_core::redact` (#328) |
| Versions and commits of app and kernel | **Yes** | `desktop/src/build.rs`, `klog::git_sha`, attach `hello` |
| A modal sheet pattern | **Yes** | `permissions_sheet.rs` ~168–182 |
| Window screenshot code | **Yes, but gated** to `ARBOS_DRIVER=1` | `desktop/src/driver.rs` |
| Authenticated write into another machine's store | **Yes**, on `main` | [#251](https://github.com/unarbos/arbos/pull/251), [#254](https://github.com/unarbos/arbos/pull/254), [#257](https://github.com/unarbos/arbos/pull/257) |
| A fifteen-minute pickup loop with dedupe | **Yes**, for the phone | `mobile-feedback-poll` timer, `internal/mobile/testflight-feedback-loop-investigation.md` |

What is genuinely new: a review sheet, a screenshot on the normal build, an
outbox that survives being offline, and a poller pointed at a different
directory.

Background reading, gathered for this design: the kernel inventory in
`internal/desktop-feedback-inventory.md`, the shell and control survey in
`internal/report-a-problem-ui-exploration.md`, and the delivery survey in
`internal/desktop-feedback-hub-exploration.md`.

---

## 2. The way in

**Requirement: reachable in a second. The moment he notices is the moment he
will use it.**

Cursor's answer, which the parity ledger records
(`docs/cursor-parity-report-2026-09-12.md` row 8), is a footer under a
finished answer: thumbs up, thumbs down, copy, and a relative time. The
layout worker matched it. So the control exists — it simply stops short.
Today a thumbs-down records a number and asks nothing.

**A thumbs-down opens the report sheet.** That is Cursor's shape, it is
already the fastest thing on screen, and it answers the "which turn?"
question by where it sits. A plain thumbs-down still works for anyone who
does not want to write.

**One menubar item and one shortcut** cover the other half. Half of his
phone reports are not about an answer at all — the composer was hidden, the
list said the wrong thing, the stream jerked while he read (F1, F2, F6, F12,
F16). There is no turn footer to click for those. The item opens the same
sheet with no turn anchored, and the report carries the latest turn instead.

Not a status-bar icon. Nothing in the parity ledger records one in Cursor,
and the bottom-left corner is the settings gear and the version badge.

Asked of the layout worker in
`internal/features-inbox/2026-09-16-desktop-feedback-layout-ask.md`.

---

## 3. What a report carries

**Requirement: enough to act on, without him typing any of it.**

| Part | Source | Note |
| --- | --- | --- |
| His words | The sheet's text field | The only thing he types |
| The turn's trajectory, with tool calls | Kernel `feedback` frame | Tool arguments and an output glance; bodies and diffs replaced |
| The kernel log for that turn | Same frame | The log lines inside the turn's span, plus the log's own tail |
| The transcript tail | Same frame, `tail: N` | More history than the anchor turn, for a behaviour bug |
| The app's own view of the chat | `.arbos/desktop/sessions/<id>.json` | So a drawing that disagrees with the transcript is visible |
| App version, build and commit | `build::version_label()`, `ARBOS_COMMIT` | Compiled in, because a shipped bundle has no repository to ask |
| Kernel version, commit, built-at | The bundle's `kernel` block | The running process's own commit, not the file's |
| Machine and place | `os`, `arch`, the project's name | The place *path* is left out on purpose: it names his home directory |
| Provider and model | The bundle's `kernel` block | A provider refusal reads as a bug otherwise |
| A screenshot of the window | New: the driver's capture path, ungated | The Arbos window alone, never the whole screen |

He is asked for nothing that can be read from the running app.

### The turn, and why the kernel's unit needed changing

The bundle anchors on a transcript line, so the sheet must know which line
the footer's turn began at. That needs the transcript sequence number kept
on the chat item; it is on the wire today and dropped.

Three corrections to the kernel's idea of "one turn" were filed against #328
in `internal/features-inbox/2026-09-16-feedback-bundle-desktop-answer.md`.
The important one: it split the transcript on *any* wake, and the normal
Arbos shape — he asks, the root spawns workers, the turn ends, the workers
report, and a second turn reads them and answers — is two spans. Either span
alone loses half the story. Boundaries must be the user's own wakes.

### How much of a tool's output

A four-hundred-character glance is right for a phone fold and wrong for
diagnosis: the middle of a long run is where the failure lives. The rule
asked for is asymmetric — a call that errored keeps its error uncut and
about eight kilobytes of body weighted to the tail, the last call of the
turn gets the same, everything else keeps the glance, and a call he clicked
on comes back whole. Reasoning and the sizes are in the same inbox note.

### Enough to reproduce, not only to recognise

A report that shows *that* something looked wrong is enough for a rendering
bug: the screenshot is the evidence and the fix is in the drawing code. A
behaviour bug is different. "It said it was working and then answered
something else" cannot be reproduced from a picture. It needs the sequence
that led there.

The parts above cover rendering. For behaviour, two additions:

**The transcript tail, from the kernel.** The bundle's `events` already *are*
lines of `agents/<id>/transcript.jsonl`, redacted and slimmed, so the source
is right — but only the anchor turn's span of it. Reproducing a behaviour bug
needs more history than the turn he pointed at. So the request gains
`tail: N`: the last N lines of that agent's transcript, whatever turn they
fall in, slimmed and redacted identically, alongside the anchor turn.

`tail` also replaces a weaker idea. An earlier ask was for `turns: N` with
the older turns thinned to one line each. `tail` is the better primitive: the
wake lines are in the events, so an agent reading the tail can see the turn
structure for itself, and one primitive beats two. The ask to the features
agent was reduced accordingly.

**The app's own view of the chat, from the desktop.** This one is mine, not
the kernel's, and it matters more than it sounds. The classic desktop bug is
that the app drew something the transcript does not say — one worker drawn
three times, a line that says "Starting" forever, a fold split into three.
You cannot see that from the transcript alone, because the transcript is
right; the divergence *is* the bug. So the report carries the app's own
session record (`.arbos/desktop/sessions/<id>.json`) beside the kernel's
truth, and an agent can compare the two. F14 and F15 on the phone were both
this shape.

Both are parts in the sheet with their own ✕, like every other part.

---

## 4. Nothing leaves without him seeing it

**Requirement: show him exactly what will be sent, and let him cut any of
it.**

The sheet is a centred card on a scrim. His words at the top. Then one row
per part:

- **Screenshot** — a thumbnail that opens full size, and an ✕.
- **Trajectory** — "14 lines, 6 tool calls", expandable, and an ✕.
- **Kernel log** — "83 lines", expandable, and an ✕.
- **Transcript tail** — "200 earlier lines", expandable, and an ✕.
- **The app's own view of the chat** — expandable, and an ✕.
- **Versions and machine** — the short line itself, and an ✕.

A line reads "2 credentials were removed" when the bundle's count is not
zero, so redaction is visible rather than assumed. One more control drops
every tool argument and output at once, for the case where he does not want
his code leaving at all.

Send is the only thing that sends. There is no silent path.

**The report says what he removed.** Every part he cuts is recorded as
`included: {"log": false, …}`, not simply left out. Otherwise the loop cannot
tell "he did not want to send the log" from "there was no log", and it would
chase the second while the first is the truth.

### The limits of redaction, stated honestly

Redaction works in two stages: known secrets by value, then credential
*shapes* nobody registered — provider key prefixes, GitHub and Slack and AWS
tokens, JSON web tokens, `op://` references, `api_key = …` values, private
key blocks. It is shapes, not entropy, so a commit hash survives.

Two things it cannot do, and no design should pretend otherwise:

1. **It cannot read pixels.** A screenshot showing a key in the terminal
   view is a leaked key. The mitigation is that he sees the screenshot full
   size before Send and can drop it in one click, and that only the Arbos
   window is captured, never the whole screen.
2. **It cannot tell his code from anyone's.** A `write` call's argument is
   the whole new file. That is not a credential and no shape catches it.
   Hence the toggle, and hence the question in section 9.

A related bug found in #328: those arguments are unbounded today, so one
large `write` can exceed the size cap by itself and push the kernel log out
of the report entirely. Clipping them is part of the same inbox note.

---

## 5. Delivery

**Requirement: somewhere an agent can poll; never raw logs in the public
repository; a report sent while offline must arrive later.**

### Where, and why there

**Into the ArbosLife store, by address, using the federated write that is
already on `main`:**

```
arbos://arboslife/<project>/internal/feedback/<report-id>/report.json
arbos://arboslife/<project>/internal/feedback/<report-id>/screenshot.png
```

That path is authenticated, does compare-and-swap, is capped at twenty
megabytes, is confined to `docs/`, `internal/` and `media/`, and refuses
root-owned pages to peers. It needs no new code on the hub.

Weighed against the alternatives:

- **A new hub upload route.** The hub has six routes today, all GET or
  WebSocket, keeps its roster in memory and writes one file. A feedback
  endpoint would mean a route, a store, a retention rule and an auth path
  that all already exist one layer down.
- **A GitHub issue.** The repository is public. His kernel log and his file
  paths would be public with it. Ruled out by the brief.
- **E-mail, or any third party.** A new credential to hold, and his logs on
  someone else's disk.

The deciding property is that a report arrives as **files in a directory**.
That makes the pickup loop a directory listing instead of an API client,
which is what lets it copy the phone loop rather than resemble it.

### Surviving an outage

The report is written to `.arbos/desktop/feedback-outbox/<report-id>/` on his
own disk **before Send returns**. Delivery reads from there. So:

- Offline, Send still succeeds, and the sheet says "will send when
  connected" rather than failing.
- The app can be quit and reopened; the outbox is on disk, not in memory.
- Delivery retries on the reconnect backoff the workspace already uses.
- A delivered report keeps its folder for a day with a receipt, so the loop
  can write back to it (section 7).

The desktop's chat queue is not reused: it is not persisted
(`desktop/src/model/record.rs` 19–51), so it would lose a report on
relaunch, which is the one thing this must not do.

`<report-id>` is a UTC timestamp and a short random suffix, assigned by the
app. The human-readable number comes later, from the poller, exactly as the
phone loop turns an App Store Connect identifier into `2026-09-16-1`.

Asked of the mesh worker in
`internal/features-inbox/2026-09-16-desktop-feedback-hub-delivery-ask.md`:
which project holds these, a writer token for his desktop, whether a real
`.png` is allowed under `internal/`, the create-if-absent rule, and how the
desktop should learn the hub's current address.

---

## 6. Getting picked up

**Requirement: reaches a loop within minutes, recorded under
`media/desktop-feedback/<date>-<n>/`, fixed in the cycle it arrives.**

This copies the phone loop step for step. That loop is not being forked; its
shape is being reused, and its owner —
[Test iOS app on AWS Mac loop](bc-08d8261b-fea2-5075-9949-d45f6f9d4acc) — is
the reference for anything unstated here.

| Phone loop | Desktop loop |
| --- | --- |
| `subscribe_timer` `mobile-feedback-poll`, 900 s | `subscribe_timer` `desktop-feedback-poll`, 900 s |
| Agent connects to the rented Mac | Agent connects to ArbosLife |
| `~/asc-feedback.py` reads App Store Connect | A poller lists `internal/feedback/` |
| Dedupe in `~/mobile-feedback/seen.json` | Dedupe in `seen.json` beside that directory |
| Copies to `media/mobile/feedback/<date>-<n>/` | Copies to `media/desktop-feedback/<date>-<n>/` |
| Ledger `internal/mobile-feedback-log.md` | Ledger `internal/desktop-feedback-log.md` |
| Quiet when nothing is new | Quiet when nothing is new |
| Fix goes into the cycle in progress, ahead of the rotation | Same |

A recorded report is a folder holding `report.json`, `screenshot.png` and a
`feedback.md` written for a person: his words, what the trajectory shows,
which build, and what it turned out to be.

**Two things this loop should do better than the phone's, because it can:**

The poller belongs in a mirrored place, not only on a host. The phone's
poller lives at `~/asc-feedback.py` on a rented Mac and is in no repository,
so the machine going away takes it. This one goes in the repository —
`deploy/feedback/poll.py`, reading its token from the environment — so it
survives its host and can be reviewed. Nothing secret is in it.

**Which loop takes the fix** is a coordination decision, not Jacob's. The
honest analog of the phone loop is the desktop cycle that already runs
endlessly against the app and already absorbs findings: the
[Match Cursor chat view exactly](bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39)
loop. Recommended that the timer fires into it, with kernel-side findings
filed to the features agent the way the phone loop already files them. To be
settled with that loop and the coordinator before the timer is registered.

---

## 7. Closing the loop back to him

**Requirement: when a report is fixed, he learns which build carries it.**

The phone loop does this in the ledger: a column reading `**994** (#306)`,
and the coordinator tells him in project chat. That works because TestFlight
tells him a build arrived.

The desktop can do better, and should, because it already publishes a build
for every green commit on `main` and already has an update bar in the app.

1. The fix merges. The dev channel publishes, signed and notarised, with a
   build number that is the commit count.
2. The ledger records the build against his words, as the phone's does.
3. **The loop writes `fixed.json` back into the report's own folder** on
   ArbosLife — the build number, the pull request, one sentence of what it
   was.
4. **The app reads it** on its next attach and says so where he will see it:
   "Your report from 15:12 is fixed in build 884." If he is behind, the
   update bar is already the thing that offers him the build.

Step 4 is what the phone cannot do, because TestFlight has no reply channel
to an internal tester. Here the report has an identity and the app kept it,
so the answer can come back to the same place the complaint left from.

---

## 8. The slices

Each stands alone, compiles, passes, and ships something usable, because an
hourly steward merges green pull requests and a half-feature must not be
visible.

| # | Slice | Stands alone because |
| --- | --- | --- |
| S1 | Sequence number on the chat item; call `feedback`; write the bundle to the outbox; reachable from the menubar | He can already hand the file to an agent |
| S2 | The review sheet, and window capture on the normal build | The real control, with review; Send still means "saved to disk" |
| S3 | Delivery by store address, plus outbox retry and its state line | Reports now arrive; everything before it still worked |
| S4 | Thumbs-down opens the sheet | The fast path; the menubar path already worked |
| S5 | The poller, the timer, the record folder and the ledger | Pickup; reports were already arriving and readable by hand |
| S6 | `fixed.json`, and the app saying which build carries the fix | The close; the ledger already recorded it |

S1 and S2 depend on the layout worker's answer about the sequence number and
the sheet's home. S3 depends on the mesh worker's answers. S5 depends on the
phone loop's poller pattern, which is now known, and on which loop takes the
fix.

---

## 9. What needs Jacob, not a guess

**May his source code leave the machine at all?**

The trajectory carries tool arguments and outputs. That means the contents of
files his agents wrote and read. Redaction catches credentials by shape; it
cannot catch "this file is mine". Everything goes to our own hub on
ArbosLife, never the public repository — but it does leave his laptop.

The plan is to send it, because a report without it is not actionable, and to
put a one-click control in the sheet that drops every tool argument and
output. If he would rather that control were the *default*, with the
trajectory added only when he asks for it, that is a one-line change and his
call to make.

Two smaller ones he may want an opinion on, though the owners can decide:

- **The screenshot cannot be redacted.** It is pixels. He reviews it and can
  drop it. On macOS the first capture may raise a Screen Recording
  permission dialog; a refused permission must leave the report sendable
  without a picture rather than blocking it.
- **How long a delivered report is kept** on ArbosLife. A month is proposed,
  since a report is evidence for a fix and stops being useful once he has the
  build.

---

## 10. Who owns what

| Area | Owner |
| --- | --- |
| The sheet, the screenshot, the outbox, delivery, the poller, the ledger | This feature |
| The transcript's sequence number, the thumbs-down entry point, the menubar item | Layout worker (`bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39`) |
| The bundle, its turn boundary, its budgets, redaction | Features agent (`bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027`), [#328](https://github.com/unarbos/arbos/pull/328) |
| The store address, the writer token, the binary rule, the hub's address | Mesh worker (`bc-22d20d79-de36-524a-ae31-3e1c44c03b98`) |
| The polling pattern and the fifteen-minute rule | [iPhone loop](bc-08d8261b-fea2-5075-9949-d45f6f9d4acc) |

Asks are filed in `internal/features-inbox/`, one per owner, dated
2026-09-16. No files of theirs have been edited.
