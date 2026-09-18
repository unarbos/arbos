---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# iPhone loop — cycle reports

A copy of each cycle's closing report, in the order sent, so a report missed in the conversation can be read here. The ledger is `mobile-findings.md`; the rotation record is `mobile-coverage.md`.

## Cycle 37 report (00:31 and 00:46 UTC, 09-17)

Sent in two parts. `main` `55dd38bd` on the simulator against the ArbosLife hub with the phone token.

- Long history and older-lines paging on `phone` (1,265 lines): opens at the tail in 1.1 s; "Show 200 earlier lines" pages back and holds the row under it; a second page reaches the 16:xx turns; recorded (`media/mobile/cycle-37/recording-long-history-paging-20s.mp4`). M-120 (a scroll lock after paging) withdrawn — the harness swiped on the composer's inset.
- Several workers at once on `pod`: four workers, four Done lines, Agents 3 → 7, all seven in the sheet, kept across a reopen.
- Cold start 3.1 s (was 6 s). Background 8 s → chat as it was. Attachments: photo chip with a line.
- **M-121 fixed** (#365): the direct kernel and the roster's `phone` are one kernel; the list drew it twice. `hello.store` folds the pod row into its roster twin.
- **M-122 fixed** (#365): Settings' token edit put up iOS's "Save Password?" sheet every time; the fields are `.oneTimeCode` now. Token editing exercised for the first time: wrong token → rows Off, right token → rows back.
- Style: composer at rest and photo chip vs Cursor (`01-`), list after M-121 vs All Agents (`08-`), one-surface check vs desktop #359 (M-123: already so on the phone).
- M-119: `arboslife/demo` still served by an old image (hello without `git_sha`); the #357 rerun waits. M-124: second store loss of the mobile documents at 00:44, restored from the Mac mirror; guarded mirror script since.
- Mac cost ≈ $31 (35 h at $0.88/h).
- Next: the `demo` rerun when its hello carries a `git_sha`; otherwise the worker-chat pair.

## Cycle 38 report (01:30 UTC, 09-17)

Short cycle, started by hand on the coordinator's kick after the timer question.

- Cycle timer: alive the whole time (fired 00:00:16; the cycle-37 report followed at 00:31 and 00:46). Recreated it anyway with a rule that every cycle turn ends with a `## Cycle N report` message and a copy in this file, so a missed message can be found.
- `demo` journey: **not run** — the `demo` kernel is still an old image (M-125): the machine row says `b6e7098`, the kernel's own `hello` says otherwise. Evidence to the mesh worker; the rerun is the first item once `hello` carries a `git_sha`.
- Style pass vs the Mac's one-surface look (#359): consistent, nothing to change (M-126; `media/mobile/cycle-38/01-`).
- Call screen, both states, in the simulator (no microphone → "No microphone input."): `03-`.
- PR: #365 (M-121, M-122) still the cycle's PR; nothing new to fix from this half.
- Mac cost ≈ $32 (36 h at $0.88/h).
- Next: the `demo` rerun on the mesh worker's word; otherwise the worker-chat pair against Cursor's agent chat.

## Cycle 39 report (02:10 UTC, 09-17)

- `demo` came back on `b6e7098` (hello with `git_sha`, raw frames) at 01:3x, so the journey ran first: run 30, `main` 0fede124 (= TestFlight 1194, which carries #365's M-121/M-122). J1 P (seed 125 s), J3 P, J4 P, J6 P (away card in the run), J7 **P on the merits by the runner**, P1–P3 P (photo answered, scoped call answered, connect 0.50 s), J5 F (model), PUSH U with reason.
- **But J2 failed, and it names JB-6**: every spawn on arboslife is refused in 35 ms — `start arbos-kernel serve: No such file or directory` — because the worker daemon starts children from `current_exe()` and its binary is gone from under it (runs 29 and 30). The root did the work itself, so J7's pass is not the worktree test #357 needs; JB-5 stays untested. Note to mesh and kernel: `features-inbox/2026-09-17-mobile-spawn-refused-daemon-binary-gone.md`. The runner now scores a refused spawn F instead of P.
- Phone fix, PR #373 (M-127): a refused spawn is one plain line — who would not start, where, why — instead of a "Starting worker" line with the reason inside a fold; `host` read as well as `machine` so the F15 "Running on <machine>" line fires on mesh records.
- Media: `media/mobile/journey/0917-014026/` (29 files, the whole run), `media/mobile/cycle-39/01-`, `02-`.
- Mac cost ≈ $33 (37 h at $0.88/h).
- Next: the `demo` rerun the moment the mesh worker says the daemon is restarted (the worktree test); otherwise voice notes and barge-in re-measure (cycles 12 and 14, the oldest rows left).

## Cycle 40 report (02:30 UTC, 09-17)

- Answer first: #373 (cycle 39) makes a refused spawn one plain line on the phone — who would not start, where, why — instead of a "Starting worker" line with the reason in a fold; verified on `demo`'s replay; ready for review. The cycle around it: run 30 on `demo` (`b6e7098`), J7 P on the merits but J2 F — every spawn on arboslife refused, ENOENT on `arbos-kernel serve` (JB-6, the daemon's binary gone from under it), so JB-5's worktree path is still untested.
- This cycle, against tonight's kernel work: a #374 kernel built on the Mac with a 20 s stall clock, the app on it through launch args. The stall notice reached the phone 32 s after the send but was cut at two lines with raw backticks, and the phone had no Stop for the line to offer. PR #376: notices read whole with code chips, "Waiting on <worker> — <step>" instead of "Working waiting on", and a stop square in the composer while a turn runs that ends it ("Worked 32s", "Stopped by you").
- F15's root cause recorded (M-131): the macOS kernel deaf to its finished children — Jacob's Mac, not the phone.
- Media: `media/mobile/cycle-40/01-`…`04-`.
- Mac cost ≈ $34 (38 h at $0.88/h).
- Next: the `demo` rerun on the mesh worker's word (JB-6); otherwise voice notes and the barge-in re-measure.

## Cycle 41 report (06:20 UTC, 09-17)

Answer first: the Mac is back, the loop's harness is no longer only on it, and JB-6 can now be made to happen on demand — which is what let the phone be shown naming it.

- **The Mac.** Its only private key lived in `/tmp` on the previous worker's VM, so losing the worker lost the machine (M-132). Recovered from first principles: new key pair `arbos-mobile-2` with its private half **in the vault** (`l6zmhcj7thvr6bi7xdfr36akc4`), the disk imaged before anything was touched, relaunched on the same dedicated host. Everything survived — Xcode 27, the iOS 27 runtime, the `Arbos iPhone 15 Pro` on its old udid, `idb`, `ffmpeg`, `~/arbos`, 54 cycles of `~/mobile-out`, `~/mobile-docs`. **Nothing reinstalled.** New address `13.217.52.163`, instance `i-016b41e8f552dec09`.
- **The harness** (M-133): recovered off the imaged disk and committed as `deploy/mobile/` in #387, scenario one-offs under `deploy/mobile/scenarios/`. The TestFlight poll now runs from this VM out of `deploy/feedback/`, so feedback survives the Mac being down.
- **The journey records its kernel** (#387): `kernel.py hello` reads the kernel's own `--version` line off the attach socket — not the hub's `/list`, whose `git_sha` is whichever process registered last. `mac-journey.sh` reads it at both ends of a run and `journey-record.py` writes the history line, so QA stops printing "kernel commit: not recorded by the phone loop". Verified live: `arbos-kernel 0.2.0 01e6b6530809 protocol 1 BINARY-GONE`.
- **JB-6 on demand** (M-134): a #385 kernel started from a copied binary, the file then deleted under it. It serves happily; `binary_gone` is true in its `hello` and in its `builds` entry. The phone now says so twice — the row reads "Restart needed", the chat says it once as a plain line at the tail. The first build drew neither: the line was yielded on `hello` and the transcript seed replaces the item list, so it was wiped before it was drawn; fixed in #396, verified on the rebuilt app.
- PRs: **#387 merged** (the harness, the journey's kernel commit, the app reading `builds`); **#396** open with the one-line ordering fix, which #387's merge beat to the branch.
- **For the features agent** (M-135): the kernel computes `binary_gone` live for every `hello`, but the hub only hears it in the registration frame — so a binary deleted *after* a kernel registered, which is JB-6 exactly, leaves the roster silent until the link bounces.
- Feedback poll: 19 submissions on the account, all 19 already in the log. Nothing new.
- Media: `media/mobile/cycle-41/01-launch.png`, `02-list-restart-needed.png`, `03-chat-binary-gone.png`, `roster-before.json`, `roster-after.json`.
- Cost: the dedicated host has billed ~$36 since 09-15 and bills whether or not an instance runs, so the episode added under $1 of AWS (two small helpers, some snapshots, all deleted). What it cost was about three hours of loop time, 109 minutes of it the host's scrubbing workflow.
- Next: the oldest rotation rows — voice notes (cycle 14) and the barge-in re-measure (cycle 12) — then the `demo` journey on the new recording, with the kernel commit in the record this time.

## Cycle 42 report (06:50 UTC, 09-17)

Oldest rotation rows: barge-in (last done cycle 12) and the voice path (cycle 14). Recording due and taken.

- **The find: the first 320 ms of every call went in the bin** (M-138). The call opens the microphone and then connects, and connecting takes 0.64–0.77 s; captured frames were only pointed at the socket once `connect()` returned. Fixed in #402 by holding the newest two seconds and flushing them when there is somewhere to send them. Measured before, 42 of the rig's first 50 frames reached the socket; after, 50 of 50.
- **It needed a new rig** (M-140). The loop could not exercise the microphone path at all: `-injectWav` writes straight into the socket and never touches capture. `-micWav` plays a clip into `audio.onCapture` from the moment the engine starts, and counts frames at both ends. Two of my own wrong turns are in the commit history rather than tidied away — I first read `-injectWav`'s clipped transcripts as evidence, and then suspected my own rig's pacing.
- **Barge-in: 500–522 ms** over four runs against 401 ms at cycle 12 (M-136) — but the number is the round trip to the gateway, not the phone. The phone's own part is the 2–4 ms between being told and cutting playback, and has not moved. Quoted separately from now on. Connect has gone 0.28–0.45 s → 0.64–0.77 s over the same period, which points the same way.
- **Kernel-answered first audio 4.4–8.4 s** against 2.93 s when first measured (M-137). All of it inside the kernel's turn; small talk unchanged at 502–682 ms. Not the phone's, but it is what Jacob feels when he asks his project something.
- **For the voice gateway** (M-139): with the phone proven to send every frame (`clip=450 sent=450`), the server still loses "Hello" and splits the sentence, six runs of six. `features-inbox/2026-09-17-mobile-first-word-lost-in-the-speech-server.md`, with a reproduction.
- Purpose check: the call screen is one surface, voice-first — orb, the project's name, nothing else. Consistent with the Mac app.
- **Settled after the first write-up**: the burst was the last candidate for the residual loss, and it is not the cause — `-flushPace` sent the held frames at real time and at four times real time, six runs, same transcript every time, `clip=50 sent=50` throughout. The switch was removed again. And the final A/B, three runs each way, is the cleanest statement of the whole cycle: 40/42/40 of 50 frames before, 50/50/50 after, while the transcripts say nothing useful — the fullest sentence of the six came from a run that lost eight frames (`media/mobile/cycle-42/mic-path-before-after.txt`). The counters are the evidence; the transcript is the illustration.
- **M-141**: the store dropped the cycle-42 rows again between writing and re-reading them; restored from the Mac mirror, third time it has been the only surviving copy.
- PR: **#402**. Feedback poll: 19 submissions, all already in the log, nothing new.
- Media: `media/mobile/cycle-42/01-launch.png`, `02-call-mic-path.png`, `03-call-speaking.png`, `04-call-after-barge-in.png`, `05-call-speaking-from-first-moment.png`, `06-call-reply.png`, `mic-path-before-after.txt`, `recording-call-mic-path-25s.mp4` (the flow this cycle is about: a call whose speech is heard from the first moment), `recording-call-barge-in-28s.mp4`.
- Next: voice notes in the composer (cycle 14, the row this cycle did not reach), then the worker-chat style pair against Cursor's agent chat, and the `demo` journey with the kernel commit now recorded.

## Cycle 43 report (08:08 UTC, 09-17)

The composer's microphone (the oldest rotation row, cycle 14), and then the voice server's first-word fix, which landed mid-cycle.

- **The first-words thread closes at both ends** (M-144). Their 07:51 deploy verified on the capture path: `ask.wav` returns one final, "Hello Arbus. What are we working on right now? Give me one sentence.", where the same clip through the same rig gave "o Arbus…" + "sentence?" an hour earlier. Small talk whole, first audio 559 ms — their figure exactly. The cause was the duplex model's own ASR dropping the onset, proved by streaming into the container with four different leading silences; my "probably the VAD" was the right neighbourhood and the wrong detector. The phone's half was M-138's 320 ms.
- **A reply that never played no longer ends the answer** (M-145, #409). A mid-sentence pause made the orb fall back to listening just before the reply. Counted: one fallback before, zero in both runs after, reply unaffected.
- **A stopped dictation says the words are safe.** It showed the operating system's sentence in red and never mentioned that the take's words stay in the box. `ChatStore.notice` takes a `failed` flag now.
- **Two instruments caught lying, both mine**: the take metric counted a final settling as the build-956 fault, and transcript comparison counted a capped tail (150 then 149 with nothing sent). Fixed as `delta_shrinks` and `kernel.py total`.
- **One blocking everything**: iOS's notifications sheet swallowed every tap in two runs while the numbers kept arriving; one of them reported a dictation take that had never started. `-noAskNotifications` for scripted runs, and the pending alert needed the simulator erased — it survived terminate, relaunch and a privacy reset.
- **The store was never at fault** (M-143). All seven episodes were the QA loop's test agent running `cd / && rm -rf *`, then restoring a mirror older than other people's work. M-124, M-141, M-142 and the 09-16 loss corrected. Audited the 06:41–06:52 window: everything survives, byte-for-byte equal to the Mac mirror.
- **Still owed from this cycle**: F18a/F18b, the composer take's own behaviour, has no clean run yet — every attempt was voided by the alert. It is the first item of cycle 44, and the blocker is now removed.
- PR: **#409**. Also open: **#406** (per-document mirroring). Feedback poll hourly now, nothing new.
- Media: `media/mobile/cycle-43/01-launch.png`, `02-call-first-word.png`, `03-call-small-talk.png`, `04-mid-pause-stays-thinking.png`.

## Cycle 44 report (08:33 UTC, 09-17)

Opened early rather than waiting for the timer, since cycle 43 was closed and the voice server's second deploy landed at 08:21. Three verifications, all passing, and one thing they made visible.

- **The composer's voice notes are closed at last** (M-147), the cycle-14 row that three cycle-43 attempts could not reach. All three on counts: 13 segments with `delta_shrinks=0`; the kernel's transcript total 1526 before the take and 1526 after it, with the take demonstrably having run; 1531 after his own tap. So the words only grow, a dictated note starts no turn, and the line goes when he sends it. What unblocked it was `-noAskNotifications` and erasing the simulator.
- **The duplicated answer is gone and the frame says why it ended** (M-148). Two runs on `pause.wav`: one answer, `reason=completed`, and one `response.done` per call where there were two — the superseded-before-audio case sends no frame at all now. The phone reads the reason instead of inferring from the frame's presence (#412), with M-145's silence check kept behind it. No premature fallback: the orb holds in thinking from the second segment to the answer.
- **What the fix made visible** (M-149): a mid-sentence pause leaves three lines in the chat for one spoken sentence — the partial question, a notice reading "stop during model call", and the merged question. Jacob would see his own sentence twice with an internal message between. Reported upstream rather than papered over on the phone; folding a user line by string prefix would be the phone guessing at intent.
- Purpose check: the call screen and composer are unchanged and still one surface. The three-line artefact above is the only thing in this cycle that reads as machinery rather than conversation, which is why it went upstream rather than into a workaround.
- PR: **#412**. Also open: **#406** (per-document mirroring), **#409** (cycle 43). Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-44/01-launch.png`, `02-dictation-words-in-the-field.png`, `03-take-ended-words-wait.png`, `04-his-own-send.png`, `dictation-counts.txt`, `mid-pause-after.txt`.
- Next: the worker-chat style pair against Cursor's agent chat (the standing "next" since cycle 37), then the `demo` journey with the kernel commit now recorded, and the list's search and filter (cycle 25).

## Cycle 45 report (09:15 UTC, 09-17)

Took the coordinator's heads-up on #417 as the cycle, ahead of it landing.

- **The phone already reads the hub's reason** (M-150), so the answer to "do you show it" was yes — but it showed the hub's own log line, `no machine named "arboslife" is registered (known: none)`. The reason does not arrive in the close frame at all: the hub sends it as an `error` frame and closes with no reason, and #417 is what stops that frame racing the close. My first commit assumed the close was the channel and was wrong; the second says so.
- **Fixed** (#420): the two shapes meaning a machine or a project is not there now read as a person would say them. Anything unrecognised keeps the hub's words — checked against four real hub strings, two of which must pass through untouched.
- **What staging it found** (M-151): a project whose machine goes off does not say so, it disappears from the list. Opened `fieldwork` while its kernel ran, stopped the kernel, reopened: the row is gone and only `pod` remains. Correct by the current rule and wrong from Jacob's side — his project reads as lost rather than asleep. It also makes #417 narrower than it looks, because the refusal is hard to reach from the UI when the row goes before he can tap it. Proposal recorded for a later cycle: a project he has opened before keeps its row, drawn Off, naming the machine it waits on.
- Purpose check: the list is still clean and the refusal wording is now the same register as the rest of the app's plain lines.
- PR: **#420**. Also open: **#406**, **#409**, **#412**. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-45/01-list-machine-up.png`, `02-project-open-while-up.png`, `03-machine-off-project-gone.png`, `hub-refusal-wire.txt`.
- Next: the worker-chat style pair against Cursor's agent chat, still the oldest style row; then the `demo` journey with the kernel commit recorded; then M-151 if it is wanted.

## Cycle 46 report (09:38 UTC, 09-17)

The worker chat (rotation row from cycle 32) and the style pair against Cursor's agent chat (standing since cycle 37). Recording due and taken. The app came out well; the rig did not.

- **The rig was tapping the wrong project** (M-152). A still is 472×1024 pixels, `idb` taps in 393×852 points, so a coordinate read off a screenshot lands about a row low. Two runs opened the wrong project while looking perfectly healthy — stills, timings and a recording all arriving. Only the name in the header gave it away. Third instrument this week to report confidently about something it was not measuring. Shared, named and explained in `deploy/mobile/sim-lib.sh` (#424).
- **The worker chat works** (M-153): "Agents 12" → a sheet of workers by goal with Done ticks and chevrons → a worker's chat with its name in the header. Recorded.
- **Style pair consistent** (M-154): same plain header, same flat surface, same composer pill as Cursor's agent chat. Arbos's Done tick before the name earns its place. Arbos replacing an unusable composer with "Follow-ups aren't available for this worker" is better than Cursor's box that cannot send.
- **Not settled, and not asserted** (M-153): the worker chat still reads "Nothing on record yet." Cycle 32 blamed the kernel answering `total: 0` for an archived agent; I could not reproduce that count today — the attach socket's `snapshot`/`tree` came back with no agents inside 12 s — so there is no number and the cause stays open. Next cycle gets the agent ids the app uses and asks the kernel for each.
- **M-149 is spreading** (M-155): the mid-sentence-pause artefact is on `phone` and `qa-cycle-11-demo` too, from ordinary voice use rather than my staged clip. Already upstream; noting the spread.
- PR: **#424**. Also open: **#412**, **#420**. #406 and #409 merged. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-46/01-launch.png`, `02-project-chat.png`, `03-workers-sheet.png`, `04-worker-chat-done.png`, `05-back-to-project.png`, `recording-worker-chat-30s.mp4`.
- Next: the worker-history count that this cycle could not get; the `demo` journey with the kernel commit recorded; the list's search and filter (cycle 25); M-151's disappearing project row if wanted.

## Cycle 47 report (10:50 UTC, 09-17)

The list's search and filter (rotation row from cycle 25) and the worker-history question cycle 46 could not answer.

- **A line typed on a filtered list went to a project not on screen** (M-157, #433). The composer picked its target from the whole roster, so searching for one project left it addressing the last one opened. Shown on one run: unfiltered with `demo` last opened it says "Message demo…"; filtered to `subnet` it says "Message subnet120…"; filtered to nothing it says "Plan, ask, build…" and cannot send, where before it would have offered the invisible project. This is Jacob's build-956 complaint in the one state nobody had exercised since the filter was added, and the cycle-25 note had recorded the symptom without it being picked up.
- **The worker chat's empty state is settled, and it is not the phone** (M-156). The kernel's live tree has one agent, `root`, 437 lines; the transcript names the 12 finished workers the sheet is built from; and the kernel answers `total: 0` for eight of the eight asked. Cycle 46 missed this because my probe read the frame's `agents` key when the field is `tree`. Counted evidence appended to the existing ask rather than opening a new one.
- **A harness note recorded, not fixed** (M-158): `idb ui text` types into whatever holds focus and the search field keeps it, so two runs put a marker in the search box and reported "nothing sent" truthfully but for the wrong reason. Reading the composer's placeholder proved the target without sending, which is the better test — it measures what the composer is addressing rather than what one tap managed to do.
- Purpose check: the list stays clean under search, one section and one composer, and the composer now always names something visible.
- PR: **#433**. #412, #420 and #424 merged. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-47/01-launch.png`, `02-composer-names-last-opened.png`, `03-composer-names-the-filtered-project.png`, `04-filter-matches-nothing-no-target.png`, `worker-history-counts.txt`.
- Next: the `demo` journey with the kernel commit recorded; background minutes/hours (cycle 27); network drop and reconnect (cycle 23); M-151's disappearing project row if wanted.

## Cycle 48 report (12:40 UTC, 09-17)

The `demo` journey with the kernel commit recorded — run 31, the first since the daemon restart.

- **The thing that worked**: the kernel's own build is in the run record, read off the attach socket at both ends — `arbos-kernel 0.2.0 0f2a8bc68cc6 protocol 1`. Deliberately not the roster, whose machine-level `git_sha` for `arboslife` is empty precisely because its seven processes disagree. QA can attach a verdict to a commit now.
- **The runner reported a kernel fault that was not happening** (M-160, #447). "J2 FAIL spawn refused: No such file or directory" in a run where no spawn occurred; the refusals quoted belong to runs 29 and 30. An empty anchor let `hist` read the whole transcript. Two guards now: an unset anchor yields nothing and says so, and a missing challenge line fails J2 for the reason it failed instead of scoring the steps below it. Fourth instrument this week to report confidently about something it was not measuring.
- **JB-6's premise is in doubt** (M-161): `arboslife`'s roster carries #385's per-process `builds` — six kernels and a worker daemon, none reporting `binary_gone` — and the demo kernel says `binary_gone: false` at both ends. The state JB-6 was attributed to is not the state that machine is in. Nothing for the phone; worth the mesh and kernel owners knowing before more is built on it.
- **The real finding** (M-162): `type_send` is unreliable — two of eight typed lines reached the kernel, and the two that landed bracket the six that did not. That invalidates every score below the first miss, so run 31's record says `unexercised` rather than `fail`. Not fixed; the causes to separate are a 436-character line through `idb ui text` and sends attempted while the root is busy after a 170 s seed turn.
- Not reached this cycle: background minutes-to-hours (cycle 27), the network drop (cycle 23) and the #417 refusal wording on the phone. The journey took 22 minutes and the scorer fix took precedence. They lead cycle 49.
- PR: **#447**. #433 still open. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-48/01-launch.png`, `02-journey-seeded.png`, `recording-journey-challenge-45s.mp4`; the run at `media/mobile/journey/0917-120912/`.

## Cycle 49 report (14:00 UTC, 09-17)

The mesh survey's two phone asks, folded into the network-drop rotation row where they belonged.

- **The app treated a refusal and a transport failure the same, and got both backwards** (M-163, #465). A refusal was retried for ever — `reconnect()` cleared `refusal` at the top, so the countdown restarted each round — which gets the same answer and implies waiting helps. A transport failure, the case that usually clears by itself, showed URLSession's sentence about a socket. Now `KernelFailure` carries which, on the rule the hub guarantees: a reason means a verdict, no reason means the path. A refusal says the hub's words and stops; the path names the status or the host and keeps trying.
- **Verified for the transport half from the app's log**: a hub that is not there classifies as `transport("127.0.0.1 could not be reached — retrying")`. That is the half the survey called most wrong.
- **The refusal half is not verified on the device and I am not claiming it** (M-165). Both attempts opened `pod` — the direct kernel, which does not use the fixture hub — because the row finder missed the fixture's project and the fallback is `pod`. Fifth instrument this week whose failure mode is carrying on and measuring something else. Leads cycle 50 along with a reliable way to open a named project.
- **`has no kernel serving "x"` now reads as a sentence** (M-164): "demo's kernel on arboslife isn't running. Retrying in 4s." It stays on the retrying path, because a kernel that is not running can start — but the wording no longer says "Link lost" about a live link. That needed a third state the app did not have: `refusal` stops the countdown, `standing` only replaces the wording.
- Not reached: background minutes-to-hours (cycle 27). It has now slipped two cycles and goes first in 50.
- PR: **#465**. #433 and #447 still open. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-49/01-project-open.png`, `02-hub-gone.png`, `classification-log.txt`.

## Cycle 50 report (14:30 UTC, 09-17)

The rig's row targeting, which had cost three cycles, and two rotation rows behind it.

- **Fixed properly rather than patched again** (M-166, #472). `find_row.py` knew four project names by glyph colour, raised on any other, and divided by 3 for a screenshot that is 1.2× the point size — and callers fell back to a default row, so a run continued and reported on whatever it opened. `ui.py` drives the simulator from `idb ui describe-all`, which gives labels and frames in points already. An exact label match wins; a name that is not there exits 1 and prints nothing. It proved itself the same hour by stopping when a project turned out to be listed under a different label.
- **Background seven minutes, then resume** (M-167, the cycle-27 row that had slipped twice): 16 elements before and after, the last three text lines byte-identical by md5, no offline line on return, and a line sent straight after resume landed in the kernel. Cycle 27 checked this by eye; this is the same shape measured.
- **The refusal path still cannot be reached, and that is the finding** (M-168). Stopping a kernel removes the project's row, so there is no chat to show a refusal — sampled four times over 32 s, nothing either time, because the app is on the projects list. That reframes #465: its transport half is what Jacob sees, its refusal half is close to unreachable until a project he has opened keeps its row. M-151 is now worth doing rather than noting.
- Purpose check: the list and chat are unchanged this cycle; the work was in the rig.
- PR: **#472**. #433, #465 still open; #447 merged. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-50/01-fixture-project-open.png`, `02-before-background.png`, `03-after-seven-minutes.png`, `04-sent-after-resume.png`, `evidence.txt`.
- Next: M-151 — a project keeps its row when its machine goes, drawn Off and naming what it waits on. That unblocks the refusal wording and fixes the "my project vanished" reading at the same time. Then the journey with `type_send` fixed (M-162).

## Cycle 51 report (18:30 UTC, 09-17)

M-151: the project that vanishes when its machine goes off.

- **Half done, and left as a draft rather than merged looking finished** (M-169, #480). A project Jacob has opened keeps its row now: `k51-place, Idle` → `k51-place, Off` after its kernel stops and a pull-to-refresh, still there when the hub goes too, where cycle 45 saw it vanish entirely leaving only `pod`. Tapping it reaches a chat again, which cycle 50 could not do at all.
- **What is not working, and I am not claiming it**: the row says "Off" where it should say "awsmac is off" or "k51-place isn't running on awsmac". `waitingOn` is empty. Survival alone does not prove my branch fired — M-88 already keeps rows when the hub does not answer, which is exactly the second case. Cause not established; cycle 52 instruments it rather than guessing.
- **The hub deregisters a machine whose only kernel stops** (M-170): `/list` returns `[]` rather than the project marked not-live. So a client cannot tell "asleep" from "never here" from the roster alone, which is the reason the phone has to remember what Jacob opened.
- **The list does not refresh on relaunch** (M-171): `simctl launch` against a running app does not re-run `.task`, so two runs read a stale row and I drew the wrong conclusion from the first before catching it. Pull-to-refresh does trigger it; "read the list" now means "pull first, then read".
- Recording: not taken. The cycle's work was list state, which a still shows better than a film; due again next cycle.
- PR: **#480** (draft). #433 and #472 still open; #465 merged. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-51/01-project-open.png`, `02-row-kept-kernel-stopped.png`, `03-row-kept-hub-gone.png`, `04-chat-reachable-again.png`, `evidence.txt`.
- Next: instrument `waitingOn` and finish #480; then the journey with `type_send` fixed (M-162), and the recording.

## Cycle 52 report (19:40 UTC, 09-17)

Finishing #480, which cycle 51 left as a draft with half of it working.

- **The branch was right all along; the test was not** (M-176). One debug line in the kept-row pass settled in a single run what three cycles of screenshots had not: `kept-row pass — listed 1, opened before 1, machines []`, then `keeping hub:awsmac/fieldwork — awsmac is off`. Cycle 51's runs had nothing remembered — `opened before 0` — because the project had not been opened in that install before its kernel was stopped, so the branch had nothing to keep.
- **The row now reads as intended**: `c52-place / awsmac is off · fieldwork`, with the kernel stopped and the machine gone from the roster, where cycle 45 saw it vanish entirely. Verified again end to end on the final commit `06e9f646`.
- One debug line stays — the one naming which path kept a row and why. Reading that off the screen is what sent the previous cycle guessing.
- **#480 is out of draft and ready.** This was the seventh time this week a run reported about something other than what it measured, and the cheapest of the seven to correct: a print answered it outright instead of another round of stills. Worth generalising — when a fix cannot be seen, instrument the decision rather than photograph the result.
- Purpose check: the list reads in the app's own voice; a project that has gone quiet says which thing is off rather than disappearing.
- PR: **#480** (ready). #433 still open; #472 and #485 merged, #484 closed in #485's favour.
- Media: `media/mobile/cycle-52/01-row-named-awsmac-is-off.png`, `02-final-commit-verified.png`, `evidence.txt`.
- Next: the journey with `type_send` fixed (M-162) — it invalidates most of a run and is the oldest open rig problem — then a recording, which is due.

## Cycle 53 report (21:25 UTC, 09-17)

The screenshot #502 owed, and a correction to my own claim.

- **The call's words read back in the project chat** (M-177), shown on screen at last: both halves marked Spoken, each question once because the kernel's replay replaces the local copy. Four earlier attempts failed on harness navigation — leaving a call goes through a context menu, "Tap to call" is a label rather than the control, and `-previewCall` returns to the list rather than a chat. The path that works: open the project to set the target, relaunch into the call, then come back and open the project again.
- **A claim of mine was wrong and is corrected** (M-178). I wrote that small talk "never touches the kernel". In this run GPT-Live delegated "day." too and the kernel recorded it. Whether a trivial utterance is delegated is GPT-Live's decision, not a property of small talk. Four consistent runs established what happened four times, not a rule — and I wrote it as a rule. Corrected on #502 and in `gpt-live-iphone.md`.
- **The shape question now has a picture** (M-179): the kernel's answer and the spoken answer sit one after the other, the same thing in different words. Not guessed at; deduping on text cannot work, since the two wordings differ by design.
- Purpose check: the chat reads as a conversation, with the spoken mark distinguishing what was said aloud. The duplicate answer is the one thing that reads as machinery.
- PR: **#502**. #433 and #480 also open. Feedback poll hourly, nothing new.
- Media: `media/mobile/cycle-53/01-chat-shows-the-call.png`, `02-call-screen.png`, `evidence.txt`.
- Next: `type_send` (M-162), still the oldest open rig problem; then a recording, now two cycles overdue.

## Cycle 54 report (22:30 UTC, 09-17)

**Looked at:** the shape decision on the chat mirror, then the oldest open
harness problem, M-162 — typed lines going missing in the journey.

**The decision, built.** The project chat shows the spoken conversation.
Where Live spoke the kernel's answer, the spoken row stays and the kernel's
parallel row for that turn goes. Scoped to the *turn*, not the text, because
the two wordings differ by design. Shown on screen, with the kernel's own
record proving it wrote the other wording for the same turns and that it is
not displayed: `media/mobile/cycle-54/01-one-wording-per-turn.png`, kernel
seq 1957 and 1961. M-179 closed. On #502; no further pushes.

**M-162 closed, and it was never the app.** Three faults in the one line of
shell that types a line into the composer. The tap point was fixed at
`200,788`; the composer is near `196,470` and moves as the box grows, and
with the keyboard up `200,788` is the space bar. Tapping a text box's centre
puts the caret in the middle of what is already written, so a second line
wove itself through the first — the box's own value showed it. And nothing
read the box back before pressing return.

Counted, four long lines twice each way: **6 of 8 reached the kernel the old
way, 8 of 8 the new way.** #515, with the measurement kept as
`deploy/mobile/scenarios/type-send-measured.sh` so the next person reruns it
rather than trusting the table.

**Two readings of mine corrected mid-cycle** (M-181): `idb ui text` does not
truncate, and the box can be read with the keyboard up. Both came from
looking at part of the evidence — a stripped prefix that was no longer a
prefix, and a `tail -8` of a long dump.

**Owed:** a recording, now three cycles overdue. The journey has not been
re-run against the fixed `type_send`; that is the first item of cycle 55 and
it is the run that tells us how much of the journey's history was measuring
the harness.

## Cycle 55 report (23:10 UTC, 09-17)

**The acceptance journey, run against the fixed `type_send`, and recorded.**
Run 32 on `pod`, kernel `arbos-kernel 0.2.0 efcab58f29e1`.

**Seven typed lines of seven reached the kernel**, each at "after 0s". Run 31
landed two of eight. Every J step now passes or carries a standing
`unverified`, and the steps run 31 had to record as *unexercised* are
exercised — this is the first journey run in a while that measures the app
rather than the harness. The three FAILs left are the phone-only steps: P1
dictation, P2e the photo line, P3 the call.

**Getting there found three more harness faults, all of the same family —
a check that could not pass, or a failure that said nothing.**

- J1 scored FAIL on every run because its wait read the transcript against
  an anchor it was itself about to set.
- The journey never passed `-noAskNotifications`, so iOS's permission alert
  sat over the chat and swallowed every typed line after it appeared. The
  first attempt at this run died there, and the log only said "gave up".
- The recording produced no file, because a recorder left by the interrupted
  earlier run holds the device and refuses every later one — silently, with
  the output going to `/dev/null`. Clearing it needs the simulator shut down
  and booted.

All three are fixed on #515, and `type_send` now names what is on screen when
there is no composer to type into. That is how the photo-picker cause of P2e
was identified in the same run instead of next cycle.

**A fourth, mine:** `type_send`'s read-back retried for ever on a line
containing `'seeded'`, because iOS had made it `‘seeded’` — same sentence,
same length, different characters. `ui field plain` undoes the substitutions
before comparing.

**The recording is done**, three cycles late:
`media/mobile/cycle-55/recording-journey-challenge-and-workers-123s.mp4` —
the 490-character challenge typed in about six seconds and arriving whole,
sent, the worker stepping through `Reading mathlib.py` and `creating fix
branch`, and the reply at 2:02 with the branch name and `OK (7 tests ran)`.
19 stills, `score.txt` and `run.txt` beside it.

**Owed:** P1, P2e and P3 are the only steps the journey cannot speak for, and
P2e is now understood. That is cycle 56.

## Cycle 56 report (23:30 UTC, 09-17)

**Looked at:** P3, the journey's last open phone-only failure, and the store
report that cycle 55's recordings were missing.

**The recordings were never missing.** All 23 files were in
`media/mobile/cycle-55/`, on two reads five seconds apart, with md5s
identical to the Mac mirror. Logged as a read failure (M-188), not a loss,
and deliberately not "restored" — copying an older copy over content that
was merely unreadable is the damage, not the repair. A checksum against the
mirror settles this in one command where a directory listing cannot, since a
listing is what failed.

**P3 and P2 were one fault, not two.** P2's photo tap was at y=230 points,
which is the "Private Access to Photos" banner rather than the grid. Nothing
was selected, so the picker's tick stayed disabled, so the picker never
closed — and it then swallowed the photo line *and the entire call step*.
Run 32's `P3-call.png` and `P3-call-answered.png` are stills of the photo
grid. Nobody had noticed because the step scored on the kernel's reply, and
the model answers a question about a photo it cannot see, plausibly.

Both fixed (#524) and proven:

- the photo attaches and the picker closes itself — `media/mobile/cycle-56/01-photo-attached-picker-closed-itself.png`;
- the call opens from the chat menu by name and the question lands in **this
  project's** transcript, which is what P3 exists to check — seq 2175 `Hello
  Arbus. What are we working on right now?` and 2176 the answer.

A picker that has to be dismissed by hand now scores P2 as *no photo
attached*, rather than quietly asking the model about nothing.

**I walked into the pixel/point trap myself** (M-190). I read the tick's
position off the rendered still as `426,157`; the screenshot is 472 wide and
the device is 393 points, so the tap landed nowhere. The original `355,131`
had been right. `sim-lib.sh` has existed for this since cycle 46 — the
journey simply never sourced it. It does now, and the picker taps read as
the pixels anyone would measure. A rule only holds where it is reachable
from.

**Open:** the app itself has had no scrutiny for two cycles; the loop has
been repairing its own instruments. With the journey honest again, cycle 57
goes back to the oldest coverage row — the list composer, last checked at
cycle 11.

## Cycle 57 report (23:40 UTC, 09-17)

**Looked at:** the list composer, the oldest coverage row — last exercised at
cycle 11, forty-six cycles ago — measured on `77f7c964`, which carries #433
and #524.

**Its three standing claims all hold**, and are now numbers rather than
opinions:

- the placeholder names the project a line would go to (`Message phone…`,
  seven rows on screen);
- with a filter on it names one that is **on screen**, which is #433's rule:
  `sub` → `Message subnet120…`, `const` → `Message const…`;
- with nothing matching it offers nothing — `Plan, ask, build…`, send
  disabled — rather than an invisible project;
- and it rides above the keyboard: the composer's top moves **774 → 480**
  points as the keyboard comes up, and the keyboard starts at **683**, so a
  182-point gap. Cycle 11 judged this from a still; it is read off the
  accessibility tree now, so the next check is a comparison.

**One real fault, and it is the app's** (M-191). A search matching nothing
left the screen blank under the search box, with an empty **"Read ⌄"** header
sitting over it — which reads as a section somebody collapsed, or a list
still loading, not as an answer. The empty state only ever fired when the
*roster* was empty; a filter emptying the list had no branch. It now says
`No project matches “zzzz”.`, and `No project is live. Turn the filter off to
see the rest.` for the live-only filter, and the misleading header stands
down. #529.

Before and after: `media/mobile/cycle-57/04-no-match-before-blank-screen.png`
and `05-no-match-after-it-says-so.png`.

**Worth saying:** this is the first cycle in three to find something in the
app rather than in the loop's own instruments, which is what the instrument
repairs were for.

**Next:** the rotation's oldest remaining rows are the style pair against the
Cursor stills (cycle 7) and notifications (cycle 36). AirPods, CallKit and
the TestFlight build all need Jacob's phone and cannot move here. TestFlight
was **1657** in this report and that was wrong: the steward's number is **1716** (`2eae41c7`), which already carries #529. The build on Jacob's phone is the steward's to say, not this loop's.

## Cycle 58 report (00:05 UTC, 09-18)

**Looked at:** notifications, the rotation's oldest row at cycle 36; and the
style pair, which turned out not to be due.

**The style row was not old — the ledger was wrong** (M-195). Its
reference-still numbers (2–7) were sitting in the *last checked* column and
the cycles (17, 18, 21, 37, 38) in *how*, so it read as last exercised at
cycle 7. Both the coordinator and I acted on that. It was last done at 38.
Row repaired; the rotation is only as good as the column it sorts on.

**Notifications work, and the banner had a fault worth having found**
(M-193). A reply arriving while the phone is elsewhere raised

```
arboslife/phone · root replied
```

where the chat header, the list row and the composer all say `phone`.
`ChatStore.title` went straight to the target's label, while the chat *view*
had always preferred the project's own name, then its folder, then the
target — so two surfaces drawn from the same store disagreed, and the one
showing the raw string was the one with the least room and the least
context. Fixed on #533, with before and after captured on the same simulator
minutes apart: `media/mobile/cycle-58/01-` and `02-`.

**The recording is done** and is this flow:
`media/mobile/cycle-58/recording-notification-away-and-back.mp4` — a line
sent, the phone put away, the banner, the badge, and the tap landing back in
the project.

**Two ways the rig faked a failure before any of that** (M-194). The banner
belongs to SpringBoard, so `describe-all` cannot see it, and a detector
reading the accessibility tree reported "no banner in 120s" through a run
with one plainly on the screen. And a slow reply raises no banner at all —
it is posted from the live socket and iOS suspends a backgrounded app after
about half a minute, so asking the kernel to count slowly to twenty gives a
badge and nothing else. Both are now written into
`deploy/mobile/scenarios/notifications.sh` with the reasons beside them.
Neither was a fault in the app; both would have been filed as one.

**One correction of my own process.** I committed the banner fix onto the
cycle-57 branch, which the steward is watching, and moved it to its own
branch within the minute. #529 is back to its single commit.

**Open:** the style pair is genuinely due around cycle 41 by the repaired
row, so it goes next. The rows that cannot move from here — AirPods, CallKit,
the TestFlight build on Jacob's phone — are still waiting on him. TestFlight
was written as **1657** here and corrected afterwards to the steward's **1716** (`2eae41c7`), which already carries #529.

## Cycle 59 report (00:20 UTC, 09-18)

**Looked at:** the style pair against the Cursor reference stills, the row
the repaired ledger showed as genuinely oldest.

**The chat was showing the model's markdown markers** (M-196). The kernel
writes markdown and the phone parsed only the inline kind, so a heading
arrived as `## Shapes` and a bulleted list as `- Note one`. Bold and `code`
were fine. Numbered lists were the only kind that looked right, and only
because `1.` reads as a number whether anything parses it or not.

The parser stays as it was — the full one reflows the text and throws away
the model's own line breaks, which is worse than a visible hyphen. Instead
the markers that start a line become the typography they stand for before
the inline parse, and lines inside a fence are left alone. #535.

The kernel's text was identical across the before and after runs (seq 2210
and 2214), so the difference on screen is the rendering and nothing else:
`media/mobile/cycle-59/01-` against `02-` and `03-`.

**One difference I could not close** (M-197). Cursor times every row on the
right — `1m`, `4m`, `3m`. Arbos shows seven rows all reading `Idle`, in
alphabetical order, indistinguishable, with nothing to say what moved
recently. The app cannot invent it: `GET /list` carries no per-project
timestamp, and the only time on the payload is `since` on the machine, the
same for every project on it. Filed for the hub's owner as
`internal/features-inbox/2026-09-18-hub-last-activity-per-project.md`.

**Otherwise the two lists agree**: same top bar, same collapsible Working and
Read sections, same row shape, same composer pill. Arbos's placeholder names
the project where Cursor's is generic, which is the better of the two and
deliberate.

**A process fault of mine, now twice** (M-198). I committed cycle 58's fix
onto the cycle-57 branch and cycle 59's onto the cycle-58 branch, both being
watched by a steward. Caught inside a minute each time and moved, and both
PRs are back at exactly the commit they were reviewed at — #529 at
`46a53ba2`, #533 at `7dd74f4d` — but the branch a commit lands on should not
be something I discover afterwards. Checking `git branch --show-current`
before committing, not after pushing.

**Corrected in the record:** the build on Jacob's phone is the steward's,
and it has moved twice while this cycle ran — **1657** was wrong in two of
my earlier reports, **1716** (`2eae41c7`, carrying #529) replaced it, and
the steward has since written **1725** (`74b49b4c`, carrying #533) and then
**1731** (`3ef5f436`, #535), **1735**, and now **1748** (`53dffd33`, #543).
That number is the steward's to set and this loop's only to record; it has
moved six times while these cycles ran, which is why no report of mine
should state it as a fact of its own.

### Cycle 59, second half (00:25 UTC) — the rest of the chat pairing

**Links are already right** (M-199). A markdown link and a bare URL both draw
in the accent colour and are tappable, as Cursor's `#231` does. Checked
rather than assumed, because neither is something this app maintains — one
comes from `AttributedString(markdown:)` and the other from iOS's own
detection, and either could stop working without anybody touching this code.

**A long list item wraps to the margin where Cursor hangs it** (M-200), so
after the first line the list's shape is gone. Left as it is, on purpose: a
hanging indent costs about twenty points of width on every line of every
item, and this is a phone. Arbos buys text per line where Cursor buys
structure, and the item boundary still reads because `2.` starts a line.
Recorded as a considered difference in the same class as the mic-versus-send
-arrow one from cycle 37, not as a fault. It is also a renderer restructure
rather than a setting — the agent bubble is one `Text` with the streaming
caret on its baseline — which is not worth the regression risk on a
judgement call.

### Cycle 59, third pass (00:35 UTC) — the surface nobody had sampled

**Settings was standing on iOS's ground, not the app's** (M-201). Sampled,
because the difference is small enough to argue about: the sheet was
**45.4% `#2c2c2e`** — iOS's grouped-cell grey, a colour in no part of this
palette — against a projects list that is **92.3% `#161514`**. Nearly half of
one screen on a colour the app never chose.

The view already hid the page behind the `Form` and painted it
`ArbosTheme.bg`; what it could not reach was the fill behind each **row**,
and the same modifier on the `Form` does not get there either. It goes on
each section now. Measured after: `#2c2c2e` gone, `#212121` in its place at
the same 45.4% — the raised colour the composer and the Agents pill already
use. #537, with `media/mobile/cycle-59/07-` and `08-`.

The eye barely registers it. That is why it survived fifty-nine cycles, and
why a pixel sample found it where five cycles of looking at stills had not.

**Dark-only is deliberate on both sides** (M-202). The reference stills being
light raised the question of what the phone does there. It forces dark with
a fixed palette; the desktop's `palette.rs` uses `cursor_dark` with
`t.bg = grey(0x161514)` — the same literal value as `ArbosTheme.bg` — and
says outright that Cursor's light theme was never sampled. Checked against
the desktop's source rather than assumed.

That closes the style pair. #535 (merged) and #537 carry the two defects it
found; everything else on the surface agrees with the reference or differs
for a reason that is now written down.

## Cycle 60 report (00:45 UTC, 09-18)

**Looked at:** the four rows that had gone longest unchecked — cold start,
away-and-back, long history, attachments — all sitting at cycle 37, which is
twenty-three cycles.

**Nothing has slipped, and one thing improved.** Each row had a figure from
cycle 37; each now has one for next time.

| | cycle 37 | now |
|---|---|---|
| cold start to a list with rows | 3.1 s | **3.0 s** (seven rows) |
| tap to a chat with a composer | 1.1 s | **0.9 s**, on **2220** lines where cycle 37 had 1265 |
| away 8 s and back | "chat as it was" | 26 text rows before, 26 after |
| `Show 200 earlier lines` | "pages 200 back" | **3.6 s**, older content arrives, position holds |
| photo chip | "chip in the field, send arrow replaces the mic" | chip with its `×`; right-hand button is `Up` |

The chat opens faster on nearly twice the history. Stills in
`media/mobile/cycle-60/`.

**No defect in the app this cycle.** Which is worth saying plainly, because
the rotation exists to find drift and finding none is a result.

**One in the rig, caught before it became a bug report** (M-204). The first
version flung six times to reach the top of the window and reported
`pager: not found`. The pager is real — it sits at the top of a
two-hundred-line window of long replies, which thirty flings reach and six do
not, and six had no way to know they had fallen short. It now flings until
the pager appears **or the transcript stops moving**, and says which. #539.

That is the tenth instance this week of the rig reporting about something
other than what it touched, and the first where I doubted the rig before the
app. The earlier fixes are paying for themselves.

**Next:** with these four done, the oldest rows left that can move here are
the notifications follow-ups and the worker-chat surfaces. AirPods, CallKit
and the TestFlight build still need Jacob's phone. The build there is the
steward's **1725** (`74b49b4c`).

### Cycle 60, second half (01:05 UTC) — the settings sheet, and a gap that closed itself

**The list lost six projects and said nothing** (M-206). With a hub token
that cannot work the list falls to one row — the pod's own kernel, which
needs no hub. `HubError` already words it plainly (`Hub token refused.`) and
`ProjectStore.problem` already held it; the only thing that ever drew it was
`emptyState`, which fires when the list is empty. It never is, so the
sentence was unreachable. Now it shows whenever the hub has something to
say. #543, `media/mobile/cycle-60/14-`.

That is the same shape as M-191 two cycles ago: a message that existed, with
only the all-empty case able to reach it. Worth watching for a third.

**A saved token outlives the app** (M-207). Testing that path I stranded the
simulator: uninstall and reinstall does not undo a saved token, because it
is in the Keychain, which survives an app's removal. The rig said "rows after
a reinstall: 1" and I read it as the hub being down. The scenario now
restores by typing the build's own token back, and says why.

**The gap I filed at M-197 came back filled, within the hour** (M-210). #538
put `last_activity_ms` on the roster per project — its doc comment cites
cycle 59's finding. The row now carries `now`, `4m`, `2h`, `3d` on the right.

**Not claimed as shown.** The hub deployed on ArbosLife still predates #538
and omits the field, so the only case observable today is the absent one:
the list unchanged, no times and no guesses, which is the right behaviour
and is what the screenshot shows. The times want a run once the hub
redeploys.

**Store wobble** (M-209): two writes refused with `Resource temporarily
unavailable`, the third succeeded. Checked first that the file was intact
and matched the mirror, because the dangerous case is a half-written file
copied over a good one. It was clean.

### Cycle 60, fourth pass (01:20 UTC) — the question the row had never answered

Re-reading the four rows for what they *left open* rather than repeating
them, the attachments row said **"Not sent this cycle"** — and had said so
since cycle 37. No cycle has ever shown that an attached photo reaches the
model.

The journey's P2 could not settle it either, and this is the part worth
keeping: it asked *"what is in this photo?"* and scored on the reply not
sounding like a refusal. So through every month the picker was silently
attaching nothing (M-189), P2 **passed** — on a plausible answer about a
photo the model could not see. A check that cannot distinguish "it worked"
from "it answered anyway" is not a check.

Asked instead to name the subject and its colour, and offered an exact
sentence for the negative, the kernel replied:

> Ice plant flowers, predominantly magenta/pink.

That is the library's first photo, correctly. `media/mobile/cycle-60/33-photo-in-the-chat-and-named.png`
shows the picture sitting in the chat above the question. M-214 closed, and
both the scenario and P2 now ask the discriminating question — with P2
scoring U rather than PASS on a reply that names neither the picture nor a
refusal.

## Cycle 61 report (01:35 UTC, 09-18)

**Looked at:** the settings sheet and the several-workers chat, both of which
cycle 60 had already covered — so the row bumps were owed rather than the
work — and then the genuinely oldest row left, the pulled-down call at 38.

**A crash, on a path anyone could take** (M-216). Open a project's call,
change your mind, tap the cross: the app dies.

```
*** Terminating app due to uncaught exception 'com.apple.coreaudio.avfaudio',
    reason: 'required condition is false: NULL != engine'
```

`start()` attaches the player to the engine and installs a tap on it;
`stop()` removed that tap unconditionally, and `removeTap` on a node that was
never attached raises. An uncaught `NSException`, so the process goes and not
merely the call screen.

Isolated to the step rather than guessed at: a script that enters the call
from the chat, never taps the orb, then closes — **GONE, two uncaught
exceptions**. The same script after the guard — **alive, none**. Muting was
never involved; that button is correctly disabled until a call is up.

It surfaced only because the scenario asked *where close landed* and got
nothing back. A run that had merely tapped and moved on would have shown a
green line.

**Both old notes about close were right** (M-217). Cycle 38 said it returns
to the chat; cycle 56 saw the projects list and I took that as a change.
Neither was wrong — 56 entered by launch argument, with no chat behind it.
Entered as a person does, from the chat's own menu, close lands back on the
chat. One correction to the row: with no call started the orb reads `Tap to
call`, not cycle 38's "No microphone input.", which belongs to a call that
started without a microphone.

The rest of the row holds: the pulled-down row shows `Add`, `Type to phone`,
the mic and the cross; a line typed on the call reaches the kernel and is
answered (seq 2278, `heard`).

**A CI red that was not ours** (M-215). `#543` failed `kernel (build + test)`
on `a_coordinator_that_spawns_and_leaves_the_page_alone_is_nudged`. The
branch changes nothing under `crates/` — the diff there is empty — so its
Rust tree is identical to main's, and the commit before passed the same job
on the same tree. Recorded for that test's owner as a sighting, not chased.

**The store is getting worse.** Five refusals this turn — `Resource
temporarily unavailable` on reads, on an append, and on a `mkdir`. Each
succeeded on retry and nothing was lost, but one read had to fall back to
the Mac mirror. That is the fifth episode in two days and the first where a
directory could not be created.

### Cycle 61, second half (01:40 UTC) — what the two named rows still left open

Both rows were already at 60–61, so I read them for what they had *not*
reached rather than run them again.

**The settings sheet's build line** (M-220) reads `Arbos, 0.2.0 (1)`. Build
`1` is what a debug build carries; Jacob's number comes from CI. So the one
screen that names the build is the one screen a simulator cannot check the
content of — and its footer, "The TestFlight build on this phone", is
literally untrue on a simulator while being right where it matters. Nothing
to change; worth knowing that this row has a claim no run here can test.

**The workers sheet is showing a fraction of what exists** (M-219). Asked
directly, the kernel's `tree` frame lists twelve children including the four
run an hour earlier. The sheet, reopened after a relaunch, shows **two**.

The mechanism is `publishWorkers()`, which yields
`workerOrder.filter { touched.contains($0) }` — and `touched` is what *this
session* watched happen. `remember()` takes the tree's children into
`workers` and `workerOrder` but never into `touched`, and none are running,
so after a relaunch the sheet can only show whatever the replay happened to
brush against.

**Left for the next cycle deliberately.** The filter is not the question;
the question is which of a kernel's children belong on that sheet — all of
them for ever, the recent ones, or the ones this project actually ran. That
is a product decision, and taking it alone at 01:40 is how a loop ships
something nobody asked for. The diagnosis is written down so the next cycle
starts from the answer rather than the search.

This also reframes M-153 from cycle 46, which found worker chats reading
"Nothing on record yet" and established the kernel answers `total: 0` for
archived agents. That remains true and separate: one is about which workers
are listed, the other about whether a listed worker's chat has anything in
it.

### Cycle 61, closing (01:50 UTC) — the decision I did not need to take

An hour ago I left M-219 open, writing that the fix "needs a decision I
should not take at 01:40: which of a kernel's children belong on that
sheet". That was half right and worth correcting.

**The desktop had the answer written down.** `desktop/src/view/panel.rs`
draws the live tree and folds the kernel's *archived* workers under a
collapsed `N archived` row, faint with their checks, below the live ones. It
does not quietly drop live ones. And the phone's twelve were all still in
the kernel's tree — nothing had archived them. So the phone was not making a
defensible different choice; it was losing rows.

Fixed: `publishWorkers()` shows a child the kernel has told us about, rather
than only what this session watched go by. **Two rows before, fourteen
after**, measured against the kernel's own tree, with the four workers run
an hour earlier back among them. `media/mobile/cycle-61/06-`.

The lesson is about where I stopped. "This needs a product decision" was
true of the filter and false of the behaviour, and the brief already names
the reference app — reading it took five minutes and turned a judgement call
into a consistency check. Deferring was the right instinct at the wrong
depth.

Both of cycle 61's changes are on #547: this and the crash on leaving a call
that never started.

### Cycle 61, journey run 33 (02:00 UTC) — the phone-only steps, settled

The hub is still not serving `last_activity_ms`, so the list's `4m` waits on
a redeploy, as instructed — not waited on. Instead: the first full journey
since the repairs of cycles 56 to 61.

| step | run 32 | run 33 |
|---|---|---|
| P2e, the photo line reaching the kernel | FAIL | **PASS** |
| P3, the call's question landing in this project | FAIL | **PASS** |
| P2, the model naming the picture | passed on a plausible answer | **`Ice plant flowers, predominantly magenta.`** |
| P1, the dictated line | FAIL | FAIL — but see below |

Every typed line landed again, and the J-steps all pass or carry a standing
unverified. Evidence and both recordings in `media/mobile/cycle-61/run33-`.

**P1 was failing on a spelling** (M-223). The clip says *summarise*; iOS's
recogniser writes **summaries**; the check looked for **summarize**. The
line reached the kernel every time — seq 2326, `Please summaries what the
workers did today in two sentences.` — and the run log printed it on the
line above the failure:

```
P1 heard: Please summaries what the workers did today in two sentences.
P1 FAIL no /user +please summarize what the workers did/ within 30s
```

The step's own log had the answer and nothing read it. Matched now on the
words the recogniser does not get to choose. Cycle 61's earlier fix
(Microphone → **Stop** → Up) is what made the line reach at all; the
spelling was hiding behind a step that never sent, and only surfaced once
sending worked.

That is the twelfth of this family this week and the cheapest to have
caught: no instrumentation, no rerun — two adjacent lines of a log that
contradict each other.

### Cycle 61, closing (02:10 UTC) — a tool that was answering the wrong question

Fixing the workers sheet unblocked something cycle 46 had left open, and
taking it turned up worse.

**A worker's chat does have content** (M-225). Opening `say sentence about
mountains` from the now-complete sheet: its name, `status`, `read ·
project-context.md`, `read · notes.md`, its three-step plan, and `Done.
Follow-ups aren't available for this worker.` `media/mobile/cycle-61/07-`.

**But the tool that said otherwise was broken** (M-224). `kernel.py total
<agent>` sent the agent name and then printed the **first** `history_end` it
saw — and attaching starts the root's replay, so the root's frame arrives
first. Every worker asked about came back with the root's count. Filtering on
the name alone then reported **0 for the root**, because the kernel answers
`main` under `root`: the opposite mistake, made and caught in the same
minute. Fixed both ways, and measured after — root 2336, three workers 8, 8
and 5, a name that does not exist 0.

**That unmakes M-153's evidence.** Cycle 46 read "8 of 8 workers answer
total: 0" off this tool and concluded empty worker chats were the kernel's
truth rather than the app's fault. The conclusion may still be right; the
measurement never supported it.

**And it cannot be retaken.** The sample was `qa-cycle-11-demo`, whose tree
now carries zero children. So: worker chats are not inherently empty, and
whether *archived* ones keep their transcripts is genuinely unknown again. I
appended a correction to
`internal/features-inbox/2026-09-16-mobile-worker-history-archived-agents.md`
— another subagent's document, so the note is additive and attributed, and
its frontmatter is untouched. Leaving a live ask standing on evidence known
to be false seemed the worse option; move it if that is the wrong call.

Thirteenth of this family, and the first where the instrument was mine and
the conclusion was already in the ledger.

### Cycle 61, the gap that closed (02:15 UTC)

The hub is serving `last_activity_ms`, and the phone draws it:

```
demo        Idle        6h
subnet120   Idle        4m
```

Quiet, right-aligned, where Cursor puts it — `media/mobile/cycle-61/08-`.
That is M-197 closed end to end: found at cycle 59 pairing the list against
Cursor's, filed for the hub because the app could not invent it, added by
#538 with the finding cited in its own doc comment, and now on screen.

Four rows still show nothing, and that is the design working rather than a
gap. The field arrives as each project's kernel updates, so the roster is
mixed for a while — and **absent is treated as unknown, never as zero**. A
phone that had defaulted to "now" would be claiming every project on an
older kernel had moved this second, which is the kind of confident wrong
answer this loop has spent the week removing.

### Cycle 61, the last question on that field (02:20 UTC)

Three projects show a time and four do not, so I checked whether that was
the app before reporting it as the design.

It is the kernel's build, exactly. Read against the per-process `builds` the
roster carries:

| kernel | projects | time |
|---|---|---|
| `c3247332dc4e` | demo, feedback, subnet120 | **6h, 32h, 6m** |
| `cbbe9922d6a2`, `30eef166a191`, `efcab58f29e1` | const, parity-proj, qa-cycle-11-demo, **phone** | absent |

Only kernels at or after #538 send the Activity frame. Nothing for the app
to do — and worth writing down before somebody chases it, because `phone` is
this loop's own test project and will stay blank until its kernel updates.

A pleasing detail: the question was answered entirely from the roster's
per-process `builds`, the field added at #385 precisely because one build
per machine had misled this loop for days. The instrument built after that
mistake is what made this a one-command answer.

### Cycle 61, the times re-checked (02:16 UTC)

Asked not to claim the times from a pre-rebuild run, I re-measured rather
than defend the timeline — which turned out to be the better move, because
the second reading is stronger evidence than the first.

| | 02:12 | 02:16 |
|---|---|---|
| hub says `subnet120` last moved | 4m ago | **8m ago** |
| the app's row reads | `4m` | **`8m`** |

The number advanced by exactly the elapsed four minutes and agrees with the
hub to the minute. One reading shows a number; two readings four minutes
apart show it is *the right* number, and that it tracks rather than
rendering once and sticking.

For the record on the timeline: the pre-rebuild check at 01:47 had every
project `ABSENT`, which is what makes the two states distinguishable at all.
Both the original run and this one were after the hub began serving the
field. `media/mobile/cycle-61/09-`.

**Noted for next time:** a fresh branch for the next PR rather than the
previous cycle's.

### Cycle 61, the live-worker check and a third correction (02:30 UTC)

**What I set out to do:** with the sheet now listing the kernel's whole tree,
see what a *running* worker looks like among thirteen finished ones.

The pill flips to **`Working 1`** twelve seconds in, and the chat's worker
line reads **`1 Working count slowly one to forty · Reading notes.md`** with
a spinner — name and live step, which is the row's claim. Both confirmed.

**The sheet's live row is not confirmed.** It showed zero Working rows and I
took that for a fault for several minutes. The worker had finished
(`2350 turn_complete`) before I opened the sheet, so `Done` was correct and
my timing was wrong. Recorded as unmeasured, not as a defect. Wants a slower
worker next cycle.

**And the tool was wrong a third time** (M-230). Chasing the above, `total
count-slowly-one-to-forty` returned **2350** — the root's count again.
M-224's fix had filtered with `f.get("agent", agent)`, so a frame with **no**
agent field defaults to the one asked about and matches; the root's
unlabelled frame could win the race under any worker's name.

What found it was running the command **three times per agent instead of
once**:

```
main                         2350 2350 2350
count-slowly-one-to-forty      11   11   11     (was 2350)
say-sentence-about-mountains    8    8    8
a name that does not exist      0    0    0
```

The earlier 8, 8 and 5 were right — but won by a race, not by the filter. A
single passing run said nothing about whether the command worked.

**The rule this loop should take from it:** a fix to a measuring tool is not
shown by one green reading. Three corrections to eleven lines of Python in
one night, two of them announced prematurely by me, and each time the
announcement rested on exactly one run. #552.

### Cycle 61 closes (02:40 UTC)

**A prediction confirmed by something I did not do** (M-232). I wrote at
02:20 that a project shows no time only because its kernel predates #538,
and that `phone` would stay blank until its own kernel updated. It has:
`phone` moved from `efcab58f29e1` to `c3247332dc4e`, and its row now reads
**2m**. Every project on the newer kernel shows a time; the two still on
older ones do not. The rule is observed now rather than inferred — and
waiting for it beat proving it by updating the kernel myself, which would
only have shown that I can change a variable.

**One gap left open on purpose** (M-233). Four attempts at catching the
workers sheet with a live row. Three surfaces confirmed — the pill
(`Working 1`, twelve seconds in), the chat's worker line (`1 Working … ·
Reading notes.md`), and the worker's own chat (`Working running sleep 90`,
updating). The sheet's own row reading `, Working` is the one I have not
caught: the first worker finished before the sheet opened, the second tap
drilled into the worker's chat instead of the sheet, the third never sent.

Three of four surfaces is not "the live state works". Left named rather than
rounded up. The method is known for next cycle: a `sleep 120` worker gives
the window, and the pill wants tapping by its frame, because the label it
carries is one the sheet rows carry too.

**Cycle 61 in sum:** a crash on leaving a call (#547), the workers sheet
showing two of fourteen (#547), P1 failing on a spelling (#551), the `total`
command corrected three times (#551, #552), journey run 33 with P2e and P3
flipped to pass, the list's `4m` shown and then re-shown against the rebuilt
hub, and a kernel CI flake written up for its owner rather than chased.

The store wobbled repeatedly through the cycle — reads, appends and a
`mkdir` all refused at least once, every one succeeding on retry. Fifth and
sixth episodes in two days.
