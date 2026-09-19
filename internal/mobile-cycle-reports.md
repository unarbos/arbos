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
the TestFlight build all need Jacob's phone and cannot move here. A TestFlight
number was stated in this report and it was wrong. The build on Jacob's
phone is the steward's to say, not this loop's, and it is written in one
place only: `internal/mobile-mac-host-and-testflight.md`.

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
the TestFlight build on Jacob's phone — are still waiting on him. A TestFlight
number was written here and corrected afterwards. It is not restated: the
steward's number lives in `internal/mobile-mac-host-and-testflight.md`.

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

**The build on Jacob's phone is the steward's number, not this loop's.**
The number itself is not written here, and no longer anywhere but
`internal/mobile-mac-host-and-testflight.md`. Earlier reports of mine chased
it through several values, one of which was simply wrong. A number restated
in a dated report is read later as current, which is the mechanism that
produced the mix-ups; the only facts worth keeping here are whose number it
is and that this loop does not invent one.

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
and the TestFlight build still need Jacob's phone. The build there is the steward's, and
is recorded in `internal/mobile-mac-host-and-testflight.md` rather than
here.

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

### Cycle 61, the gap closed (02:42 UTC)

M-233's named gap is shut. Caught with a `sleep 150` worker, seventeen
seconds in, **all four surfaces show the live state**:

| surface | what it says |
|---|---|
| the pill | `Working 1` |
| the chat's worker line | `1 Working sleep 150 seconds · sleeping 150 seconds` |
| **the sheet's row** | `⠼, sleep 150 seconds, sleeping 150 seconds` |
| the worker's own chat | `Working running sleep 90` |

The row's claim holds in full, and `media/mobile/cycle-61/11-` shows it.

**It nearly went into the ledger as a defect** (M-235). The scenario counted
live rows with `grep ", Working$"`, found **0 of 12**, and printed
`VERDICT: the sheet lists 12 workers and marks none of them working`. But a
finished row reads `<goal>, Done` and a live one reads
`<spinner>, <goal>, <step>` — there is no literal "Working" in it, the
spinner glyph carrying that. Scrolling the sheet and reading the raw dump is
what showed the row sitting there, correct.

Fifth time tonight a count asserted a shape the screen never promised, and
the closest to landing: the verdict was already written. The only habit that
has ever caught these is reading the dump instead of the count.

TestFlight changed again at this hour — the sixth number in one session,
which is why no report of mine states one as a fact of its own. The current
build lives in one place only:
`internal/mobile-mac-host-and-testflight.md`.

## Cycle 62 report (02:50 UTC, 09-18)

**Looked at:** the project chat's core path — send, card, streaming, Worked
line — last exercised at cycle 41 and the surface the app is mostly made of.

**It works, and now has numbers** (M-236):

| | |
|---|---|
| send → the card | ~1.0 s |
| send → the reply's first words | **8.9 s** |
| send → `Worked 8s` | **9.8 s** |

The composer resets to `Follow up…` and the kernel holds both the line and
the reply (seq 2364–2366). `media/mobile/cycle-62/`, scenario at
`deploy/mobile/scenarios/the-core-chat-path.sh`.

**One of those three numbers is not what it looks like** (M-237). The card's
`1.0 s` is at the floor of the method: one `ui dump` costs 0.36–0.44 s over
five runs, so a poll loop cannot resolve anything faster than about half a
second, and a card drawn instantly still reads as ~1 s. I measured the
instrument before quoting it, and the floor is written beside the number in
the scenario so nobody later reads it as the app being slow to draw a card
it almost certainly draws at once.

The other two are twenty times the floor and mean what they say.

A number without its resolution is an opinion with a decimal point — and
this loop has spent the week on instruments that claimed more than they
could see.

## Cycle 63 report (03:00 UTC, 09-18)

**Looked at:** the loop's own machine, oldest row at 41, and the claim it
carries — that the harness lives in the repo so no worker's disk is
load-bearing. That claim is the one written after the night this loop nearly
lost its Mac, so it is worth testing rather than admiring.

**It was true of the files and not of the paths** (M-238). `mac-journey.sh`
called four committed tools from `$HOME`: `~/kernel.py` seven times, plus
`~/frame-log.py`, `~/journey-record.py`, `~/push-check.sh`. All four also
live in `deploy/mobile/`. Both copies existed and they drifted.

The cost was paid tonight without anyone noticing: `kernel.py` was corrected
**three times** — the agent filter, the `main`/`root` alias, the race on
unlabelled frames — and **not one of those fixes reached a journey run**,
because the run read the home copy. Side by side on the same agent:

```
via the checkout:  8      the worker's own count, correct
via ~/kernel.py:   2366   the root's count, the bug fixed three times
```

Run 33 is not invalidated — the journey uses `history`, not `total` — but
the next fix that mattered would have gone the same way, silently. #554.

**And it removes an undocumented step from rebuilding the Mac.** The cycle-41
recovery story implies a fresh machine can clone and run. It could not: it
needed someone to know that four files must first be copied into `$HOME`,
which was written down nowhere.

**The shape of it** (M-239). This is "the only copy is not a copy" inverted.
Cycle 41 stopped the loop depending on one disk by committing the tools.
Nobody checked which copy the loop actually *ran*, so the repo copy became
the maintained one and the home copy the used one, and the gap grew quietly
for twenty-two cycles. Two copies with one maintained is worse than one
copy, because it reads as safety.

The cheap check is now written into the row: compare each committed tool's
checksum against the copy the harness invokes.

## Cycle 64 opened (03:10 UTC, 09-18)

**The workers-sheet ask was already shipped, and I re-measured rather than
say so from memory** (M-240). `main` carries
`workerOrder.filter { touched.contains($0) || inTree.contains($0) }` from
#547. Verified by a cold relaunch — terminate, reinstall, launch fresh — and
the pill read **Agents 16** with the sheet listing `count slowly one to
forty` and the four `say sentence about …` workers, every one from an
earlier session. "I already did that" is a sentence this loop has been wrong
about twice tonight, so it now costs a measurement.

**The voice cluster is the next rotation, and its rig is confirmed up**
(M-241). Opening it, my probe connected to the gateway and reported
`frames seen: none in 15s` — which as written is "the gateway accepts
sockets and says nothing", a finding for another team. It was my probe. The
client opens the conversation: `SelfHostedVoiceSession` sends
`session.start` on connect. Sending it first returns

```
session.ready {engine: openai, asr: openai/gpt-live-1,
               tts: openai/gpt-live-1,
               reply: openrouter/google/gemini-2.5-flash}
```

Third time tonight an instrument of mine would have produced a false finding
about somebody else's system, and the same check caught it each time: read
what the real client does before deciding the server is at fault.

**Next:** barge-in (last measured at 42: 500–522 ms round trip, the phone's
own part 2–4 ms) and the microphone path, with the clips in `~/mobile-clips`
and a recording due this cycle.

## Cycle 64 report (03:20 UTC, 09-18)

**Looked at:** what the list says about a machine the hub is holding open
after its last kernel left (#545).

**It said `Off`** (M-242). The hub now sends `online: false` with the
projects `live: false` and an `offline_since_ms`; the app read neither
field, so those projects arrived looking like any idle one. `Off` is true
and says nothing about *which* thing is off — which the row's own comment
already calls the only question worth answering there. The hub had started
answering it and the app was not listening.

The rows name it now:

```
alpha    Idle · 2m
beta     sleepy-box is asleep
gamma    sleepy-box is asleep
phone    arboslife is off
```

That last line is the remembered-project path from #480, and the two read
differently on purpose: *asleep* is a machine the hub is holding open for,
*off* is one it has lost track of. #556, `media/mobile/cycle-64/01-`.

**How it was tested, since the live roster has no sleeping machine.**
Stopping somebody else's kernels to make one is not a test worth running, so
the app was pointed at a small local hub serving a fixture of exactly the
shape #545 documents — checked against the live roster's own fields first.

**And that turned up a second fault** (M-243). The fixture showed nothing
but stale cached rows. `HubClient.list` mapped **only** `ws` to plain http,
so an `http://` hub became `https://` and never answered, while `attachURL`
has always kept `http` and `ws` plain. A plain hub on a local network was
attached to over `ws` and listed from over `https`: its roster never loaded,
and the list quietly showed what it had cached, with no error anywhere.

Nobody would have met this on the trycloudflare hubs this loop uses, which
are all `wss`. It waited for the first plain hub — which happened to be a
test fixture rather than one of Jacob's machines, which is the good version
of finding out.

## Cycle 65 report (03:15 UTC, 09-18)

**Looked at:** barge-in, oldest of the voice cluster at 42, with the
recording that was due.

**It is a third of what it was** (M-244). Three runs on the current build
against the live gateway; two armed:

| | cycle 42 | now |
|---|---|---|
| barge → `speech.started` | 500–522 ms (four runs) | **137 ms**, **185 ms** |
| barge → `response.done` | — | 179 ms, 227 ms |

Cycle 12 measured 401 ms, so this is the best it has been. The voice team's
Silero/Whisper work reported 232–243 ms in their own numbers; the phone sees
better than that now.

**Reported as two of three, not averaged over three.** The third run
produced no barge metric at all — its reply's first audio came at 754 ms and
again at 1478 ms, so the clip that fires 1.5 s in probably landed in a gap
rather than over a reply. A run where the thing never happened is not a slow
reading of it, and folding it in as a zero or dropping it silently would
both be wrong.

**Connect wants one careful look** (M-245). The same three runs: 5773 ms,
1849 ms, 1430 ms, against 1017 ms recorded at cycle 52. The shape is a cold
first call after a fresh boot settling to 1.4–1.8 s. Whether the settled
figure has drifted is a separate question that three runs cannot answer, and
I am not reporting a regression I have not isolated.

**Recording:** `media/mobile/cycle-65/recording-call-and-barge-in-38s.mp4`,
with seven stills across the call.

## Cycle 66 report (03:25 UTC, 09-18)

**Looked at:** the microphone path, oldest of the voice cluster at 42, whose
claim is a frame count at both ends through `-micWav`.

**The first run was 13 frames short**, and stayed exactly 13 short the whole
way — 800/787, 900/887, 1050/1037. A single loss at the start, not a leak.
Cycle 42 measured no loss at all, so this looked like a regression in the
capture path.

**It is not.** Two more runs settled it:

| connect | frames |
|---|---|
| 2633 ms | 1050 clip / 1037 sent — **13 lost** |
| 1514 ms | 900 / 900 — none |
| 1936 ms | 900 / 900 — none |

The hold that covers the connect is **two seconds** of audio, oldest dropped
first. Under two seconds nothing is lost; over it, the opening of what was
said goes. The microphone path and the hold both work exactly as designed —
what changed is connect (M-245).

**And it retires a comment's premise** (M-247). The hold reads "anything
held for longer than that has said nothing to lose". That is sound when the
delay is the hold. The delay here is connect, and the audio being discarded
is the user's first words, which have plenty to lose. The assumption was
true when connect ran 600–770 ms at cycle 42; it is not at 2633 ms.

The fix is one of two things and neither is mine to pick alone: widen the
hold, which costs memory and keeps staler audio, or make connect reliably
under two seconds. Named and cross-referenced from M-245, so whoever looks
at connect sees what it costs — not a number, the first words of a sentence.

This is the chain the loop exists to find: a timing drift in one component
quietly eating the opening of every slow-connecting call, with no error
anywhere and the transcript still arriving well enough to look fine.

## Cycle 67 report (03:30 UTC, 09-18)

**Did the thing M-247 named rather than leaving it named.** The hold that
covers the connect goes from two seconds to six.

The old window's reasoning — "anyone silent for longer has said nothing to
lose" — assumed the wait belonged to the hold. It belongs to the connect,
and what was being discarded is the opening of a sentence already spoken.

| | connect | frames |
|---|---|---|
| before | 2633 ms | 1050 clip / 1037 sent, **13 lost** |
| before | 1936 ms | 900 / 900 |
| after | 2140 ms | 1000 / 1000 |
| after | 2096 ms | 850 / 850 |
| after | 1759 ms | 850 / 850 |
| after | 1578 ms | 850 / 850 |

Two of the four "after" runs sit above the old two-second boundary and would
have lost frames. #557.

**What is not claimed.** A connect in the 2.6 s range did not recur across
those four runs, so the exact 2633 ms failure is covered by the window being
three times wider rather than by re-observing that case pass. The mechanism
is understood and the boundary has moved — but I did not see the failing
reading turn green, and that distinction is the difference between a fix and
a hope.

Connect-time drift stays a named look, as instructed. Widening the hold
means a slow connect no longer costs the user their first words, which was
the part doing harm.

**The mesh worker reached M-227's conclusion independently** (M-249), from
the other end of the same roster:
`internal/last-activity-ms-on-the-live-roster-2026-09-18.md`. Same finding —
the field is kernel-fed, absent means the kernel predates `11a01d84` — with
what I could not see from here: the hub's binary verified by md5, each
kernel's build named, and `phone` deliberately left alone while Jacob was
mid-turn, then moved when idle. The phone already renders absent as unknown,
which is what that note asks for. Cross-referenced both ways.

## Cycle 68 report (03:40 UTC, 09-18)

**Looked at:** first word and transcription, oldest at 43 — and its open
item, M-146.

**The first word is fine.** Driven down the capture path, `Hey!` and the
full status question both arrive whole, 900 of 900 frames sent. That half of
the row holds, and the six-second hold that shipped in #557 protects it on a
slow connect now too.

**M-146 is still open, and today it is cleanly isolated** (M-251). One
request cut in half by a pause comes back as **two transcripts and two
spoken answers**, both `response.done reason=completed` with audio played.
The caller hears two replies to one question.

The phone is provably not the cause: 900 frames captured, 900 sent, no loss
at the socket. The stream the gateway received was continuous and the split
is its own segmentation of it. That mattered enough to establish before
filing, because this family has been ours before — cycle 42's first-word
loss was the phone's, and #557 landed today for a related reason.

Re-filed for the gateway with a reproduction:
`internal/features-inbox/2026-09-18-one-question-two-answers-across-a-pause.md`.
Re-filing rather than pointing at the old ledger row because the original
predates both the Silero/Whisper change and the new hold, and either could
reasonably have been assumed to have closed it.

A pause mid-question is how people talk. The failure is not a wrong answer;
it is being answered twice, the second arriving over the first, with the orb
settling and re-firing as though the app had lost track.

## Cycle 69 report (03:55 UTC, 09-18)

**Looked at:** voice notes in the composer, oldest at 44 — and the
recording that was due at 68.

**The dictation promise holds, and is now repeatable** (M-252, M-254). The
claim inherited from cycle 44 is that dictated words sit in the field and
nothing reaches the kernel until he sends. That is a claim about the socket,
not about the screen, so it is counted on the kernel's own transcript at
three moments. Two runs tonight, hours apart:

| | before | words waiting, unsent | after his send |
| --- | --- | --- | --- |
| first run | 2411 | 2411 | 2415 |
| second run, recorded | 2415 | 2415 | 2419 |

Cycle 44 read 1526 → 1526 → 1531 on a different build. Three measurements,
three builds, the same shape. The field held `Please summaries what the
workers did today in two sentences.` with `Up` beside it, and the send was
his.

The second run exists because one before/after pair cannot tell a promise
kept from a kernel that happened to be quiet for eight seconds.

**A fault in my own rig, caught before it could lie** (M-255). The scenario
took the project row as an argument and then counted `pod`'s transcript
regardless. It agreed only because `phone` and `pod` are one kernel drawn
twice (M-121) — so it would have gone on being right by luck until someone
pointed it at another project, where it would have reported the promise kept
while measuring a kernel the dictation never touched. The row and the
counted kernel are separate arguments now, and an unreadable count stops the
run rather than printing numbers. This is the same family as M-183, M-186
and M-224, and it is the fault this loop pays for most often: not a wrong
answer, but a right-looking answer to a question nobody asked.

**The phone row draws its time** (M-253). The mesh side moved that kernel to
`c3247332` and the row now reads `phone · home · 2m`, beside `demo 7h` and
`subnet120 1h`. This is M-227's rule playing out for the fourth and last row
the mesh side owns, exactly as predicted, so there is nothing to fix. The
three still silent — `const`, `parity-proj…`, `qa-cycle-11-demo` — are
Jacob's desktop's, the parity loop's and QA's kernels, and stay correct as
unknown rather than as idle.

**Recording** (due at 68, one cycle late): the dictation flow end to end,
53 s —
`media/mobile/cycle-69/recording-voice-note-waits-then-sends-53s.mp4`.
The words arriving in the field, waiting there with the send arrow beside
them, and going only on the tap. The recording is the illustration; the
counts above are the evidence, and they disagree about nothing.

**Stills:** `media/mobile/cycle-69/` — `01-the-words-wait-in-the-field.png`,
`02-phone-draws-its-time.png`, `03-listening.png`,
`04-the-words-wait-unsent.png`, `05-sent-on-his-tap.png`.

**PR:** [#561](https://github.com/unarbos/arbos/pull/561) — harness only, no
app change. Nothing in the app needed one this cycle.

**Housekeeping.** The build on Jacob's phone is the steward's, and it now
lives in exactly one place — `internal/mobile-mac-host-and-testflight.md` —
rather than being restated in each report, because restating it is how this
loop produced six different numbers in one session. That rule applies to
this sentence too, so the number is not repeated here. The kernel CI flakes stay filed as evidence in
`internal/kernel-ci-flakes-2026-09-18.md` and are not this loop's to fix.

**One more, found by closing the cycle properly** (M-256). Mirroring the
ledgers to the Mac reported the Mac's `mobile-findings.md` at **77976 b**
against the store's 137631, and `mobile-coverage.md` at **12712 b** against
21476. The rule since M-89 is to mirror after every write, and recent cycles
wrote and did not. So for several cycles the only backup of about sixty
kilobytes of findings did not exist, on a store that has reverted these
files three times. Current again now. The lesson is not "mirror harder": it
is that the mirror already prints both sizes, and reading that one line at
each cycle close would have caught it the first time.

## Cycle 70 report (04:40 UTC, 09-18)

**Looked at:** the two oldest rows — hub refusals, last exercised at 45, and
network drop at 49 — which share a fixture and a rule.

**The rule from cycle 49 holds, and is finally measured on a device**
(M-257). That rule is: a refusal is a verdict, so say the reason and stop; a
transport failure is the path, so name it and keep trying. Its refusal half
had never been driven on a device (M-165), because the live hub cannot be
made to refuse to order. A fixture serving `Hub::kernel`'s three refusals
verbatim, plus a 502 at the tunnel, settles it by counting attaches:

| the hub says | the phone says | attaches in 25 s |
| --- | --- | --- |
| no machine named "ghost-box" | `ghost-box is not connected — its kernel isn't running, or the machine is off.` | **1 — stops** |
| fixture-box is offline: … | `fixture-box has been off for 2 hours. It comes back when a kernel starts on it.` | 3 — retries |
| has no kernel serving "no-kernel" | `no-kernel's kernel on fixture-box isn't running.` | 3 — retries |
| 502 at the tunnel | `127.0.0.1 could not be reached — retrying` | 3 — retries |

Counting is what makes this a measurement. Retrying and stopping are claims
about behaviour over time, and no screenshot can tell a socket that has
stopped from one that is about to come back.

**Four things were wrong, and all four are in [#566](https://github.com/unarbos/arbos/pull/566).**

The reason was erased a moment after it arrived (M-258). A refusal is two
events — the hub's words, then the socket going — and only the first has
any. The second overwrote it, so a sentence naming exactly which kernel was
not running became `Link lost` before a person could read it.

An offline machine matched no pattern (M-259), so it printed the hub's
record of itself — prefix, RFC3339 timestamp, the project it used to serve
and the remedy — in red, once per retry. The hub's own source calls this the
commonest refusal there is.

The app guessed about the path beside an answer (M-260). The hub sends its
reason and then closes bare, so every refusal also looks like transport, and
every refused chat carried `127.0.0.1 could not be reached` above the hub's
explanation — on a host it had plainly reached and been answered by. This is
#417's problem one layer on: #417 made the reason arrive, and the bare close
still invents a second story about the same event.

And an empty chat said it was ready one line above why it was not (M-261).

**Two of my own faults, caught before they became reports** (M-262). The
first fixture paraphrased the hub, putting the project where the machine
goes, and the app's translation — correctly taking the first word as the
machine — came out as `no-kernel's kernel on Project isn't running.` I was
one step from filing that as an app bug. The second draft put the reason in
the close frame, where the hub sends none, and its longest reason overflowed
the 125 bytes a control frame allows, so the app saw a broken socket and was
right to retry it. Both drafts read the app as losing reasons it had never
been sent.

Third time in this loop a rig has invented the fault it reported (M-162,
M-183, M-224). The habit that works is cheap and I will keep it: copy the
other end's strings out of its source, verbatim, and say in the fixture that
they are verbatim so the next reader does not improve them.

**Stills:** `media/mobile/cycle-70/` — the fixture list and one per case.
**PR:** [#566](https://github.com/unarbos/arbos/pull/566), which also carries the scenario.

## Cycle 71 report (05:05 UTC, 09-18)

**Looked at:** the projects list's search, filter and refresh — the oldest
row left, last exercised at 47.

**The row holds, and now has numbers rather than screenshots** (M-263).
Against a four-project fixture, every claim counted off the accessibility
tree:

| | rows |
| --- | --- |
| at rest | 5 — alpha, beta, beta-two, gamma, pod |
| typing `beta` | 2 — beta, beta-two |
| typing `zzzz` | 0, and the screen says `No project matches “zzzz”.` |
| cleared | 5 again |
| Live only | 3 — alpha, beta, pod |
| All projects | 5 |
| pull to refresh | `/list` calls 2 → 3, and `arrived-late` reached the screen |

Live only showing three is right: the fixture serves two live projects and
the pod's own row is a direct kernel that needs no hub. M-191's no-match
line holds. Nothing in the app needed changing for this row.

**What did need changing was everything around it** (M-264). Every round
button in the app announced its icon rather than its purpose, because
SwiftUI reads the SF Symbol's name when there is no label: settings was
**"Gear Shape"**, and the project glyph in the chat header read **"Move"**
next to the word `pod`. Worse, a button inside a `Menu` gets no name at all
— the `Menu` presents itself and the child's label never reaches the tree —
so the list's filter and the chat's overflow were both **"PopUpButton"**.

That last one has a cost this loop has been paying: the harness has tapped
the chat's overflow **by coordinate since cycle 56** because there was
nothing to ask for. Both audiences are served by the same fix. VoiceOver
stops reading icon names, and `ui menu Filter` now opens the list's filter
by name. [#571](https://github.com/unarbos/arbos/pull/571).

**Two faults in my own rig, and both would have been reports.**

A count of rendered rows is not a count of the list (M-265). The list is a
`LazyVStack`, so a row below the fold is never built and `describe-all`
cannot see it. My first run read 11 rows at rest and 7 after clearing the
search, and I was reading that as four projects failing to come back. They
were under the keyboard.

And the refresh test's answer depended on how many times the app had
happened to ask (M-266): the fixture added its new project from the fourth
`/list` on, so one run saw it and the next did not, and the second read as
the app failing to refresh. The scenario now tells the fixture when to add
it, and the fixture logs every call, so the two questions are asked
separately — did the gesture make the app ask again, and did the answer
reach the screen.

That is the fourth and fifth rig-invented fault in this loop's life (M-162,
M-183, M-224, M-262), two of them tonight. The pattern in all five is the
same: the rig quietly answers a different question from the one asked, and
the answer looks like an app bug. The only habit that has ever caught them
is checking the other side — the view's own code, the hub's own source —
before writing the finding down.

**Stills:** `media/mobile/cycle-71/`.

## Cycle 72 report (05:40 UTC, 09-18)

**Looked at:** the acceptance journey itself, last run at cycle 55 — the
loop's spine, and by far the most overdue thing on the board.

**Run 33 is healthy** (M-267). Against `pod` on kernel `c3247332dc4e`:
**14 pass, 2 eye, 4 unverified, no failures.** The setup line and its
seeded reply, the challenge and its worker, a clean turn end, the result
verified on disk through the kernel's own read frame — CHANGELOG present,
`area = w * h`, unittest OK, a branch with commits — then dictation, the
photo the model actually described, and the call's question landing in this
project's transcript.

J8c deserves its own line: the link was cut for **25 seconds mid-turn** and
the turn finished after it came back.

The four unverified are the standing ones and none is new: a hosted kernel
cannot be restarted from the phone, J4's mid-flight half, Stop at J5, and
PUSH until the APNs key exists.

**And the run handed me the measurement cycle 67 could not get** (M-268).
M-248 widened the pre-socket hold to six seconds and said plainly that the
failing reading had not been seen to turn green — none of those four runs
connected slowly enough to be the case. Six runs tonight, counted at both
ends:

| connect | clip | sent | lost |
| --- | --- | --- | --- |
| 2060 ms | 650 | 650 | 0 |
| 2108 ms | 650 | 650 | 0 |
| 1999 ms | 650 | 650 | 0 |
| 1472 ms | 650 | 650 | 0 |
| 1493 ms | 650 | 650 | 0 |
| 1282 ms | 650 | 650 | 0 |

Two are past the old two-second boundary and would have lost audio before
#557. And the journey's own call connected at **2635 ms** — M-246's exact
magnitude — with P3 passing.

**What I am not claiming.** None of the six counted runs reached 2633 ms.
The exact magnitude is covered by the journey's pass, which carries no frame
count, and not by a count of its own. Two readings pointing the same way is
weaker than one reading of the thing itself, and I caught my own scenario
about to print a verdict that blurred them — it now counts runs at M-246's
size separately from runs merely past the old window.

**Two harness faults, both of them claims made under this loop's name.**

The journey has been telling QA that the phone has no Stop control (M-269).
That string sat in J5's scorer, stated as fact, in every run since it was
written. The phone has one: the composer's stop square, scored at cycle 40
and labelled `Stop` in the tree, absent at J5 only because that turn has
already ended. QA imports these verdicts, which makes a wrong scorer string
a published claim rather than an internal note.

And the mic-path measurement existed only in one evening's shell history
(M-270). Cycles 66 and 67 did all that counting with ad-hoc commands that
were never committed, so closing M-248 tonight began by rebuilding the rig
from scratch. That is M-133 and M-238 for the third time: the finding
survives, the means of re-checking it does not.

**PR:** [#572](https://github.com/unarbos/arbos/pull/572), harness only.
**Evidence:** `media/mobile/journey/0918-051151/` — stills per step, the
console, the score, the run record and a recording of the challenge and its
workers.

## Cycle 73 report (05:55 UTC, 09-18)

**Looked at:** the workers sheet's live Working row — the row cycle 61 tried
four times to catch and never did.

**It is caught** (M-271). With a `sleep` worker running, the sheet draws it
with the braille spinner and its step: `⠋, sleep 150 seconds, running sleep
150`, and on a later run `⠙, sleep 300 seconds, sleeping 300 seconds`. The
chat's own line is right alongside — `1 Working sleep 296 seconds · sleeping
296 seconds`. The surface works, and shows what the desktop shows.

**One question I am leaving open rather than answering badly** (M-272).
Both sightings were of workers that had been running for minutes. On the one
run that started from a quiet project and opened the sheet **18 seconds**
after the send, the sheet — scrolled to its end — held twelve rows and no
live one, while the chat behind it named the worker correctly. And every
run, in every state, shows exactly **twelve** rows.

Three explanations fit: the sheet lags its first publish, it caps at twelve,
or it orders a new worker outside what is rendered. I have separated none of
them, so I have filed it as a question with the constant twelve as the
thread to pull, and changed no app code. Nobody is left uninformed in the
meantime — the chat's line is live and correct throughout. It is the sheet's
promptness in doubt, not the app's honesty.

**Two faults in my scenario, and both are repeats of faults this loop has
already paid for.**

It counted spinner rows from one screen of a scrolling list (M-273) and
reported that the sheet showed no spinner on any row, with the live row
present and below the fold. That is cycle 71's mistake exactly (M-265), two
cycles later. Only rendered rows reach the tree, so "it is not there" taken
from one screen means "it is not on this screen".

And it scored on the previous run's worker (M-274). The rerun read `sleep
150 seconds` as its live row while that run had asked for 300 — the worker
still going had started two minutes earlier. The pill lit within four
seconds for the same reason, so the wait was satisfied by somebody else's
work and the sheet was read before this run's worker existed. That is
M-160's fault in a new place: evidence from an earlier run read as this
one's.

Both are fixed, and the second is fixed at the right level — the run now
waits for the project to be quiet before it sends, rather than detecting
contamination after the fact.

**PR:** [#574](https://github.com/unarbos/arbos/pull/574), harness only.
**Stills:** `media/mobile/cycle-73/`.

## Cycle 74 report (06:05 UTC, 09-18)

**Looked at:** the worker chat against Cursor's agent chat, last paired at
46 — and, because it was one tap away, the sheet that opens it.

**The pairing holds** (M-276). Plain header with a back chevron, a tick and
the worker's name; collapsed tool rows (`read · project-context.md`,
`read · notes.md`, `status`) with disclosure chevrons; flat surface, no
bubbles; no composer, which is M-154's deliberate choice rather than an
omission — the tree carries no `TextField` at all. The header now reads
`Back` instead of a symbol name, after #571. The chat has real content, plan
and tool rows and a Done report, which answers M-225 again from a different
direction.

**And one tap away, the sheet disagrees with the app about itself**
(M-275). The chat's pill reads `Agents 19`. The sheet it opens, paged to its
end, holds **12** rows. No two labels are the same, so the 12 is a count of
distinct rows rather than an artefact of truncated goal text.

This changes the shape of cycle 73's open question rather than answering it.
The run that saw no live row 18 s in may not have been watching a sheet that
lags; the sheet is short by seven against the app's own count, and a worker
that has only just started is a plausible one to be missing. I have not read
`publishWorkers` against this and I am not changing app code on a number I
cannot explain yet, so what ships is the comparison itself:
`deploy/mobile/scenarios/pill-count-vs-sheet.sh`, one command instead of a
reconstruction.

**A fault of mine inside that scenario, fixed in its own commit.** Its
duplicate-label check compared across all ten pages, where every row repeats
by construction, and reported twelve duplicates for a list with none. Two
workers can only hide each other where both are on screen together, so the
check has to be within one dump. Left uncorrected it would have made the
19-versus-12 gap look explained.

**PR:** [#576](https://github.com/unarbos/arbos/pull/576), harness only.
**Stills:** `media/mobile/cycle-74/`.

**On cycle 72's red kernel job:** that branch changes two shell files and no
Rust, and `macOS (check kernel + desktop)`, which compiles the kernel,
passed. It is the flake already filed in
`internal/kernel-ci-flakes-2026-09-18.md` and not this loop's to fix.

## Cycle 75 report (06:20 UTC, 09-18)

**Looked at:** background to resume, the oldest row left, half answered
since cycle 27.

**The resume half still holds** (M-277): Home, 120 seconds, back — the same
screen, and the last three lines identical by md5. M-167's result again on a
build twenty-five cycles later.

**The other half was never examined, and did not need Jacob's phone**
(M-278). The row has said "hours-long suspension still needs Jacob's phone"
since cycle 27, and that is true of one case and not of the other. Hours is
two things: iOS keeping the process, which is the answered case for longer,
and iOS **reclaiming** it, which is what actually happens overnight. In the
second, coming back is a cold launch — and reproducing that needs no hours
at all, only the process gone, which is one command.

Measured: left in the chat, process killed, relaunched — he lands on **the
projects list**, not the chat he left.

**No app change, deliberately.** The app knows which project he was in;
`settings.kernelTarget` persists and is what the list's composer uses to
name one. So the list is a choice the code makes rather than a limit it is
under, and both answers are defensible — continue where he was, or show the
roster after a night. The desktop persists and restores workspace state, but
I established only that it restores panels and tabs *within* a project, not
that it reopens the active one. Changing navigation on half a comparison is
the exact shape of fault this loop has paid for repeatedly, so the question
is filed for a decision:
`internal/features-inbox/2026-09-18-where-should-the-phone-put-a-returning-user.md`.

Worth naming the general point, because it applies beyond this row: "needs
Jacob's phone" had been treated as covering the whole of hours, and it
covered half. A blocked row is worth re-reading occasionally for the part
that is not blocked.

**PR:** [#578](https://github.com/unarbos/arbos/pull/578), harness only.
**Stills:** `media/mobile/cycle-75/`.

## Cycle 76 report (06:25 UTC, 09-18)

**Looked at:** the call's words read back in the project chat, oldest at 54
— the feature Jacob specified himself.

**His rule holds** (M-279). A delegated turn is answered twice and the two
answers differ, which is exactly what makes the rule checkable:

| | |
| --- | --- |
| the kernel wrote | `This project (poems, sorting algorithms, and thirteen mathlib challenges…) is fully done, tested, and committed…` |
| the chat shows | `Everything's complete and committed on its branches; no work is in progress.` |

Six rows marked `Spoken`, his own halves among them, and the kernel's
wording absent from that turn. The scenario used to take stills and leave
the comparison to whoever opened them; it now takes a distinctive run of the
kernel's words and requires it to be absent from this turn's reply.

**A fault in that check, caught only because I had already seen the
screenshot** (M-280). The first version searched the whole screen for the
kernel's phrase, found it, and reported that both wordings were showing. It
was there legitimately — spoken rows do not survive a restart, so older
turns replay as the kernel's text, which M-179 recorded when the rule
landed. Scoped now to the rows after the last `Spoken` marker.

Worth being plain about how close that came: without the screenshot in my
hand I would have filed a regression against a rule that was working
perfectly. The rig's answer was true; it answered a question nobody asked.
That is the sixth time tonight, and the only defence that has ever worked is
holding a second, independent view of the same thing.

**A third slow connect, in passing** (M-281). This run connected at
**2724 ms**, above M-246's 2633 ms, and both turns completed with audio.
With the journey's 2635 ms at cycle 72, two connects past that magnitude
have now carried a whole conversation. Neither carries a frame count and the
counted rig's slowest run is still 2108 ms, so it is supporting evidence and
recorded as such — three observations pointing one way, none pointing the
other.

**PR:** [#579](https://github.com/unarbos/arbos/pull/579), harness only.
**Stills:** `media/mobile/cycle-76/`.

## Cycle 77 report (06:30 UTC, 09-18)

**Looked at:** voice barge-in, and the recording due on the every-third rule.

**Barge-in holds** (M-282). On current `main`: `barge_in_speech_started
181 ms`, `barge_in_response_done 223 ms`. That sits inside cycle 65's pair
(137/185 and 179/227 ms) and far below cycle 42's 500–522 ms. Connect
1888 ms, first reply audio 521 ms. The recording is 30 s at
`media/mobile/cycle-77/recording-barge-in-181ms-30s.mp4`, verified by md5
against the file on the Mac.

**A DEBUG metric called its own success a failure** (M-283). The run's tail
read `metric barge_in_unarmed clip=false injector=true`, and I took it —
for about a minute — as the barge having failed to arm, which is the case
M-244 saw once. It had armed, fired and been measured twenty lines earlier.

There is one barge clip per run, spent on the first reply, so every later
reply prints that line, and the wording gave the spent case and the
never-supplied case the same name. It now says which: `clip already used`,
`no clip given`, `no injector`. [#580](https://github.com/unarbos/arbos/pull/580), DEBUG only.

Two things worth keeping from that. The fault was in the *words* and the
measurement was perfect, which is a different failure from the rig faults
earlier tonight and needs a different guard — reading a metric's name as if
it were a verdict. And I was exposed to it by reading the tail of a log
rather than the log: everything I needed was in the middle.

## Cycle 78 report (06:35 UTC, 09-18)

**Looked at:** the coverage table itself, against what journey run 33
actually exercised.

**Three rows were understating themselves, by as much as eighteen cycles**
(M-284). The rotation takes the oldest rows first, so a table that
undercounts sends the loop back to work it did the cycle before:

| row | said | run 33 actually exercised |
| --- | --- | --- |
| the harness — typing into the composer | 54 | seven typed lines sent, seven reached the kernel |
| journey — the phone-only steps P1, P2, P3 | 56 | all three passed |
| attachments (`+`), photos, files | 60 | the photo attached and the model described it |

**Credited narrowly, on purpose.** The attachments row now reads 72 *for the
photo reaching the model*, and leaves the chip's ×, files and the mic-to-send
swap at their cycle-60 reading, because the journey does not touch them.
Dating a whole row from one step of a journey would swap one wrong number
for another, and this row already has a history of sending readers to the
wrong place.

That is the second time a coverage row has misdirected the loop — the
style-pair row at cycle 58 had its cycles and its still numbers in each
other's columns. Both faults have the same cause: the table is written by
hand at the end of a cycle, when the run's detail is freshest and the
temptation to summarise is strongest. Worth watching for a third.

**No PR.** Nothing in the repository needed changing, and inventing a commit
to satisfy the one-per-cycle habit would be the same kind of bookkeeping
fiction this cycle is correcting.

## Cycle 79 report (06:45 UTC, 09-18)

**Looked at:** connect-time drift, a standing look since cycle 66 and never
isolated.

**It is not drift** (M-285). It is two engines, and the answer was already
in the record: every call run the loop has done printed its connect time and
its engine into a console log, and 138 of those logs are still on the Mac.

| engine | n | min | median | p90 | max | mean |
| --- | --- | --- | --- | --- | --- | --- |
| duplex | 101 | 284 | 543 | 732 | 898 | 526 |
| openai | 37 | 999 | 1936 | 2635 | 5773 | 2109 |

The two do not overlap at all: the slowest duplex connect is faster than the
fastest openai one. Of the seventeen connects over two seconds, all
seventeen are openai; of the 102 under a second, 101 are duplex.

The numbers appeared to grow across the cycles because the phone moved to
GPT Live as its gateway partway through. The engine changed underneath the
measurement, so the record read as one population wandering. That is why it
survived thirteen cycles as a look: nobody had grouped it, and no new
experiment was ever needed.

**The grouping also shows something nobody had written down** (M-286). The
pre-socket hold is 6000 ms. The worst connect in the whole record is
**5773 ms** — a margin of **227 ms**, under four per cent.

Six seconds was chosen at cycle 67 against that exact reading, so this is a
decision already taken and not an oversight. But a margin that thin,
recorded nowhere, is a trap for whoever meets it next, and a connect
slightly worse than the worst yet seen costs the caller their opening
words. I have not widened it on my own; the scenario prints the margin on
every run instead.

**A fault of mine, in its own commit:** the overlap test unpacked a
four-tuple into two names and threw — after the table had already printed,
which is the only reason I saw the numbers before the traceback.

**PR:** [#582](https://github.com/unarbos/arbos/pull/582), harness only, and it takes no new
measurements.

## Cycle 80 report (07:00 UTC, 09-18)

**Looked at:** the diagnosis I deferred at cycle 74 — the pill counting 19
agents while the sheet listed 12.

**The app was right and my scroll was broken** (M-287). Re-measured with the
gesture inside the screen: the pill says 19, the sheet pages out at 19, they
agree. The kernel's tree, asked directly, holds fifteen children of root
with fifteen distinct names, so no duplicate labels were hiding rows either.

The mechanism is worth more than the correction. Both scenarios I wrote this
session swiped from **y=900 on a screen 852 points tall**. Nothing errors:
the gesture lands nowhere, the list never moves, and every "page" re-reads
the first screen — so a list of nineteen reports as twelve, ten times over,
with perfect consistency. The screenshot is 1024 px tall and the screen is
852 pt, which is the pixels-versus-points trap `pt()` was written for at
M-152, resurfacing in new code that did not use it.

Fixed where it cannot recur: `page_up()` in `sim-lib.sh` computes the
gesture from `SIM_PT_H` and takes only the udid. Every older scenario was
checked and they all stay under y=850.

**What that costs the ledger** (M-288). M-275 is withdrawn outright. M-272 —
"the scrolled sheet held 12 rows and no live one" — is qualified down to
nothing measured, because that sheet was never scrolled either; whether a
brand-new worker reaches the sheet promptly is untested rather than
doubtful. What stands from cycle 73 is M-271: the live row exists, caught
twice, with its spinner and step.

**The part I want on the record.** Three findings this session rested on one
broken gesture, and all three read as *the app being wrong*. That is the
direction this loop's rig errors always point — never once has a broken
measurement flattered the app — and it is the cheapest available warning
sign. A measurement that makes the app look bad deserves the second look
that a flattering one would never get.

**PR:** [#584](https://github.com/unarbos/arbos/pull/584).

## Cycle 81 report (07:15 UTC, 09-18)

**Looked at:** the list composer, oldest row at 57.

**All four of its claims hold** (M-289), measured off the tree rather than
looked at:

| claim | reading |
| --- | --- |
| at rest it names a project on screen | `Message phone…`, and `phone` is a row |
| a filter moves it | `sub` → `Message subnet120…`, the only row left |
| an empty filter empties it | `Plan, ask, build…`, and the screen says why |
| it rides above the keyboard | composer top **480 pt**, keyboard top **568 pt** |

The 480 is the same number cycle 57 recorded independently twenty-four
cycles ago, which is a pleasant check on both runs.

**Two bad reads in the fourth measurement** (M-290). It took the *first*
TextField — the search box at 177 pt, not the composer at 490 — and looked
for elements of type `Key`, which do not exist; the keyboard arrives as the
`GenericElement` covering the bottom of the screen. Both came from guessing
at the tree instead of dumping it once and reading what is in it.

The thing worth keeping is how it failed. It printed **"could not read both
numbers — inconclusive"** rather than a confident wrong number. That is the
same class of mistake as the off-screen swipe an hour ago, and the
difference is that this one was caught by the code. A check that cannot get
its inputs should say so; that is cheap to write and it is the only reason
this did not become another withdrawn finding.

**On the kernel CI red** on the connect-drift branch: one shell file, no
Rust, and `macOS (check kernel + desktop)` passed. Third time tonight in
that shape, and each time the failed log is gone by the time I look because
a re-run has replaced it. The flake file stands as the evidence and this is
not the loop's to fix.

**PR:** [#586](https://github.com/unarbos/arbos/pull/586), harness only.
**Stills:** `media/mobile/cycle-81/`.

## Cycle 82 report (07:30 UTC, 09-18)

**Looked at:** notifications, oldest row at 58.

**The path still works end to end** (M-291). Reinstalled so the permission
decision is fresh: the ask appears and is allowed, the reply posts a banner
about two seconds after going away, and tapping it lands in the `phone`
chat. The console agrees — `notify 432 reply state=2 unseen=1`,
`banner 432: posted`.

The still is better evidence than usual because it carries both halves at
once: the banner reading `phone · root replied — OK`, and a red **1** on the
Arbos icon behind it. Real pushes still wait on Jacob's APNs key; this is
the local path.

**A before-and-after pair with no difference in it** (M-292).
`03-home-waiting` and `04-home-with-the-banner` came out byte-identical, md5
`f608d57b…`. The banner arrives about two seconds after going away and the
before-shot was taken at four, so both had it.

Nothing was wrong with the app and nothing was misreported — but a pair of
stills offered as before-and-after is a claim, and that one was quietly
false for however many runs it has been in. Fixed, and renamed
`03-home-before-the-banner` so its job is on its face.

That is the fourth small thing tonight where the evidence was weaker than it
looked while the app was fine. They are worth as much attention as the
app's own faults: a loop whose evidence drifts stops being able to tell when
something real breaks.

**PR:** [#587](https://github.com/unarbos/arbos/pull/587), harness only.
**Evidence:** `media/mobile/cycle-82/` — the banner-and-badge still and a
recording of going away and coming back.

## Cycle 83 report (07:45 UTC, 09-18)

**Looked at:** the style pair, oldest row at 59 — the projects list against
Cursor's agents list.

**They agree on shape, and now there is a number for it** (M-293):

| | ground | row pitch |
| --- | --- | --- |
| Arbos | `(22, 21, 20)` | 223 px of 2556 — **8.7%** |
| Cursor | `(247, 247, 247)` | 227 px of 2736 — **8.3%** |

Within half a percentage point, on phones of different sizes. Structurally
each element has its counterpart: round buttons at both top corners, a
title, a collapsible section header, rows of glyph and name with the age
right-aligned and a second line of state · folder, and a composer pill with
`+`, placeholder and mic.

The two grounds are nothing to reconcile — `#161514` is the literal the
desktop's `palette.rs` uses, and dark-only is deliberate on both sides
(M-202).

**The style rows were the last thing this loop judged by eye** (M-294).
Forty cycles have moved nearly every other row onto counts, and "the two
surfaces agree on shape" stayed a verdict from looking at two pictures. Eyes
are especially bad here: a row ten per cent taller reads as fine beside a
reference, and a ground two shades off reads as identical.

`deploy/mobile/style-pair.py` prints both grounds and both row pitches as a
share of screen height. It compares only pitch, and refuses to compare
colour between a light reference and a dark app — which is the mistake
anyone would make first, and the reason the refusal is written into the tool
rather than left as a note.

**PR:** [#588](https://github.com/unarbos/arbos/pull/588), harness only.
**Still:** `media/mobile/cycle-83/01-the-list-against-cursors.png`.

## Cycle 84 report (08:00 UTC, 09-18)

**Looked at:** two video reviews of recordings I had already filed. Both
were worth asking for; one was wrong and one found a real fault in the app.

**The dictation review was wrong, and cycle 69's finding stands** (M-295).
It reported that the dictated words never reach the composer and appear in a
pending bubble in the transcript. They reach the composer: the code writes
dictation into `draft`, which is the `ComposerBar`'s own `TextField`, and
the still shows the words in the composer pill with `+` at its left and the
up-arrow at its right.

The misreading is worth understanding rather than waving away. The composer
is a rounded multi-line pill that looks like a bubble, and the scenario
dictates a sentence **it has already sent in an earlier run**, so the
identical words are sitting in a real bubble a few inches above. Anyone
without the code in front of them would read it the same way. The scenario
should dictate something it has not said before.

**The barge-in review found the app's fault, by asking about the video**
(M-297). It reported that the recording shows an orb changing colour with no
captions, no transcript and no status words, and asked what someone who
could not hear the call would have.

The answer was: nothing. `CallView` carried no accessibility label anywhere.
The screen is wordless in a call *by design* — `hint` returns nil for
connecting, listening, thinking and speaking — so colour was the only
carrier of state, and colour carries to one kind of person. The discs were
unnamed too.

Fixed without touching the design: the orb is a button labelled `Call` whose
value is the phase, from the `Phase.label` that already existed, and
`CallDisc` gained a label as `RoundButton` did in #571 — `Call menu`,
`Settings`, `Mute`/`Unmute`, `End call`. No text is drawn.

**And my own claim was too strong** (M-296). I filed that recording as a
demonstration of barge-in. It demonstrates very little: the thing being
shown is inaudible and unwritten. The counts are the evidence — 181 ms and
223 ms — and I should have checked that the illustration illustrated
something before offering it as one.

Two of tonight's habits meet here. The loop's rule that a count is evidence
and a picture is illustration is only worth anything if somebody looks at
the picture. And the best question asked all night came from outside the
loop: *what would a person who cannot hear this see?*

**PR:** [#590](https://github.com/unarbos/arbos/pull/590).

## Cycle 85 report (07:15 UTC, 09-18)

**Looked at:** the pause split, re-checked against the gateway's #562
(`1e0e9cde`, serving since 04:13).

**Half of M-146 is closed** (M-298). Six runs of `pause.wav` down the
capture path:

| | result |
| --- | --- |
| runs whose transcript split | **0 of 6** |
| runs answered more than once | **6 of 6** — 2, 3, 4, 3, 2, 3 replies with audio |
| frames captured vs sent | **900 / 900**, every run |

The breath no longer splits the transcript. Every run returns the whole
question as one final, which is what #562 was for.

**And the half that remains is a different fault now.** One transcript still
draws more than one spoken answer — from run 2, with only that single
transcript in the log:

```
reply: Right now, nothing's in progress; all recent workers have finished.
event response.done reason=completed playing=true
phase listening
reply: We just finished having two workers each run a timed sleep command…
```

No second transcript precedes the second reply. It is not segmentation any
more: the gateway hears one question and answers it twice, the second
arriving over the first. Re-filed with the numbers, and the phone is still
provably not in it.

**A rig fault worth more than the usual** (M-299). My first verdict counted
a run as bad if *either* the transcript split *or* more than one answer
played, and printed "still splitting, every run" — for runs whose
transcripts were whole.

Every other rig fault tonight made a working app look broken. This one made
**a gateway fix invisible**, and I would have reported no progress on a
change that had done exactly what it set out to do. A check inherits the
shape of the world when it is written, and the world moves; when a fix lands
upstream, the checks written before it are the first thing to re-read.

**PR:** [#592](https://github.com/unarbos/arbos/pull/592), harness only, and the re-check is
committed this time so the next ask is one command.

### Cycle 85, continued (07:20 UTC) — the breath turns out not to matter

**Staying on the re-check produced the useful part** (M-300). The same
counter, pointed at a clip with no pause in it:

| clip | what it asks | transcripts | answers with audio |
| --- | --- | --- | --- |
| `pause.wav` | 1 question, one breath | **1**, every run (8) | **2–4**, every run |
| `acceptance.wav` | 2 utterances, no pause | **2**, every run (3) | **3–5**, every run |

Transcription is right in both. Answers are over in both, by roughly one per
question, breath or no breath. So the pause is a red herring for the half
that remains: the gateway answers *any* question more than once, and the
replies are distinct answers rather than one reply in parts.

The item is filed under a pause because that is how it was met at cycle 43,
and the transcript half genuinely was a pause fault — #562 fixed it. The
inbox item is re-scoped rather than left under a name that would send the
next reader looking at segmentation.

**And the counter had "one question" baked into it** (M-301). Pointed at the
two-utterance clip it reported "the transcript split, 3 of 3" for runs whose
two transcripts were exactly right. `SAYS` now says how many things a clip
asks.

Worth noticing what that nearly cost. Without the fix the control run would
have been called broken, and the comparison that re-scoped M-146 could not
have been made — the tool would have agreed with the old theory by refusing
to measure the thing that disproved it.

## Cycle 86 report (07:35 UTC, 09-18)

**Looked at:** the four rows still dated 60 — cold start, long history,
background-and-back, attachments.

**All four hold** (M-302): cold start **3.2 s** to seven rows; a chat with
its composer in **1.0 s** on a transcript the kernel puts at **2601** lines,
paging back in **3.5 s**; **21** text rows before and after eight seconds
away; and the attachment chip with its `×` in the field. Two committed
scenarios, one run, three cycles of rows cleared.

**Then three faults in my own harness, each worse than the last.**

Cycle 80's fix went into the two files I had open, not into the fault
(M-303). `several-workers.sh` counted the sheet off a single screen — 12
rows against a pill of 22 — which is exactly what M-287 withdrew a finding
over. I fixed the two scenarios written that hour and never swept. The sweep
was one `grep` and it took six cycles.

"Paged to the end" was eight pages and a hope (M-304). With 25 agents the
fixed loop collected 17 rows and called that the end: a page count is an
assumption about list length dressed as a measurement. `collect_rows` now
pages until two pages add nothing and says whether it **converged** or hit
its ceiling.

And with paging provably converged the numbers *still* disagreed — pill 25,
labels 17 — and the instrument was still the reason (M-305). Rows are
collected by goal text and **three labels are shared on one screen**; this
project has run enough poems and sleeps to repeat itself. The comparison now
prints `cannot say`, calls its own number a floor, and names the condition
under which it would mean something.

**What I take from the three together.** Each one produced a number that
looked like the app contradicting itself, and in each the app was fine. The
progression is the useful part: the first was a gesture that did nothing,
the second a loop bound mistaken for a measurement, the third a counting
rule invalidated by the data it was counting. They get subtler as the
obvious faults are removed, which means the defence cannot be vigilance — it
has to be tools that report their own limits. Cycle 80's fix was a better
gesture; today's is a tool that refuses.

**PR:** [#593](https://github.com/unarbos/arbos/pull/593), harness only.
**Stills:** `media/mobile/cycle-86/`.

## Cycle 87 report (07:45 UTC, 09-18)

**Looked at:** the settings sheet, oldest row at 60.

**It holds** (M-306): five sections, three saved-token fields reading
`Token saved · paste to replace`, and at the foot `Arbos, 0.2.0 (1)` under
`The TestFlight build on this phone.` A token that cannot work drops the
list to **one** row — the pod, which needs no hub — and the screen says
**`Hub token refused.`**; restoring it brings **seven** back. M-206's
visible refusal and M-201's themed ground both still standing.

**Naming the buttons properly broke a script that depended on the bad name**
(M-307). #571 renamed the settings disc from SwiftUI's `Gear Shape` to
`Settings`. This scenario tapped `Gear Shape`, found nothing, printed
`no settings button` and exited — on a build that had been improved.

That is a cost of the accessibility work rather than a surprise, and worth
saying plainly: **accessibility labels are an interface**, and the harness
was coupled to their absence. Anything in a script reading a symbol-derived
name is reading a bug, and will break the moment the bug is fixed. Fixed
here with the name first and the old rendering as a fallback.

**And a fourth script reading one screen of a scrolling view** (M-308). The
build-line check read the first screen of the sheet and printed an empty
line, as though the app had lost it. It is four pages down, exactly where
M-220 recorded it.

The interesting part is why the three earlier fixes missed this one: they
were all about *counting rows*, and this is *reading text*. I had been
fixing a symptom shape rather than the rule, and the rule is wider —
anything read off a scrolling view must be read after moving to it. Five
scripts have now had some version of this fault.

**PR:** [#595](https://github.com/unarbos/arbos/pull/595), harness only.
**Stills:** `media/mobile/cycle-87/`.

## Cycle 88 report (08:00 UTC, 09-18)

**M-146 is closed** (M-309), after forty-five cycles. Six runs of
`pause.wav` between 07:50 and 07:54 gave **one transcript and one answer
every time**, 900/900 frames throughout. Eleven runs earlier the same
evening gave two to four answers each, so the gateway closed the remaining
half between 07:12 and 07:50.

Both halves went separately: #562 stopped the breath splitting the
transcript, and this second change stopped one question being answered
twice. The re-scoping looks like the part that mattered — while it was filed
as a pause fault it sat for weeks, and once cycle 85 showed it happening
*without* a pause it moved within the hour.

**The several-workers row is measurable at last** (M-310). Giving the four
workers a per-run tag at the front of each goal makes this run's findable
among every earlier run's: pill **25 → 29**, exactly four, and all four on
the sheet by name, each `Done`, with paging converged at 21 rows.

That is what M-305 asked for and it is not a cleverer count — it is goals
that do not collide. The sheet was always right; the question had been
unaskable.

**A quotation mark cost a whole run** (M-311). The tagged goals were first
written with quotes around them; iOS curls a typed `"`, the read-back never
matched, and after its twenty-second wait the scenario sent whatever was in
the box. Pill unmoved at 25, zero Done lines, nothing on the sheet, and a
confident verdict of `0 of this run's four reached the sheet` — an app
failure invented by a punctuation mark.

M-185 found smart punctuation and built `ui field plain` to undo it, and I
then wrote a new line with quotes in it anyway. The rule is not "handle
curly quotes"; it is **never put a character the keyboard transforms into a
line a script must match**.

**A note on branching.** Cycle 88 began stacked on cycle 86's branch because
`main` lacked the paging helper it needed, then moved to a fresh branch off
`main` once #593 and #595 landed. "Always branch off main" and "your
dependency is in an unmerged PR" genuinely conflict; stacking and then
re-basing when the parent lands is the honest way through, and the old
branch is deleted rather than left to rot.

**PR:** [#598](https://github.com/unarbos/arbos/pull/598), harness only.

## Cycle 89 report (08:10 UTC, 09-18)

**Looked at:** the call pulled down, oldest row at 61.

**All four claims measured** (M-312): the row reads `Add`, `Type to phone`,
`Mute`, `End call`; a line typed on the call reaches the kernel (seq 2745,
`heard`); tapping `Mute` turns the disc to **`Unmute`**; and closing returns
to the chat the call came from.

The mute half had never been exercised. The scenario was tapping the
*composer's* microphone — which is on a different screen — and reporting
"no mic button to mute" from the one screen that has a mute.

**The labels kept breaking the scripts, so this time I swept** (M-313). #571
broke the settings scenario last cycle and #590 broke this one. Instead of
patching one more file I compared every name the scripts tap against the
labels the app defines.

Four scripts still tapped `Close` for a button now called `End call` —
including **`mac-journey.sh`**, where the fallback is a coordinate tap that
lands on **Back**. The acceptance journey has been closing calls by hitting
the wrong control, and passing while doing it.

Four `Close` taps, ten `Up` taps and two new app labels moved in one commit.

**And the rule underneath, which I had been missing** (M-314). The harness
was tapping `Up`, `Microphone`, `Gear Shape`, `Close` — none of which the
app ever said. Those are SwiftUI's readings of unlabelled symbols. Every one
was a defect the scripts depended on, which is exactly why each
accessibility fix broke something:

> A name a script taps should be one the app states in
> `accessibilityLabel`. If it is not, the script is coupled to a *missing*
> label and will break when somebody supplies one.

That reframes the last three cycles. I had been treating the breakages as
collateral damage from good work; they were the good work finding places
where two things had grown into each other.

**PR:** [#599](https://github.com/unarbos/arbos/pull/599).
**Stills:** `media/mobile/cycle-89/`.

## Cycle 90 report (08:20 UTC, 09-18)

**Looked at:** the worker chat — open from the sheet, open from a worker's
line, and back. Oldest row at 61.

**The sheet path holds** (M-315): the pill opens the sheet, the first row
opens that worker's chat with its name in the header, the chat carries its
content and **no composer**, and `Back` returns to the project chat rather
than the sheet or the list.

**A claim that needed narrowing rather than a bug filed** (M-316). The row
has promised two ways in since cycle 32. Tapping
`w074832 mountains · Turn ended. Last words: …` does nothing, and I was one
line from reporting that. `workerLines` draws a `Button` **per running
worker and nothing for the rest**; the `Turn ended` line is kernel
transcript text, not a control, by design. The second way in exists only
while a worker runs, and the row had been implying every worker line is a
door.

**The running-worker half is still untested, and I would rather say so**
(M-317). With the target corrected the scenario starts a sleeper and waits
for its line; none appeared in 80 seconds. The likely reason is the
scenario's own — it types and sends without the read-back every other typed
line in this harness uses, so a composer that was not focused sends nothing
and the run cannot tell. Two of the row's three claims are measured; the
third is committed with the right target for the next cycle to finish.

**And a scroll in the wrong direction is indistinguishable from an absence**
(M-318). The first search used `page_up`, which walks towards the newest
line, for something older. It reported "no Done line in view" and would have
said that for ever.

That belongs beside M-287 as a pair: a gesture that does nothing, and a
gesture that goes the wrong way, both read as *the thing is not there*. The
tell in each case was a negative result arriving too cleanly.

**PR:** [#600](https://github.com/unarbos/arbos/pull/600).
**Stills:** `media/mobile/cycle-90/`.

## Cycle 91 report (08:30 UTC, 09-18)

**Looked at:** finishing the half I left open at 90 — a running worker's
line in the chat.

**It is still untested, and now the run says why it cannot say** (M-320,
M-321). Three things came out of chasing it, in order:

The read-back was missing from my own new file (M-319). Cycle 90 typed and
tapped Send without confirming the line landed, so when no worker started
the run could not tell a failed app from keystrokes that went nowhere. That
is the fault M-180 fixed everywhere else in this harness. I wrote a rule
about exactly this class at cycle 89 and broke it at cycle 90 — a rule in a
ledger is not a rule in a file.

With the read-back passing and the line provably sent, **still no worker**.
Watched for 64 seconds: the chat held nothing but the sent message.
`Through one worker: …` has the root run the sleep itself; the proven
wording is `Through one worker **you wait for**: …`, and even that did not
delegate in this project tonight. The kernel confirms no child for the
request. Whether a worker runs at all is the model's decision, not the
phone's.

And the check could not tell those apart (M-321). "No running-worker line
appeared" covered two different worlds — the app failing to draw a line, and
there being no line to draw — and only the first is the phone's to answer.
It asks the kernel now, and reports *no worker ran, so there was no line to
draw. Untested, and not the app's doing.*

**What this cycle actually produced** is not a verified row; it is a check
that can no longer blame the app for something the model decided. Given how
many of tonight's findings began as instruments misreading themselves, I
would rather have that than a green tick.

**PR:** [#602](https://github.com/unarbos/arbos/pull/602), harness only.

## Cycle 92 report (08:40 UTC, 09-18)

**Looked at:** the pill-versus-sheet thread from cycle 74 and #576, which
had been stuck behind an instrument that could not measure it.

**They agree, and the thread closes** (M-322). Run on `qa-cycle-11-demo`, a
project whose goals are distinct:

```
the pill says:  Agents 12
the sheet lists: 12 rows (paging converged)
labels sharing a name on one screen (each would hide a row): 0
VERDICT: the two agree on 12.
```

Two clean readings now agree — 19 = 19 at cycle 80 and 12 = 12 here — and
every apparent disagreement between them was the instrument: an off-screen
swipe, a fixed page count mistaken for the end of a list, and a count by
label on a list whose labels repeat. **The app was never wrong about its own
agents.**

That is worth stating plainly because cycle 74 filed the opposite, and the
correction took four cycles and three separate rig fixes to reach. The
original finding was wrong in a way that looked thoroughly evidenced.

**A refusal should name its way out** (M-323). The comparison rightly
declines where goals collide, then left the reader to go hunting for a
project where it would work. It names `qa-cycle-11-demo` now. A tool that
declines to answer is only useful if the next step is obvious; otherwise the
refusal is simply where the investigation stops.

**PR:** [#603](https://github.com/unarbos/arbos/pull/603).

## Cycle 93 report (08:50 UTC, 09-18)

**Looked at:** the loop's own machine — replaceability — oldest row at 63.

**Eleven scripts were still running tools from `$HOME`, and those copies had
drifted badly** (M-324). M-238 fixed the journey at cycle 63 and nothing
else; eleven more scripts went on calling `~/kernel.py` and
`~/find_row.py`. Measured against the checkout:

| tool | home copy | the repository's | |
| --- | --- | --- | --- |
| `ui.py` | 3480 b | 8310 b | less than half the current file |
| `kernel.py` | 7404 b | 8432 b | missing the agent filter from M-224 |
| `find_row.py` | same | same | retired, never changed |

The `ui.py` those scripts were using predates `field`, `focus`, `plain`,
`on_screen` and `menu` — everything cycles 54 to 56 built to make the
harness reliable. They were running the tooling of five weeks ago while the
fixes sat in the repository, committed and unused.

That is M-133's failure in its worst form. The first time, the tooling
existed only on one machine and losing the machine nearly lost it. This time
the repository had it all along and the scripts simply did not look there —
which is harder to notice, because nothing ever breaks loudly.

**The check M-238 asked for is a file now** (M-325).
`deploy/mobile/check-tools.sh` fails if any script reaches outside the
checkout for a tool, and prints the checksum of each tool the harness ships.
Both sides agree on all six.

M-238 recommended exactly this check and nobody wrote it, which is why the
same fault survived thirty cycles in eleven other files. That is the third
time tonight the difference between a rule and a file has cost something:
the ledger said the right thing each time, and only the file changed
behaviour.

**PR:** [#605](https://github.com/unarbos/arbos/pull/605), harness only.

## Cycle 95 report (09:00 UTC, 09-18)

**Looked at:** the call's orb — a row that had never had a check of its own.

**Measured directly for the first time** (M-327). The reason it never had
one is that the orb's state existed **only as a colour**; nothing in the
tree said listening, thinking or speaking. #590 gave the orb a value
carrying `Phase.label` for VoiceOver's sake, and the side effect is that the
sequence is machine-readable.

One question, one call:

```
  3.2  connecting
  5.8  listening
  8.4  thinking
 20.1  speaking
 26.6  listening
```

One pass, in order, no flapping — and one spoken answer for one question,
which is M-146 closed again from a different direction.

The provenance is worth keeping. This instrument exists because of an
accessibility fix made for a person who cannot see the colour. The loop
asked "what would someone who cannot hear this call see?", fixed the answer,
and got a new way to measure the app as change from it.

**And "one question" baked in for the third time** (M-328). Run against the
two-utterance clip, the flapping check called an ordinary two-turn call a
fault: each turn legitimately walks thinking → speaking → listening, so a
clip asking twice revisits all three.

M-301 and M-311 were the same assumption in different files. Three times is
a pattern rather than a slip, and the mechanism is clear — the assumption is
invisible while the default clip has one question in it, so every checker I
write passes its own first run and hides the flaw until somebody points it
somewhere else.

**PR:** [#607](https://github.com/unarbos/arbos/pull/607), harness only.

## Cycle 96 report (09:05 UTC, 09-18)

**Looked at:** the acceptance journey, run against the harness #599 swept.

**Run 34 holds** (M-329). `pod` on kernel `c3247332dc4e`, `main` at
f662b876: **14 pass, 2 eye, 4 unverified, no failures** — the same shape as
run 33.

Two of cycle 89's corrections are visible in it. P3's close now taps
`End call` rather than falling through to a coordinate that lands on **Back**
— the journey had been closing calls by hitting the wrong control and
passing while it did. And J5's line no longer tells QA the phone has no Stop
control; it reads *Stop not exercised by this step (the control exists:
composer stop square, M-130)*.

**Why this was worth a cycle.** #599 changed fourteen tap targets across the
harness, the journey's among them. A sweep that size deserves the spine run
against it rather than an assurance that it should be fine. It was, and now
that is a measurement rather than a hope.

connect 2532 ms, dictation 5 segments with `delta_shrinks=0`.
`media/mobile/journey/0918-085213/`, 22 files.

**No PR.** Nothing in the repository needed changing: the run exercised
code already on `main` and produced evidence, not a diff.

## Cycle 97 report (09:15 UTC, 09-18)

**Looked at:** the part of the attachments row cycle 72 deliberately did not
credit — the chip's × and the mic-to-send swap, both still reading from 60.

**All three claims measured** (M-330): an empty composer ends in
`Microphone`; a photo puts **one** chip in the bar and the end becomes
`Send`; tapping the chip's × leaves **zero** chips and `Microphone` returns.

**The × had no name, and answered to the wrong one** (M-331). Unlabelled it
read as `Close` — the SF Symbol's own name, and the same word the call's end
button answered to until #590. With several chips in the bar nothing
distinguished them, by ear or by script. It now reads
`Remove <file name>`.

That is the second time in three cycles a measurement could not be built
until a control had a name; the orb at M-327 was the other. Accessibility
labels keep turning out to be the *precondition* for testing rather than a
nicety beside it — which is a better argument for doing them than the one I
started with.

**And one rig fault, fixed the easy way** (M-332). The first attempt tapped
the photo picker's buttons by label, found nothing and reported `no chip
arrived`, blaming the attach. The picker is another process and the app's
tree holds nothing while it is up — exactly what M-194 said of the
notification banner.

`photo-reaches-the-model.sh` settled those coordinates at cycle 56, so this
run uses them rather than inventing a second answer. M-303's lesson applied
without pain for once: look for the existing fix before writing one.

**PR:** [#610](https://github.com/unarbos/arbos/pull/610).
**Stills:** `media/mobile/cycle-97/`.

## Cycle 98 report (09:25 UTC, 09-18)

**Looked at:** the running-worker line, untested since cycle 90 and reported
three times as something the app might not be drawing.

**The app draws it, and always did** (M-333). Dumping every Button while two
workers ran:

```
⠙, 2 Working p091543 one · Starting
⠋, Working p091543 two · Running sleep 80
⠇, Working 2                              (the pill)
```

Three faults in my own scenario hid it, and every one of them pointed at the
app:

* the line pattern wanted a digit straight after `Button`, where the label
  begins with the **animated braille spinner**;
* the check meant to ask whether a worker had run looked for the sleep
  duration in the child's *name* — children are named from the goal, so it
  always answered no and printed `the root answered it itself`, which was
  false. The kernel's record has `.arbos/agents/r091304-one/` and `-two/`
  running and finishing;
* the pill pattern had the same spinner blind spot and said `no pill in this
  chat` about a chat that had one.

**The rule** (M-334): never match on an animated glyph. The spinner changes
every frame, so a label read a second ago does not exist when the tap lands
— a race that always loses, and one that reads exactly like a control
refusing to respond. Match the part that holds still.

**What I am not claiming** (M-335). With the pattern fixed and the tap aimed
at the worker's name, the screen stays on the project chat. I am not filing
that. Three rig faults surfaced in this one file today and all three pointed
at the app; the worker chat may be presented as a sheet, in which case my
screen-classifier would see the chat *underneath* and report `project-chat`
whether or not it opened. That is the next check, and it is about the rig.

**The honest shape of this cycle**: I spent it discovering that three
previous cycles' careful reports were wrong in the same direction. The
ledger now says so plainly, which is worth more than the row.

**PR:** [#612](https://github.com/unarbos/arbos/pull/612).

## Cycle 99 report (09:30 UTC, 09-18)

**Looked at:** the one claim I left open at 98 — does tapping a running
worker's line open its chat?

**It does** (M-336). Tapped **by frame**, it lands in the worker's chat
first time: `Back`, the spinner, and `q092440 one` in the header. The
presentation is a `navigationDestination`, not a sheet, so my "the
classifier is being fooled" theory was also wrong — the screen genuinely
changes.

Tapping by *label* cannot work here, for two reasons I had not put together:
the spinner at the front is animated, so an exact label is stale the moment
it is read; and the worker's name appears on more than one element, so a
name match can land somewhere that is not the button.

**The tally for this one row** (M-337). It took cycles 90, 91, 98 and 99:

1. a send with no read-back;
2. a scroll in the wrong direction;
3. a pattern blind to the animated spinner — plus a sibling of it in the
   pill, plus a child-check hunting for the sleep duration in a goal-derived
   name;
4. a tap by a label that cannot be matched.

**Not one was the app.** The line, the pill, the naming and the navigation
were right the whole time.

That is the finding worth more than the row. Four independent mistakes in
one small area all produced the same shape of output — *the app did not do
the thing* — because that is simply what a rig failure looks like from
outside. A negative result about the app deserves more scepticism than a
positive one, and after tonight this loop has the tally to say so rather
than just the instinct.

**PR:** [#612](https://github.com/unarbos/arbos/pull/612), updated.

## Cycle 100 report (09:45 UTC, 09-18)

**Implemented the returning-user behaviour** (M-338). Cycle 75 measured that
after iOS reclaims the app the phone came back to the **list**, not the chat
the person left, and filed the question rather than choosing. The features
agent answered it from the desktop's own code: `state.toml` keeps the open
projects *and* which was `active`, and `Workspace::new` makes that one front
again.

The phone matches now, and the scenario that found the problem confirms it:

```
left it at:        chat:phone
reclaimed, relaunched:
came back to:      chat:phone
VERDICT: put back in the chat he left, even though the app had been killed
```

`kernelTarget` could not answer "which was in front" on its own — it names
the last project opened and stays set while the list shows, because the
list's composer talks to it. `frontProject` is set while a chat is on the
stack and cleared when it is not: the same distinction the desktop draws
between its open tabs and its active one. Restored once at launch, so
leaving the chat does not put you straight back in.

**Two checks had to follow the feature** (M-339). The scenario read the
restored chat's title at 9 s, before the identity arrived, and reported a
*different project*; then on the next run it could not find the `phone` row
at all, because the app now opens into a chat rather than the list.

That is the ordinary cost of shipping behaviour, and worth naming: a change
to where the app lands invalidates every scenario that assumed the old
landing. The first symptom was a confident false verdict — `somewhere else
entirely` — from a check that was correct yesterday.

**One hundred cycles.** The thing I would tell the next worker is the tally
from M-337: four rig faults on one row, none of them the app. Tonight's
findings were far more often about the instrument than the phone, and the
instrument always failed in the same direction.

**PR:** [#615](https://github.com/unarbos/arbos/pull/615).
**Stills:** `media/mobile/cycle-100/`.

## Cycle 101 report (09:50 UTC, 09-18)

**Looked at:** the list's search, filter and refresh, oldest row at 71.

**It holds** (M-340), with the same numbers as thirty cycles ago: five rows
at rest, `beta` → 2, `zzzz` → 0 with the no-match line, Live only → 3, and
the pull taking `/list` from two calls to three with `arrived-late` arriving
on screen.

**A trigger file outlived the run that made it** (M-341). The fixture serves
its "new" project when `/tmp/fixture-add-late` exists, and cycle 71 left one
behind. It served that project from the **first** call, so `arrived-late`
was on screen before the pull and the verdict congratulated itself on a row
that had always been there. The refetch count was real; the "new project
appeared" half was not.

Same family as M-274, where a worker left running by the previous run made
its evidence look like this one's. State on the machine outliving the run is
its own class of fault — and worth separating from the rest of tonight's,
because **both of these produced a pass**. Every other rig fault today made
the app look broken; these two made it look fine, which nobody goes looking
for.

**On the kernel CI red** on cycle 98's branch: shell-only diff, the kernel
compile job passed, and the failed log was gone again — the job was already
`in_progress` on a re-run before I could read it. That is five attempts. The
automatic re-run means the log cannot be captured this way at all; whoever
owns the flake needs the Actions UI, or the retry turned off.

**PR:** [#617](https://github.com/unarbos/arbos/pull/617).
**Stills:** `media/mobile/cycle-101/`.

## Cycle 102 report (10:00 UTC, 09-18)

**Looked at:** the call's words read back in the chat, oldest row at 76 —
the feature Jacob specified himself.

**The rule holds** (M-342), but the check that tests it had rotted. The run
first reported `both wordings are showing`. It had not: the chat showed the
answer **once**.

The check asked whether this turn's reply *differed from* the kernel's
wording. That works only while the two differ — and since today's gateway
change Live often says **exactly** what the kernel said, so the single
spoken row matched the phrase and was counted as a second copy.

**A check built on a coincidence, and the coincidence ended** (M-343). M-279
made the rule testable by noticing that the two answers differ. That was
true when it was written, and it is the whole reason the check worked. It
stopped being true today, and the check had no way to know — it reported a
violation of a rule that was being kept.

Replaced by counting the rows this turn gives its answer, which needs no
assumption about the words. Then counting the *whole screen* over-counted,
because older turns replay as the kernel's text once their spoken rows are
gone — so it is scoped to the rows after the last `Spoken` marker.

Two corrections in one cycle, both from the same root: **the world moved
under a check that had encoded a fact about it.** That is a different
failure from the rig faults earlier today, and probably the more dangerous
one — the check was correct when written, reviewed, and used, and became
wrong without anybody touching it.

**PR:** [#618](https://github.com/unarbos/arbos/pull/618).

## Cycle 103 report (10:05 UTC, 09-18)

**Built a sweep, and it found a rotted check on its first run.**

The reason for building it: today the loop found six rotted checks, one
cycle at a time, and **every one was found by accident while looking at
something else**. A scenario that no longer measures what it claims goes on
passing quietly. So run them together and read the verdicts side by side.

Ten scenarios, ten verdicts. The one that matters (M-344):

```
pill-count-vs-sheet   they disagree — pill 2, sheet 3, and no shared labels to explain it.
```

It is not a disagreement. The pill shows `Agents N` — every agent — when
idle, and **`Working N` — only those running** — when busy; the sheet always
lists them all. Comparing against the Working form compares a subset with a
whole.

That is the **third** assumption encoded in that one tool, after the fixed
page count and the count-by-label, and the **fourth** time it has accused an
app that was behaving exactly as designed. It declines now unless the pill
is in its `Agents` form.

**Two more things the sweep surfaced.** The worker-chat row passes
unattended now (M-345) — cycle 99 proved it by hand, this is the committed
scenario reaching the same conclusion on its own, which is the difference
between a finding and a check. And four scenarios reach **no verdict at
all** (M-346): they print measurements and stop, so the sweep fell back to
their last line of output. Not broken, but half a check, and invisible until
something tries to read them in bulk.

**What I would keep from today.** The loop's instruments fail about as often
as the app does, and far more often than the app in the direction of
accusing it. Reading them together is the cheapest way to catch that; one
run found in ten minutes what six cycles found by stumbling.

**PR:** [#619](https://github.com/unarbos/arbos/pull/619).

## Cycle 104 report (10:15 UTC, 09-18)

**Looked at:** the gap the sweep named at 103 — four scenarios that measure
and never conclude.

**Two of the four now conclude** (M-347):

```
VERDICT: a token that cannot work empties the list to 1, says why, and 7 come back
VERDICT: pulled down it types, mutes, and closes back to the chat it came from
```

Before this the sweep could only quote their last line of output. A scenario
that measures without concluding is half a check — fine while a person reads
it holding the row's claims in mind, useless the moment anything reads ten
at once.

**And the first verdict misread its own scenario** (M-348). It matched
`where()`'s output against the word "chat" and announced `closed to 'Button
Back', not the chat the call was entered from`. `where()` returns a raw tree
line; a `Back` button *is* the pushed chat, and the close had gone exactly
where M-217 says it should.

I wrote the reader and the thing being read within minutes of each other and
still encoded a wrong assumption about the format. That is worth recording
because it corrects something I had half-believed all day: these faults are
not caused by *distance in time* from the thing being measured. They are
caused by not looking at the actual output. The fix is the same whether the
code is five weeks or five minutes old.

**The other two** — `cold-start-and-history` and `the-core-chat-path` —
carry several claims each and need more than a line; left for the next
cycle rather than given a verdict that flattens them.

**PR:** [#620](https://github.com/unarbos/arbos/pull/620), harness only.

## Cycle 105 report (10:20 UTC, 09-18)

**Apple refused the #615 upload** — `Upload limit reached. Please wait 1
day` (M-350). Every commit under `ios/` on `main` spends an upload, so a
merge per cycle burns the day's allowance on work nobody is testing yet. For
the rest of today `ios/` changes batch into one PR; harness work under
`deploy/mobile/` touches no archive and ships as before, which the
`ios changed?` job already enforces.

Recorded in `internal/mobile-mac-host-and-testflight.md` rather than left in
a conversation: it is a rule about the calendar, and the next worker will
not have been here today.

**The last two scenarios now conclude** (M-349), which completes what the
sweep named at 103:

```
VERDICT cold start:   3.1s to a list of 7 rows
VERDICT long history: chat in 1.1s, pager Show 200 earlier lines
VERDICT away and back: the chat is as it was — 29 text rows both sides
VERDICT: send → card 1.0s → reply 20.3s → Worked 21.3s, and the composer cleared
```

`cold-start-and-history` carries four coverage rows, so it gets four lines
rather than one that flattens them. The measurements were always there; the
missing piece was the sentence that makes them legible to anything but a
reader who already knows the row's claims. They also re-confirm four rows in
passing — cold start 3.1 s against 3.2 s at cycle 86.

**Where this leaves the suite.** Ten scenarios, ten verdicts, one command.
Three days ago most of these rows were checked by looking at screenshots;
the value of the last few cycles is that a person can now read what the loop
concluded without re-deriving it, and so can the sweep that catches the
checks going stale.

**PR:** [#622](https://github.com/unarbos/arbos/pull/622), harness only.

## Cycle 106 report (10:25 UTC, 09-18)

**Looked at:** notifications, oldest row at 82. Harness-only, to respect the
upload cap.

**The row holds and now says so** (M-351):
`banner ~2s after going away (unseen=1), and the tap landed in the project`.
It had been measuring the ask, the banner, the badge count and the landing,
then printing a raw tree dump and stopping. It is in the sweep now — eleven
scenarios, eleven verdicts.

**A second thing found, and deliberately not fixed** (M-352). The
transcript's worker-line glyph reads `Arrow Turning Down Then Right` —
SwiftUI rendering `arrow.turn.down.right`, the same family as `Gear Shape`
and `Move`. It is decoration beside text that already says what the row is,
so the fix is one line of `.accessibilityHidden(true)`.

It is not going in today. Apple refused the #615 archive with `Upload limit
reached`, so `ios/` changes batch into one PR — and this is the first entry
in that batch. Worth having; not worth an upload of its own.

That is a small discipline but a real one: the cap makes the cost of a
change visible, and a one-line accessibility tidy is exactly the kind of
thing that would otherwise spend a day's allowance on its own.

**PR:** [#624](https://github.com/unarbos/arbos/pull/624), harness only.

## Cycle 107 report (10:30 UTC, 09-18)

**Looked at:** the style pair, oldest row at 83 — this time the chat rather
than the list. Harness-only, the cap being on.

**The tool does not transfer, and now says so** (M-353). Pointed at the chat
against Cursor's chat reference it reported **5.3% against 24.3%** row
pitch. That looks alarming and means nothing: the tool was built for lists,
where rows repeat at one height, and a chat has no such unit — on one side
those "rules" are paragraph edges, on the other message bubbles.

Guarded rather than deleted. It compares the rule counts first and refuses
when they are nothing alike (2 against 13 here), while the genuine list pair
(9 against 8) still passes silently. The chat pairing goes back to being
**unmeasured**, which is the honest state.

**The day's pattern, stated plainly** (M-354). Every rig fault today was an
instrument used where its assumptions did not hold: a gesture off the
screen, a page count taken for a list's end, a count by label on repeating
labels, a pattern blind to an animated glyph, a name the app no longer used,
a premise about the gateway that changed, a pill counting two different
things, and a pitch that only exists on lists.

Each was correct on the case it was written for and silently wrong on the
next case it met, and **none announced its boundary**. The cure that has
actually worked — seven times now — is making the tool state the condition
under which its answer means something: `cannot say`, `paging converged`,
`CAUTION`. Remembering the condition myself has failed every single time.

**PR:** [#625](https://github.com/unarbos/arbos/pull/625), harness only. The worker-line glyph
stays in the batched `ios/` PR.

## Cycle 108 report (10:40 UTC, 09-18)

**Looked at:** the chrome row from 84 — and rather than eyeball it again,
gave it a committed check. Harness-only, the cap being on.

**`check-names.sh`** walks the list and a chat and flags anything whose name
reads like a rendered SF Symbol (M-355). On current `main` the list is
clean, and the chat carries two `Arrow Turning Down Then Right` — the
worker-line glyph already in the batched `ios/` PR.

**The check caught itself on its first run** (M-356). I gave it a self-test:
three names the app really shipped plus one good one, and a refusal to run
unless it objects to exactly the three. First run: **0 of 3**.

The detector was a shell function running `python3 - <<'PY'`, which takes
its *program* from stdin — so the piped dump went nowhere and it passed
everything. **Both screens had already been reported clean by a check
looking at nothing.**

My own manual attempt to verify that detector was malformed the same way — a
heredoc swallowing the input redirect — and also reported nothing caught.
Two independent mistakes of one shape within five minutes, and only the
version living inside the script caught it.

That is the strongest form of today's lesson I have found. Eight times I
have written "make the tool state its own limits"; this is the first tool
that *demonstrates* it can still fail before it claims anything passes, and
it earned that on its first run.

**Then a false positive** (M-357): `Agents 36` was flagged because the
pattern let a digit begin a word. Fixed — every word needs a capital letter.
Both faults were found by reading the output against screens whose true
state I already knew, which is the cheapest verification available and the
one I keep having to relearn.

**PR:** [#626](https://github.com/unarbos/arbos/pull/626), harness only.

## Cycle 109 — every screen, and what the pill was hiding

Cycle 108's check only ever walked two screens. Pointed at the other three
it found the labelling work holds: **settings and the call are clean**, and
the workers sheet's only flag was `Sheet Grabber`, UIKit's own drag handle
(M-358).

**The detector had a hole a row away from a name it caught** (M-359). It
wanted two capitalised words, so `Image Circle` — the away card's bullet —
walked past it, in output I had already read and called clean.

**Then the thing worth the cycle** (M-360). Reading a still for the bullet,
the away card's last line was sitting under the workers pill. Scrolled to
its end, the transcript stops **59 points short**:

| | last line, as opened | true end of content |
|---|---|---|
| chat with workers (pill) | y 731 — behind the pill | y 672 |
| chat with none (no pill) | y 721 | y 721 — identical |

The right-hand column is the whole diagnosis. With no pill, `scrollTo` lands
exactly at the end; with one, it stops a pill's height short, and whatever is
last — a reply, or the card that exists to be read — sits behind it.

**I got it wrong first.** I blamed the scroll arriving before the layout,
added a settle-then-scroll, and it changed nothing. A log line proved the
scroll ran with its guard passing. A second scroll 400 ms later landed in
the same place. Only the no-pill comparison settled it, and it took a minute.

The fix reserves the height the pill actually measures — not a constant, and
zero when there are no workers. After it, the card opens fully in view:
`OK` at y 632 against the pill at 731.

**Two PRs, because the cap splits them:** the harness work ships now; the
three `ios/` fixes join the batch (M-350).

**A user had already reported this, and I had already closed it.** The
feedback poll this cycle re-listed F6 — *"Can't scroll any further to see
bottom chat"* — closed at build 994 against the keyboard hiding the tail.
That cause was real and is fixed. The pill hiding the tail is a **second**
cause of the same sentence, still live four weeks later (M-363). The lesson
is narrow and useful: a complaint closed against one cause is not evidence
that the words were only ever about that cause.

The poll also marked all 19 items `NEW`, because its state lives in
`~/mobile-feedback` and a fresh worker creates it empty (M-364). No harm —
`internal/mobile-feedback-log.md` is the durable record — but the `NEW`
marker means nothing on a machine's first run.

**PRs, both green:**
[#632](https://github.com/unarbos/arbos/pull/632) harness,
[#633](https://github.com/unarbos/arbos/pull/633) the `ios/` batch — iPhone
build passes; TestFlight stays **1936** until the cap resets.

## Cycle 110 — the oldest row, and a fix that was not as whole as I said

**First, the rig lied.** `mac-cycle.sh` chains its checkout with `&&` under
`set -uo pipefail`. Scratch edits I had left on the Mac made `git checkout`
refuse, the chain stopped, and the script carried on to build, launch and
report — naming `main` while sitting on the previous tree. It said **BUILD
SUCCEEDED** twice (M-365). Fixed: it clears the tree, and stops if it is not
where it claims to be.

**The oldest row holds.** `project chat — send, prompt card, streaming,
Worked line`, last measured at cycle 62, re-measured on `main` `d2a807e4`:
card **1.1 s**, reply **2.0 s**, `Worked 7s` at **8.9 s**, composer cleared,
and the kernel's record shows exactly one turn (M-367).

**And the thing I have to correct.** Yesterday's #633 says the transcript
now ends above the pill. It does for the away card — 731 to 632, measured
twice, and that still holds. It does **not** in general:

| | as opened / after | true end | short by |
|---|---|---|---|
| open `phone` (has workers) | y 673 | y 580 | **93 pt** |
| send in `phone` | y 561 | y 560 | 1 pt — flush |
| open `const` (no workers) | y 721 | y 721 | flush |

So the fault is the **way in** to a chat that has workers, and a long
trailing paragraph is still clipped by the pill. My published mechanism —
"the pill row's height is not counted" — is not the whole story.

I tried two fixes from the next theory (the tail is under the inset from the
first layout, so it never appears and `atTail` stays false):
`contentMargins(.bottom:for: .scrollContent)` changed nothing, and scrolling
on the way in regardless of `atTail` landed *further* from the end. **Both
reverted.** The measurements are written down so the next cycle starts from
evidence rather than from my third theory in a day (M-366).

## Cycle 111 — 501 points, and three theories I did not need

Last cycle I left M-366 open: opening a chat that has workers stops short.
I had three theories and no numbers, because everything I could see came
from element centres in the accessibility tree — where things *are*, never
why.

One build with `onScrollGeometryChange` ended it:

```
ARBOSGEO off 5467.3  end 5968.7  short 501.3  insetB 105.3   ← as opened
ARBOSGEO off 5863.0  end 6002.7  short 139.7  insetB 139.3   ← one more pass
```

**501 points short**, about five lines of the newest reply, under the
composer and the pill. Not the pill's height, not `atTail`, not
`contentMargins`. The rows between are not realised when the first scroll
runs, so the proxy scrolls to an estimate. A second scroll, once they exist,
arrives.

**And #633 comes back out.** Its reservation cured the away card by luck and
left every chat opening with a pill-high band of nothing under its last
line. With the second scroll and no reservation, `phone` (has a pill) and
`const` (has none) both open **exactly** at their true end — three swipes
move the last line by one point.

The core chat path still holds on the fixed build: card 1.0 s, reply 2.0 s,
`Worked` at 11.1 s, composer cleared (M-367 re-run).

**PR:** [#638](https://github.com/unarbos/arbos/pull/638), the `ios/` batch.

## Cycle 112 — what a recording sees that a still cannot

**The oldest row is a check now.** `projects list — faces, rows, sections`
had been carried as prose in the coverage ledger since cycle 69. It is
`list-rows.sh` now: 12 rows, all naming a state, 8 carrying an age, none
claiming "now" (M-371). Writing it immediately found one fault — the `phone`
row read `phone, Idle,  · , home, 17m`, because the " · " drawn between the
state and the machine is a `Text` of its own and SwiftUI hands it to
VoiceOver as an element (M-370).

**Then the recording earned its place.** A recording was overdue, so I made
one of the list and a chat opening, and asked for a review of what was
actually on screen. It confirmed the opening position #638 fixed — and found
what four stills across three cycles had not:

> scrolling back down rests with the last message **partially hidden** behind
> the pill, and the user has to perform an additional upward swipe to pull
> the rest of the message into view.

Opening a chat was right. Scrolling to the bottom **by hand** was not. A
still shows where a scroll *landed*; it cannot show where a scroll *rests*,
and I had been proving this surface with stills for three cycles (M-372).

Fixed with a bottom content margin the height of the pill. The opening
position is untouched: last line at y **581** both as opened and at the true
end.

The review also suggested the list's composer overlaps the last row. It does
not — scrolled to its end the list clears it, and what the video caught was
an ordinary mid-list position (M-373). Worth a line, because a plausible
fault that is not one costs the next reader the minute it cost me.

**Recording:** `media/mobile/cycle-112/recording_demo.mp4`.

## Cycle 113 — one breath, one transcript, one *kernel* answer

The gateway now serves B, so this is the re-check. Five runs across two
passes, every one clean: one transcript carrying the whole question across
the breath, one spoken answer, `900 of 900` mic frames. **M-146 does not
reproduce**, 70 cycles after it opened (M-374).

**But the scenario was only proving half of what the sentence claims.** It
counted the gateway's `response.done` frames. That says how many answers
were *spoken*; it says nothing about who spoke them. B is "hold, then one
**kernel** answer", and a gateway answering from its own model would pass
that count exactly — which is not a hypothetical, it is F18.

So it reads the kernel's own record now, per run:

```
 2957 user          Hello Arbus. What are we working on right now? Give me one sentence.
 2958 assistant     We just replied with three short sentences about the sea for a quick test prompt (core 115306).
 2959 turn_complete
```

One question, one answer — and the answer cites `core 115306`, a turn only
the kernel knows about. That is the proof the frame count could never give
(M-375).

**PR:** [#643](https://github.com/unarbos/arbos/pull/643), harness only.

## Cycle 114 — a check that printed its evidence and never read it

The oldest row left was `voice notes in the composer`, last measured at
cycle 69. It holds: `2975 → 2975 unsent → 2977 on the send`, twice over.
Dictation fills the field, the button reads `Send`, and the kernel's
transcript does not move until he taps it (M-376).

**What the run was not checking.** The scenario's own first line says
"dictation puts words in the composer and sends nothing until he does". It
proved the second half properly, off the kernel's counts — and printed the
first half as a line of text that nothing ever read. An empty field would
have passed. A sentence from a different clip would have passed.

It compares now, and the comparison is soft on purpose:

```
  the field holds:               Please summaries what the workers did today in two sentences.
  of the clip's words:           9/10 kept (90%)
    not heard:                   summarise
    heard instead:               summaries
```

Recognition is never exact — a synthesised voice saying "summarise" comes
back "summaries" — so demanding every word would make the check flap, and
demanding nothing made it blind. It asks how much survived and names the
difference (M-377).

**And the sentence now lives in one place.** `mac-attach.sh` speaks it to
make the clip; the check needs the same words to compare against. Written
twice, a check can verify dictation against a sentence the clip no longer
says. `NOTE_SAYS` in `sim-lib.sh` (M-378).

This is the third cycle running where the fault was the same shape: the
instrument gathered the evidence and then judged something narrower than
its own sentence. Cycle 112 counted rows without hearing them, 113 counted
answers without asking who gave them, and 114 printed words without reading
them.

## Cycle 115 — a check that had measured nothing for fifteen cycles

`refusal-and-transport.sh` ran, printed four sections, and ended with its
paragraph about how a refusal and a transport failure should differ. Every
section said `no <row> row`. It had opened no case at all.

The cause is mine. At cycle 100 I made a cold start come back to the chat
that was in front (M-338). That was the right change. It also means the app
no longer lands on the projects list — and this scenario taps four fixture
rows by name on the list. It has measured nothing since, and said nothing
about it, because its closing text is printed unconditionally (M-379).

Fixed three ways: it reaches the list, it refuses to run when the fixture's
four rows are not on screen, and a case that never opens now produces
`VERDICT: none`.

**The behaviour itself is fine.** Once the scenario could see the fixture:

| case | attaches in 25 s | |
|---|---|---|
| `no machine named` | **1** | stops — nothing will change |
| `is offline` | 3 | retries — comes back when a kernel starts |
| `has no kernel serving` | 3 | retries — comes back when someone starts it |
| 502 at the tunnel | 3 | retries — nobody answered |

Identical to M-257 at cycle 70 (M-380). Only the instrument had rotted.

**And the paragraph was wrong anyway.** It said "a refusal should be counted
once and left alone", so by its own words two of the three refusals were
faults — contradicting the very finding the file was written to record. Two
refusals *should* retry: they name a thing that comes back. Each case now
carries its expectation and the run reaches a verdict (M-381).

**PR:** [#648](https://github.com/unarbos/arbos/pull/648), harness only.

## Cycle 116 — who else assumed the app opens on the list

Cycle 115's cause generalises, so this cycle looked for the rest rather than
finding one a week. Seven more scenarios launch the app and tap a project
row: `chip-and-send-arrow`, `photo-reaches-the-model`, `pill-count-vs-sheet`,
`several-workers`, `the-core-chat-path`, `voice-notes-wait`,
`worker-while-it-works`.

**They were not all broken, and that is the worrying part.** A fresh install
has no front project and lands on the list. So the same scenario passes when
it runs first and taps at a chat when it runs after another — the answer
depends on what ran before it, and nothing says so. `cold-start-and-history`
still reports `3.0s to a list of 7 rows` for exactly this reason: it runs
straight after the install (M-382).

One `reach_the_list` in `sim-lib.sh`, not seven copies, because eight copies
of an idiom is how the next one gets missed. Proven by running
`the-core-chat-path` twice back to back: both reach the same verdict now.

**And it passed while blind on its first test.** Run from a shell where
`BASH_SOURCE` is unset, it resolved its own directory to `.`, could not find
`ui.py`, got an empty dump, read "no Back button" from that and returned
success — with the app in a chat the whole time, as the next dump showed.
That is the same fault as the one it was written to fix, committed while
fixing it. An empty tree is "I cannot see", not "nothing is there" (M-383):

```
--- woke up in:   Back buttons: 1
reach_the_list: ok
--- after:        Back buttons: 0, project rows: 12
--- blind:        guarded: refused to answer blind
```

**PR:** [#651](https://github.com/unarbos/arbos/pull/651), harness only.

## Cycle 117 — the returning user was already done; three instruments were not

**The returning-user request needs no code.** It shipped at #615 and the
fault in it was fixed by #635, both on `main`. Driven on the device in the
two-project form the note itself specifies: suspended two minutes → back in
the chat he left; killed and reclaimed → back in that chat; and left `phone`
for the list, opened `demo`, reclaimed → **`demo`**, `the second project,
the one he was in` (M-384). The phone does not land on the list.

So the cycle went to the first full sweep since `reach_the_list`, and it
paid three times.

**#633's hidden glyph never took, and my own check passed it.**
`Arrow Turning Down Then Right` is back beside every ended worker line, with
`.accessibilityHidden(true)` sitting in the source. The cycle-109
verification was a **false pass**: worker lines are drawn from live state,
not the transcript, so they vanish on relaunch — the chat that check opened
had none on screen. I cleared a screen that did not contain the thing I had
fixed (M-385). The row is one element now, labelled with its words and
marked as text, verified against a worker spawned for the purpose.

**`check-names.sh` cleared three screens it never opened.** Pointed at a
row that does not exist it printed the list, settings and the call, skipped
the chat and the sheet in silence, and ended `VERDICT: no control reads as a
symbol name`. It counts now, and refuses: `screens looked at: 3 of 5 /
VERDICT: incomplete — never reached: chat sheet` (M-386).

**The sweep's summary contradicted its own table**: `0 of 11 reached no
conclusion at all`, printed directly beneath three rows reading `(no
verdict) no phone row`. It was counting only runs that printed nothing at
all (M-387).

Those three "no phone row" runs are `reach_the_list` not being on `main`
yet — [#651](https://github.com/unarbos/arbos/pull/651) is green and waiting.

**PRs:** [#658](https://github.com/unarbos/arbos/pull/658) harness,
[#659](https://github.com/unarbos/arbos/pull/659) the `ios/` batch.

## Cycle 118 — a file mentioning a thing is not a file doing it

Cycle 116 swept for scenarios that assume the app opens on the projects
list, and marked four of them clean. It did that by searching each file for
the string `Button +Back`. `call-pulled-down` matched — **inside an
unrelated helper**, a `where()` that builds a grep from that text — and it
has been printing `no phone row` in every sweep since (M-389).

I checked for a mention and called it behaviour. So the check is now in
`check-tools.sh`, where the other harness-hygiene checks live, and it asks
the real question: does the step happen *before* the tap, in line order?

**It found one more the moment it ran.** `cold-start-and-history` taps its
row with no step to the list — and worse, its first measurement was of a
behaviour the app no longer has. "Launch to a list with rows" only worked
because the sweep runs it first, on a fresh install where there is no chat
to come back to. It now waits for either landing and names it (M-390):

```
VERDICT cold start:   3.3s to the chat it was left in (pod)
                      (cycle 37 measured 3.1s to a list)
```

The number stays comparable; the sentence stops being false.

**Eleven scenarios, eleven conclusions.** With #651 on `main` and these
fixed, the sweep is whole again: `call-pulled-down` concludes,
`voice-notes-wait` and `pill-count-vs-sheet` came back on their own (M-391).

One flap to watch: `list-search-filter-refresh` said the refresh worked in
one run and `the list is not redrawing the answer` in the next. Its own
header warns its fixture's trigger is timing-bound, so that is where to look
first.

**PR:** [#662](https://github.com/unarbos/arbos/pull/662), harness only.

## Cycle 119 — a count cannot tell a race from a fault

Last cycle's loose end: `list-search-filter-refresh` said pull-to-refresh
worked in one run and `the list is not redrawing the answer` in the next.

Reading it, the check could not have known. It counted `/list` calls before
and after the pull, and a rise is consistent with two quite different
stories:

- the app asked again **before** the fixture began serving the new project —
  the request was already in flight, and the run tests nothing;
- it was served the new project and did not draw it — a real fault.

The scenario blamed the app either way (M-392). The fixture now logs what it
**answered**, not only that it was asked:

```
/list calls:          2 before the pull, 3 after
answers carrying it:  1 of 1 calls since the pull
  VERDICT: the pull refetched and the new project reached the screen
```

and a run where no answer carried it says `cannot say — the request was
already in flight; this run tests nothing`.

**The flap did not reproduce.** Four runs — three isolated, one after
`list-composer` as the sweep orders them — all pass. I stopped there rather
than hunt a fifth green (M-393). The next occurrence will name which story
it was, which is worth more than another pass today.

**PR:** [#665](https://github.com/unarbos/arbos/pull/665), harness only.

## Cycle 120 — "inconclusive" is a word an instrument hides behind

`list-composer` has ended every sweep this week with `could not read both
numbers — inconclusive`. Run on its own, it passes all four of its steps.

Run second in the sweep, it reads a **chat**: no rows, no composer, no
keyboard. It needs the projects list and never asks for it — and last
cycle's check did not look at it, because it never *taps* a row. It counts
them and reads the composer's placeholder (M-394).

What kept this alive for a week is the word. "Inconclusive" sounds like the
world being unclear. Here it meant the instrument was pointed at the wrong
screen, which is not the same thing at all, and nothing in the sweep's table
distinguished them.

**The rule has now been wrong in both directions.** Cycle 116 matched a
string and missed four scenarios. Cycle 118 matched a tap and missed this
one. Widening it to "reads the list" then flagged four files that were
already correct, by counting helper *definitions* as uses (M-395). It reads
the main flow only now, and a scenario whose subject is the landing —
`cold-start-and-history` — says so in a line of its own rather than being
bent to a rule it cannot satisfy.

Two more were quietly missing the step: `refusal-and-transport` kept its own
inline copy of the idiom, and `settings-and-a-bad-token` had none.

**The sweep, after:**

```
list-composer      the composer sits 88 pt above the keyboard
...
0 of 11 printed nothing at all.
0 of 11 printed output but reached no verdict — read those first.
```

Eleven scenarios, eleven conclusions (M-396). The one decline left is
`pill-count-vs-sheet`'s `cannot say`, which is a scenario refusing a
comparison it cannot make — that one is by design.

**PR:** [#668](https://github.com/unarbos/arbos/pull/668), harness only.

## Cycle 121 — a decline that could never stop declining

`pill-count-vs-sheet` has ended every sweep with `cannot say`. The reason it
gives is correct: while anything runs the pill reads `Working N`, counting
only what runs, and the sheet lists every agent — a subset against a whole.

But it runs in the sweep after the scenarios that start workers, so
something is always working. The decline was permanent, and a row that
declines every time is uncovered while looking careful (M-397). It waits for
the project to settle now.

**Then it accused the app the moment it could speak**: `they disagree — pill
38, sheet 30, and no shared labels to explain it`.

The app cannot disagree with itself here. The pill is `chat.workers.count`;
the sheet is a `ForEach` over that same array; `workers` is a dictionary
keyed by the agent's id, so no row is lost to a collision. Reading the
source settled in a minute what no amount of re-running would have (M-398).

**Which rig fault, then?** `collect_rows` now reports `converged after 5
page(s)`, and that rules out the swipe — the sheet scrolled and still
stopped eight short. Rows are counted by their **label**, and this project
has eight more agents than distinct goals: twins on different pages collapse
under `sort -u`, and the duplicate check only ever looked within one screen.

Confirmed by running it on a project whose goals are distinct:

```
qa-cycle-11-demo — the pill says: Agents 12
                   the sheet lists: 12 rows (converged after 2 pages)
                   VERDICT: the two agree on 12.
```

The verdict now separates "never scrolled, fix the swipe" from "scrolled,
and the labels repeat" rather than naming one cause for both (M-399).

**PR:** [#670](https://github.com/unarbos/arbos/pull/670), harness only.

## Cycle 122 — a sentence about the app that was a fact about a regex

The bad-token recording from cycle 94 came back reviewed, and it flatly
contradicted a verdict the sweep has printed for weeks:

> the Projects list never shrinks or collapses. There are still exactly 7
> rows. Instead of disappearing, the status text under the project names
> changes to "Off".

The scenario said `a token that cannot work empties the list to 1`.

**It was counting rows by their status.** `rows()` matched only
`<name>, (Idle|Working)`. A refused token leaves every row where it is and
changes what each one says, so six rows that turned `Off` stopped matching
and were counted as gone (M-400). Measured on today's build:

```
  rows now: 7 (1 still reading Idle or Working)
  what the rows say: 6×Off  1×Idle
  what the screen says:  Hub token refused.
  rows after restoring the token: 7
```

The one still live is the local kernel, which does not go through the hub.

The verdict now says what happens: **all seven rows stay and are marked
off**, the screen says why, and all seven come back.

**This is the second time this week a recording has beaten the tree**
(M-401; M-372 was the scroll resting under the pill). Both times the tree
was read correctly and the *question* was wrong — "how many rows say Idle"
is not "how many rows are there", and a still cannot show where a scroll
comes to rest. Neither would ever have failed on its own.

**PR:** [#673](https://github.com/unarbos/arbos/pull/673), harness only.

## Cycle 123 — the same fault, one file over

M-400's shape was "a count of rows filtered by their status word, described
as a count of rows". That is a shape you can grep for, so I did, across every
scenario.

Three counts carry a status word in their pattern. **One was the same fault**
(M-402): `cold-start-and-history` waited for a row reading `Idle` or
`Working`, counted those, and called the result "a list of N rows".

On the tree this loop captured at cycle 112:

| | |
|---|---|
| rows, any status | **12** |
| rows reading Idle or Working | **7** |
| what the other five said | `mac is asleep` |

So "a list of 7 rows" for a list of twelve — and a list whose machines were
all asleep would have waited the full 40 seconds and reported `never`.

**The other two were already honest** (M-403). `worker-while-it-works` says
`sheet rows after scrolling to the end: N   of them live: M`, naming both
quantities. `settings-and-a-bad-token`'s `live_rows` is deliberately
status-based and is called that. The difference is not care — it is that
they say which quantity they are reporting.

That is the whole lesson of the last four cycles in one line: a number is
only a measurement if the sentence around it says what was counted.

**PR:** [#674](https://github.com/unarbos/arbos/pull/674), harness only.

## Cycle 124 — barge-in, committed at last, and what the orb says afterwards

**The row was a memory.** Barge-in was measured at cycle 77 with ad-hoc
commands and never written down — the exact trap M-270 named when the mic
rig had to be rebuilt from scratch for the same reason. It is
`barge-in.sh` now, reading the two metrics the app already emits rather
than timing anything itself (M-404):

```
run 2: it stopped talking after 192 ms, the gateway confirmed at 237 ms
run 4: it stopped talking after 335 ms, the gateway confirmed at 381 ms
VERDICT: speaking over it stops it, every run that could — 263 ms on average
         (cycle 77 measured 181 ms)
```

**Its first version reported `1 of 3`** and called that worse than never.
The app had written the reason for both other runs — `barge_in_skipped reply
already over`, `barge_in_not_armed clip already used` — and I had not read
them. Neither is barge-in failing (M-405). Runs that never had a chance are
counted apart now.

**The recording said the orb never changes.** It does: measured off the
stills, the orb region reads **(127,127,126)** listening, **(94,94,93)**
thinking, **(71,88,111)** speaking — dimmer and blue. The review was right
about that recording and wrong about the orb, because that call spent most
of its length in one phase (M-406).

**Which is the finding worth keeping.** Sampling the phase every two seconds
against the console:

```
10s speaking      barge at 202 ms, gateway confirmed at 247 ms
12s listening     response.done reason=completed playing=false
14s thinking
...
28s thinking      — no further events at all
```

After a barge-in, the call says **thinking** about a reply that will never
arrive, until the caller speaks again. `settle()` runs only when a reply
reached the caller, and the code says why: "a reply that reached nobody must
not take the screen back to listening". That is true, and it swaps one wrong
statement for another (M-407). A third thing to say is a design call, so it
is filed rather than changed.

**Recording:** `media/mobile/cycle-124/recording_demo.mp4`.
**PR:** [#677](https://github.com/unarbos/arbos/pull/677), harness only.

## Cycle 125 — I wrote a confident sentence about behaviour I had not run

The oldest row left was `the harness — typing into the composer`, last
measured at cycle 72. It holds: **4 of 4 typed lines arrived, 4 of 4 whole**,
against M-162's 6-of-8 loss in the cycle-48 journey (M-408).

The scenario's header says "run it with the app on a project chat", and
nothing checked that (M-409). So I added a guard — and in its comment I
explained what would otherwise happen:

> Run from the projects list there is no composer to focus, every line goes
> nowhere, and the run reports `0/4 arrived`.

Then I ran it from the projects list. **2 of 2 arrived, 2 of 2 whole.** The
list has its own composer — `Message phone…` — and it sends to the project
it names (M-410).

The guard was right. Its reason was invented, inside a change whose entire
point was that a precondition should be checked rather than asserted. I have
spent six cycles finding instruments that stated more than they measured,
and wrote one into the fix for the seventh.

Corrected in a second commit rather than an amend, because the wrong
sentence is the useful part of the record.

**PR:** [#680](https://github.com/unarbos/arbos/pull/680), harness only.

## Cycle 126 — the journey passes, and the check that guards it did not work

The last phone-only row, `journey — the phone-only steps P1, P2, P3`, was
measured at cycle 72. A full journey run against `pod` says it holds: **P1,
P2, P2e and P3 all pass**, with J1–J3, J4a/J4m, J5q/J5s and J7/J7v besides
(M-411). The photo step is worth a word — it passes on the model naming the
picture ("Ice plant flowers, predominantly magenta"), not on the reply
failing to sound like a refusal, which is how it passed for months while
attaching nothing.

**But J1 did not open a project.** "Open the project from the list" tapped
`phone` at **y=85** — the chat's own header — and passed, because the chat
the app woke in happened to be the target (M-412). The journey has the fault
the last six cycles have been clearing out of the scenarios, and the check
written for it never looked at the journey, which lives outside
`scenarios/`.

**So I pointed the check at it, and the check stayed silent — with the step
deleted.** Two faults, in opposite directions (M-413):

- a one-line helper, `score() { …; }`, opens a brace and closes it on the
  same line; the body-skipping took that as entering a function and ignored
  every use in the rest of the file;
- it recorded the **last** `reach_the_list`, not the first, so a file that
  reaches the list at J1 and again after the J6k relaunch looked like one
  that never did.

Both fixed, and then demonstrated the only way that means anything: delete
the journey's step and the check speaks; restore it and the check falls
silent. That rule has now been narrowed and widened across three cycles, and
the version I trust is the one I watched fail on purpose.

`sleeping-machine` was the last scenario genuinely missing the step.

**PR:** [#683](https://github.com/unarbos/arbos/pull/683), harness only.

## Cycle 127 — 31 more connects, and no new worst case

`call — the microphone path` was last measured at cycle 79, where M-285
settled the long-running "connect-time drift" look: it was never drift, it
was two engines. Re-reading every call log the loop has written:

| engine | n | min | median | p90 | max | mean |
|---|---|---|---|---|---|---|
| duplex | 101 | 284 | 543 | 732 | 898 | 526 |
| openai | **68** | 999 | 1951 | 2800 | **5773** | 2089 |

Still no overlap — the slowest `duplex` connect (898 ms) is faster than the
fastest `openai` one (999 ms). `openai` has 31 more samples than at cycle
79, and `duplex` none, which is the phone having moved to GPT Live.

**The useful number is the max.** It has not moved: 5773 ms then, 5773 ms
now, across 31 further connects. So the 6000 ms hold's margin of 227 ms is
thin but *stable*, not eroding — which is a different thing to worry about,
and worth knowing before anyone spends a cycle widening the hold (M-414).

**And the scenario told me the wrong thing when I misused it.** It takes an
output directory where every other scenario here takes a cycle number. Given
`127` it said `no logs under 127`, which reads as "there are no logs" rather
than "that is not what I take". I fell into it on the first run (M-415). A
bare number that is not a directory is now named for what it is.

**PR:** [#685](https://github.com/unarbos/arbos/pull/685), harness only.

## Cycle 128 — the last eye-judged row, counted

`style pair vs Cursor stills — worker chat` has been judged by eye since
cycle 37. Its claims are all statements about the accessibility tree —
"plain header, no composer, the header reads Back" — so none of them needed
an eye (M-416):

```
  the header names                   count slowly one to forty
  back control reads                 1
  composers (M-154: none)            0
  send buttons                       0
  microphone buttons                 0
  controls in the header             1

VERDICT: the shape holds
```

`style-pair.py` is not the tool for it. It measures row pitch and ground
colour, which are properties of a **list**, and it refuses this pairing
(M-353) — which is honest and left the row unmeasured. This is the part of
it that can be measured.

**And I ran the same counts on the neighbouring screen**, because a shape
check that would pass on the project's chat measures nothing. One Back away:
**1 composer and 3 header controls**, against the worker chat's 0 and 1
(M-417). The absent composer is M-154's deliberate choice, so a `TextField`
appearing here is a regression, not an improvement — the check treats it
that way.

**One smaller thing**: `pod` left the roster between two runs an hour apart,
and the scenario said `no pod row` — which reads as the taps having failed
rather than a project having gone. It prints the list it did see now
(M-418).

**PR:** [#689](https://github.com/unarbos/arbos/pull/689), harness only.

## Cycle 129 — do the instruments agree?

Every coverage row the simulator can reach is current, and a great deal of
the harness changed today. So this cycle ran the instrument set together
against `main` `02a4ef65` rather than measuring anything new.

They agree (M-419): every tool resolves inside the repository, every
scenario reaches the list before it needs it, the list's rows all name a
state and none speaks a separator, and `check-names` visited **5 of 5**
screens — which it can now be trusted to say, because it refuses a clean
verdict on any screen it did not open.

**One disagreement, and it was real.** `check-names` found `Image  Image` in
a project's chat: an element whose label is its own type. Two places draw a
photo with no label — the sent photo in the transcript, and the composer's
preview chip, whose **remove button** has read `Remove <name>` since cycle
108 while the chip beside it read `Image` (M-420).

Fixed, and verified the way cycle 109 taught me to: by finding the photo on
screen and reading its label, not by opening a chat that might not have one.

```
 263  163  Image        Photo you sent
```

The transcript's label counts them when there are several — three photos
each announcing "Photo you sent" is the same fault one step along.

**PR:** [#691](https://github.com/unarbos/arbos/pull/691), the `ios/` batch.

## Cycle 130 — the photo flow, watched rather than counted

A recording was due, and the photo path is the one surface with a history of
silently doing nothing — the journey's P2 passed for months while attaching
no photo at all, because it scored on the reply not sounding like a refusal.

Recorded end to end and reviewed. **It is whole** (M-421): `+` →
`Photo Library` → pick → the chip appears instantly with its remove `x` →
type → send → the photo drawn in the outgoing bubble above the words → the
reply. Nothing clipped, overlapping or stuck. And the sentence the model
gave — "A dense bed of magenta ice plant flowers with scattered purple and
yellow blooms" — is in the kernel's own record, so the photo that was
described is the photo that was picked.

**The review found a wording fault the tree would never flag.** The live
line under the sent photo read:

> Working Thinking · 5s

The chat prefixes `Working` to whatever step the kernel sends. `Starting`
and `waiting on …` had each been special-cased when they turned up;
`Thinking · 5s` had not (M-422). The kernel sends two kinds of step and they
need opposite treatment — a state it has named is already a sentence, an
activity needs a word in front — and it capitalises the first kind and not
the second. Two exceptions of one shape were a rule nobody had written down.
Now written, and verified live: `Working` with no step, `Thinking · 5s` with
one.

**One thing checked and left alone**: the three-second "Loading…" before the
system picker. No such string exists anywhere in the app — it is `PHPicker`
loading the library in the simulator (M-423). Recorded so the next person
watching the recording does not go hunting for it in our code.

**Recording:** `media/mobile/cycle-130/recording_demo.mp4`.
**PR:** [#693](https://github.com/unarbos/arbos/pull/693), the `ios/` batch.

## Cycle 131 — a regex that stopped at the first full stop

Two build numbers arrived this cycle. Writing the first, I replaced the line
with a regex ending `[^.]*\.` — and `[^.]*` stops at the first period, which
in `uploaded 0.2.0 (1997) …` is inside the version number. The line came out
as:

> It is **2110**, written by the steward after #693 reached `main`.2.0
> (1997) 6b96bbb7` on the #633 ios-testflight run.

The edit reported success (M-424). Repaired by rewriting the whole line
rather than patching the patch, and it now reads **2117** from
`uploaded 0.2.0 (2117) 9c00a389`.

**The audit that was this cycle's plan came up clean** (M-425). After cycle
130's "Working Thinking", I read every file with several literal string
special-cases. `EndpointDirectory` is a `key: value` config with two named
keys and a documented bare-URL fallback; the other chain in `MainChatView`
renders markdown's own syntax. Neither is exceptions standing in for a rule.
Worth writing down: the audit's value is what it clears as much as what it
catches.

**And the mirror got a door.** It refuses to copy a store file shorter than
the Mac's, which is right — that shape is usually a truncation. Twice today
a deliberate shortening hit the refusal and I went around it with a bare
`scp`, which skips every other check the mirror makes. A refusal with no way
past it is a refusal people go around, so there is one now, and it costs
naming the file (M-426):

```
mirror: REFUSED … store copy 6456 b is shorter than the Mac's 6499 b.
mirror: if you have read it and the shortening is deliberate, say so by name:
mirror:   SHORTER_IS_DELIBERATE=<name> deploy/mobile/mirror-docs.sh <name>
```

Proven both ways by lengthening the Mac's copy on purpose.

**PR:** [#694](https://github.com/unarbos/arbos/pull/694), harness only.

## Cycle 132 — the feedback poll was reading from somewhere else

Two builds went out since the last poll (1997 and 2117), so this cycle
polled. **Nothing new**: 19 screenshots, 0 crashes, and F1–F19 stand as they
are (M-427).

The poll printed two lines of shell error above that report, which is what
the cycle turned out to be about.

**It was running its poller from a second clone.** `poll-feedback.sh` began
`cd ~/arbos-tools` — the shape that cost this loop forty cycles of tooling,
twice. Here the directory does not exist, so the `cd` failed, the `&&` chain
stopped, and the run carried on in whatever directory it was started from.
It worked only because that happened to be a checkout too (M-428). It also
sourced `~/.op-env` without testing for it; it says where the credentials
came from now.

**`check-tools` had reported this tree clean all day.** It looks for
`~/tool.sh` and not for `cd ~/some-checkout`, which is the same reach
wearing a coat (M-429).

Widening it went wrong twice, in opposite directions, and both are worth
the record:

- `~/arbos` is the Mac's own checkout, so it must be exempt — but an
  exemption with **no boundary** exempted `arbos-tools`, the single thing
  the widening was for, and the report went clean with my probe still
  sitting in the directory;
- the boundary then had to be "not a name character" rather than "a slash",
  because `cd ~/arbos &&` ends in a space.

Proven by planting the probe and watching it caught, then wrongly cleared,
then caught again.

**PR:** [#695](https://github.com/unarbos/arbos/pull/695), harness only.

## Cycle 133 — running the edits I had not run

Between cycles 116 and 120 the reach-the-list step went into about ten
scenarios. Four of them were never exercised afterwards, and the sweep does
not cover those four. So this cycle ran them (M-430).

Three pass: `sleeping-machine`, `photo-reaches-the-model` — "the model named
the picture — the photo DID get through" — and `worker-while-it-works`.
**One was broken**, and fixing it uncovered two faults in the helper itself.

**`several-workers` taps its project twice.** Cycle 116 gave the first tap
the step and not the one after "back and reopen", where the run died:
`nothing matching 'phone' on screen`. One Back from the sheet lands on the
chat, not the list (M-431).

**Then `reach_the_list` said it had succeeded while the sheet was still
up.** It tested for the *absence of a Back button*, and the workers sheet is
a modal with no Back — so "no Back" read as "already on the list". It
succeeds on *seeing* the list now (M-432). That is the third time this one
function has had the blind-success fault, and each time the same mistake:
testing for the absence of the wrong thing.

**And then it gave up one look too early.** The loop checks, acts, sleeps —
so the last action is never verified. It printed `cannot see the projects
list after four tries` while the list was on screen a second later, because
the fourth swipe had worked (M-433).

With all three fixed: `4 of 4 named in the sheet, and the pill reads Agents
47 after going back and reopening — the workers are kept`.

**The check that found none of this now says what it does not cover.** It
reads the first use in a file, so a scenario returning to the list halfway
through can fail exactly the way it guards against. That boundary is printed
with every pass.

**PR:** [#697](https://github.com/unarbos/arbos/pull/697), harness only.

## Cycle 134 — measured before guessing

The note said opening a chat with workers still clips by about 93 pt, and
asked for a measurement before a third guess. Measured on `main`
`b82e2194`:

| chat | unused scroll on opening | last line vs the pill |
|---|---|---|
| `phone` | **1 pt** | 87 pt above it |
| `demo` | **0 pt** | 109 pt above it |

It does not reproduce (M-434). A still confirms it: the last line reads
whole, well clear of the pill. #638's second scroll and #642's bottom margin
hold together, and nothing was shipped.

**The 93 pt was itself a bad number.** It came from cycle 110, where I took
one swipe for the end of a transcript. The real shortfall then was **501
pt** — which is what #638 fixed, and why measuring it properly mattered more
than fixing it quickly.

**So the measurement is a file now, not a memory** (M-435). It separates the
two claims that were being conflated: *landing* at the end of the
transcript, and the last line being *clear* of the pill that floats over it.
A chat can do the first and fail the second, which is exactly how #633
shipped a fix aimed at the wrong mechanism. It pages four times rather than
once, and refuses to speak when it cannot read both positions.

Anyone tempted to change this surface again can run it and put the numbers
in the PR.

**PR:** [#698](https://github.com/unarbos/arbos/pull/698), harness only.

## Cycle 135 — half of a row's name

The coverage row reads "projects list — faces, rows, **sections**".
`list-rows.sh`, which I wrote at cycle 112 and marked the row current with,
measures rows and says nothing about sections. Half the name had never been
checked (M-436).

`list-sections.sh` checks the three claims `ProjectsView` makes: a header is
drawn only when it has rows under it, tapping it folds them away and back,
and a live project belongs to `Working` while one without a link rests in
`Read`.

The first two hold:

```
headers on screen:  Working absent, Read at y=175
rows on screen:     8
  under Read:       8
folding 'Read':
  rows after folding:   0   (was 8)
  rows after unfolding: 8
```

`Working` being absent is the behaviour — nothing is working — not a fault.

**And the new check tested two of its own three claims.** Membership needs
both sections on screen at once, and every project rested in `Read`, so no
row could be *seen* to be in the right one. The first draft would have
printed a clean verdict having never looked (M-437). It says so now:

> Membership: not tested this run: only one section was on screen, so no row
> could be seen to be in the right one

Writing the claims into the header is what made that visible — the prose and
the code disagreed, in a file written to fix exactly that disagreement one
level up.

**PR:** [#699](https://github.com/unarbos/arbos/pull/699), harness only.

## Cycle 136 — the faces, and a rule checked against the wrong key

"projects list — faces, rows, sections": rows were measured at 112,
sections at 135, and the faces never, because the accessibility tree cannot
see one. A row is `const, Idle` whatever colour its folder is. So this reads
pixels — the one thing in this harness that has to — at the row position the
tree gives, rather than hunting for rows in the image the way `find_row.py`
did before it opened the wrong project twice.

**The faces hold** (M-438). Across two cold launches all eight are
unchanged, with six distinct colours; `purple` and `teal` are each worn by
two projects, which an eight-colour palette makes ordinary and which the
check reports rather than counts against anything.

**And I nearly shipped a wrong measurement.** The first draft computed the
name-derived colour — FNV-1a modulo the palette — and reported:

```
faces matching the name-derived rule: 0 of 8
```

which reads as every project wearing a face somebody chose. It was the check
that was wrong: the app hashes the **target**, `hub:<machine>/<project>`,
and the list shows a machine for almost no row, so that key cannot be built
from the screen at all (M-439).

**Zero of eight should have been the giveaway.** With eight colours, chance
alone gives about one match; zero is the shape of a comparison that is not
comparing. I committed it before doing that arithmetic, and the rule is now
named as unchecked rather than checked wrongly.

**PR:** [#700](https://github.com/unarbos/arbos/pull/700), harness only.

## Cycle 137 — the rows whose names promise more than their checks

Cycles 135 and 136 each found one: a coverage row naming three things with
a check covering one or two. This cycle read every row at once, against the
scenarios that claim it.

**The clearest remaining gap was "attachments (`+`), photos, files"**, where
every measurement is about photos and nothing had ever opened the Files
picker (M-440).

It cannot be driven here. The picker opens on an empty `Recents`, and
`Browse` reads **"On My iPhone is Empty"** — the app does not declare
`UIFileSharingEnabled`, so a file dropped into its own Documents, which this
cycle tried, is nowhere the picker can look. Making it visible means
changing the app to suit the harness, which is the wrong way round.

So the gap is written down instead. What *can* be driven is now checked, and
it is not nothing: the `+` menu offers Files at all, the picker opens, and
the way out is clean — the last of which has history, since a picker left
open in journey run 32 swallowed the photo line and the whole call step
after it.

```
VERDICT: the + menu offers Files, the picker opens and closes, and
         cancelling leaves the composer as it was.
         Attaching a file is NOT exercised: … and says so.
```

**The audit also caught a slip of my own** (M-441). `call — barge-in` still
read **77** in the table, though cycle 124 measured it and wrote the
finding. The ledger was under-reporting the loop's own coverage. Reading
every row's last cycle in one list is worth doing for its own sake: that
table is the only thing that says what has gone stale, and it is kept by
hand.

One gap named and left for a later cycle: `call — voice first, orb,
colours` was last marked at 95, and the orb's **colours** were measured
ad-hoc at 124 — listening (127,127,126), thinking (94,94,93), speaking
(71,88,111) — without being committed anywhere.

**PR:** [#701](https://github.com/unarbos/arbos/pull/701), harness only.

## Cycle 138 — the orb's colours

Last cycle named this gap: the row is "call — voice first, orb, **colours**"
and `orb-phases.sh` read the phase *labels*, with "colour" appearing only in
its header comment (M-442). A caller in a quiet room has the colour and
nothing else, so an orb that changed its accessibility value while looking
identical would be a fault the sequence check cannot see.

Sampled once per phase, the first time it is entered:

| phase | on screen |
|---|---|
| listening | RGB (229, 229, 229) |
| thinking | RGB (178, 178, 178) |
| speaking | RGB (128, 167, 218) |
| connecting | RGB (178, 178, 178) |

The three a caller meets mid-call are distinct — bright, dim, and blue —
not only in the tree.

**`connecting` and `thinking` are the same grey** (M-443). My first version
said so flatly, and that would have sent the next reader after a colour
nobody has to tell apart: connecting happens once, before the call is under
way, so nobody is ever choosing between the two by eye. Two **mid-call**
states sharing a face would be the real fault, and the check now says which
kind it found:

> each pair involves a state that happens once, before the call is under
> way, so nobody is choosing between them while listening. Worth knowing,
> not a fault.

That distinction is the cycle's actual work. Finding two identical numbers
is easy; saying whether they matter is what stops a finding from becoming
somebody's wasted afternoon.

**PR:** [#702](https://github.com/unarbos/arbos/pull/702), harness only.

## Cycle 139 — measuring the thing the still was named after

`the-core-chat-path.sh` has saved a screenshot called `02-streaming` for
eighty cycles and never measured streaming. It timed the card, the first
word and the Worked line — all of which a reply arriving in one lump passes
exactly as well as one growing word by word (M-444). Only the caller sees
the difference, and they see it for the whole length of the answer.

**It took three attempts, and the numbers caught the first two** (M-445):

- the first sampled the reply's length *after* finding its first word, by
  which time a three-sentence answer is already whole. It reported "only
  ever caught at one length", honestly, which is how the timing flaw showed;
- the second watched from the send but took *the last line matching a word
  the reply might start with* — which matched the **previous turn's** reply
  too, and reported **243 characters then 123**. A length going down is not
  a reply growing. That nonsense is the only reason the flaw was visible;
- the third measures the transcript's **total** text, which only grows while
  a turn runs and needs no guess about the reply's first word.

```
  the reply starts:        6.9s
  the transcript grows in: 2 step(s)   299 → 549 characters on screen
```

A run that catches only one length now says it cannot tell streaming from a
whole reply, rather than implying either.

**And the build number stopped being a matter of belief** (M-446). `2051`
came with `232518c2` — the #659 merge at 14:59 UTC — against `2117`'s
`9c00a389`, the #693 merge at 19:52. `git merge-base --is-ancestor` orders
them: 2051 is the older upload, and the lower number agrees. That test is
now written into the file itself, so the next round resolves without
anybody's memory.

**PR:** [#704](https://github.com/unarbos/arbos/pull/704), harness only.

## Cycle 140 — the settings sheet, printed and unread

M-306 measured the sheet at cycle 87: five sections, three `Token saved`
fields, a build line at the foot. Every run since has **printed** those into
a log that nothing compares (M-447). A section quietly lost would have sat
in the output of a passing run.

They are checked now, against cycle 87's own numbers:

```
  sections: Settings; Voice; Arbos kernel; Mesh hub; Notifications;
  section count: 5   (M-306 counted 5 at cycle 87)
  saved-token fields say: 3   (M-306 counted 3)
  build line: Arbos, 0.2.0 (1)
```

All three hold. A missing build line is a fault rather than a printed
`MISSING`, because it is the only place the phone says which build it is
running.

**And that line cannot answer the question this loop keeps being asked**
(M-448). On the simulator it reads `(1)` — the locally built number — under
the words "The TestFlight build on this phone". True on a real install, a
trap here, and the check now says so when it sees it:

> that is the local build: on the simulator this line cannot say which
> TestFlight build anyone has — only a real install can

Most of today's messages have been about which TestFlight number is current.
This line looks like the place to settle that and is not, which is worth a
printed sentence rather than a reader's assumption.

**PR:** [#705](https://github.com/unarbos/arbos/pull/705), harness only.

## Cycle 141 — into the sweep, and what the first run found

Eight checks have been written since the sweep's list was set, and none of
them were in it. They ran when I remembered, which is exactly how four
scenarios went seventeen cycles without a run at cycle 133 — three passed
and one had been broken the whole time (M-449).

The sweep is nineteen now: **19 scenarios, 19 conclusions, none silent and
none verdict-less.** What stays out is written down with its reason — the
ones that drive a real call, the ones that wait two minutes by design, the
one that needs a composer already open — so nobody has to work out whether
an omission was deliberate.

**The first run with them in found a regression**, which is the argument for
the change making itself. `list-rows` failed:

```
  PUNCTUATION:     phone: '·'
```

That is M-370, fixed in #642, and cycle 128 read the row cleanly. By now it
reads `phone, Idle,  · , home, 39m` again — with `.accessibilityHidden(true)`
still sitting in `ProjectsView`, untouched (M-450).

It is the same failure as the worker line's glyph at cycle 117: the modifier
takes once and then stops holding. The form that worked there works here —
make the status line a single element and write its label, `Idle, home`.
Twice now a hidden-modifier fix has passed once and quietly come undone; the
one-element form has not.

**PRs:** [#706](https://github.com/unarbos/arbos/pull/706) the sweep,
[#707](https://github.com/unarbos/arbos/pull/707) the `ios/` batch.

## Cycle 142 — five clean runs over nothing

The sweep on `main` still showed `list-rows` failing on `phone: '·'`, with
[#707](https://github.com/unarbos/arbos/pull/707) merged and in the build.
That is twice now a fix **inside** the row has read clean once and come
undone with the code untouched (M-451): `accessibilityHidden` on the
separator at 128, then one element with a label at 141b. A `Button` composes
its label by walking what is under it, and the walk is what varies.

So the sentence is stated on the button itself, built from the row's own
parts: `phone, Idle, home, 13m`. Nothing underneath can compose a different
one.

**Then I proved it five times over nothing** (M-452).

`.accessibilityElement(children: .ignore)` takes the button's *traits* along
with its children, so the rows stopped being Buttons. `list-rows.sh` finds
rows by looking for Buttons, matched none, and printed:

```
rows on screen: 0
  rows:            0
  with a state:    0 of 0
VERDICT: every row names a state, ages read as ages, and nothing
```

Five runs of that, and I read them as five confirmations. The verdict is
true of an empty set and says nothing about the app.

Both halves are fixed. The row keeps `.isButton`, so a row you can tap
announces itself as one. And the check refuses an empty set outright —
`VERDICT: none — no project rows matched at all` — because a check that
passes on nothing is worse than no check: it is evidence.

Re-run with both in: **11 rows** counted on each of three runs, all clean.

**PRs:** [#710](https://github.com/unarbos/arbos/pull/710) the check,
[#711](https://github.com/unarbos/arbos/pull/711) the `ios/` batch.

## Cycle 143 — where the undone fixes live, and a screen examined one control deep

Two fixes inside the projects row came undone; two of the same shape
elsewhere have held since cycle 109 and 117. The difference is what they sit
inside (M-453): a `Button` builds its label by walking its children, and
that walk varies between runs. The chat's worker line and the away card's
bullet are not composed button labels, so a modifier on them stays put.

I re-checked those two properly — three runs, with **two worker lines and an
away card provably on screen** — because clearing a chat that lacks the
thing being cleared is exactly how cycle 117 fooled me. They hold. The rule
is written into `check-names.sh`: when it flags something under a button,
say it on the button.

**Then the count I added an hour ago earned itself.** `check-names.sh` now
prints how many controls it examined, because "every control here is named"
over three elements is a different sentence from the same words over thirty.
The settings sheet read:

```
== the settings sheet ==
  every control here is named   (1 examined)
```

One. That screen is six text fields, five headings and a button, and the
detector looked only at buttons, images and pop-ups (M-454). Fields and
headings carry names too — a field labelled by a symbol would be exactly the
fault this file exists for.

Widened, the same screen now reads **(12 examined)**, and the list 18, the
sheet 13, the chat 7, the call 3.

The count was added for one reason and found something else within the hour.
That is the argument for printing what an instrument looked at, not only
what it concluded.

**PR:** [#712](https://github.com/unarbos/arbos/pull/712), harness only.

## Cycle 144 — verdicts that claimed what they only printed

Carrying on the audit: which verdicts can fire on a measurement that never
happened? Two.

**`the-core-chat-path.sh` ends "and the composer cleared"** — and only ever
printed the composer's contents (M-455). A send that left the typed line
sitting in the box would have produced the same sentence, which matters
because that is a real failure this loop has seen: a line typed and not
sent, or sent and not cleared, is how M-162 began.

It is judged now — cleared means a placeholder rather than the words that
went out — and the judgement **tests itself before it runs**: feed it a
placeholder and it must say cleared; feed it the line just sent and it must
say not. If it cannot tell those apart it declines to judge rather than
guessing, because a branch that has never fired is a branch nobody has read.

**`cold-start-and-history.sh` could print `never s to a list of 7 rows`**
(M-456) — two claims that cannot both be true. If the wait timed out then
nothing was timed, and the landing was read afterwards. A timeout now says
so and names neither.

Neither fault would have shown in a passing run, which is the whole point of
going looking. Both files print their evidence; the question each time is
whether anything reads it.

**PR:** [#713](https://github.com/unarbos/arbos/pull/713), harness only.

## Cycle 145 — twenty cycles of blaming the wrong thing

M-407 has sat open since cycle 124: after a barge-in the orb reads
`thinking` until the caller speaks again. I filed it as a display fault —
the app promising a reply that will never come — and left it for a decision
that never arrived.

It still reproduces on `main`: barge at 288 ms, confirmed at 330, then
`thinking` for sixteen seconds with no frames in between.

**So I changed the phase handling to settle back to listening, and it
changed nothing** (M-458). Same phases, same sixteen seconds. The one-line
change sat behind `settle()`'s own guards, which is why it was safe — and
why it did nothing: the guard was blocking, because a turn really is
pending.

**The orb is telling the truth** (M-457). After the interrupt the app
forwards the interrupting speech, sets `kernelBusy`, and waits. The phase
returns to `thinking` *after* my settle runs. The kernel's record shows
nothing from the call at all, and the app's log ends at
`response.done reason=interrupted` with no frame after it.

So the fault is a **turn that never completes** after a barge-in — a
gateway question, not a label. And the comment I overrode,

> a reply that reached nobody must not take the screen back to listening

was right. Whoever wrote it had understood the state better than I had.

The change is reverted in the same cycle it was made, and the finding is
rewritten to describe what is actually wrong. Twenty cycles of calling this
a display bug rested on never having asked what the call was waiting *for*.

**PR:** [#715](https://github.com/unarbos/arbos/pull/715) — the `ios/`
branch, now empty of behaviour change, kept for the record of the attempt.

## Cycle 146 — the menu nothing had opened

Three items have sat behind the chat's `···` since it was built: `Call
<project>`, `Reconnect`, `Settings`. The button's own name was checked at
cycle 108; what is under it never was (M-459).

It offers what it should, and the call item names its project — `Call
phone`, not a bare "Call", which is the difference that matters when
somebody with several projects open is about to talk to one of them.
Settings from that menu opens the sheet.

Reconnect is judged by **what the chat does**, because a reconnect that
changes nothing and a menu item that does nothing are the same tap. The
transition was not caught this run, and the check says that says nothing
either way — a reconnect over a live link can finish inside one sample. A
chat that never comes back is the fault, and it came back.

**Two bugs of mine, both loud and both silent** (M-460):

- `"Opening $ROW…"` — bash reads `$ROW…` as a variable name, so the loop
  died with `ROW…: unbound variable` twenty times over **while the verdict
  printed as though it had watched**;
- `ui dump | grep -q` closes the pipe on its first match, and the reader
  died of `BrokenPipeError` mid-scenario.

Errors in the output and confidence in the conclusion is the combination
this loop keeps having to remove, and I wrote a fresh instance of it in the
same file that was meant to close a gap.

**PR:** [#717](https://github.com/unarbos/arbos/pull/717), harness only.

## Cycle 147 — four ways to accuse the app of my own bugs

The call's `≡` menu had never been opened by anything, like the chat's the
cycle before. It holds `Back to chat`, a route toggle and `Hang up`, and
they are covered now: the menu lists all three and Back lands in a chat with
a composer, three runs running (M-461).

The route toggle is the interesting one, and the interesting part is that it
**cannot be judged here**. `toggleSpeaker` flips a preference and then reads
back what CoreAudio actually did; a simulator has one output and nothing to
switch to, so the label is right not to move. That half belongs to the
AirPods row, which needs Jacob's phone.

**Getting to that took four versions, and three of them blamed the app**
(M-462):

1. the label did not flip, so — fault. It cannot flip here;
2. judged against the route instead, but the route was unreadable, so the
   chain fell through to `FAULT: the route changed to ''` — an accusation
   assembled from an empty string;
3. `offers: nothing` from a menu it had **just listed**, because it looked
   once to decide the menu was up and a second time to read it, and the menu
   closed in between;
4. tapped a menu that had closed itself after the toggle, then called the
   missing chat a fault. One run in two.

Every one of those printed a confident sentence about the app. The common
rule is small and I keep relearning it: **unknown must not fall through to
an accusation.** Missing evidence gets its own branch, and that branch comes
first.

**PR:** [#718](https://github.com/unarbos/arbos/pull/718), harness only.

## Cycle 148 — pressing the one control nothing had pressed

The journey's J5 has carried the note "Stop not exercised by this step (the
control exists)" since it was written, and nothing else touched it. It is a
control the kernel itself advertises: the stall line reads "Stop ends the
turn".

So a Stop that only quietened the phone would be the app repeating a promise
the kernel made and not keeping it — and from the screen those look
identical. This is judged on the kernel's record (M-463).

Against a `sleep 75`:

```
  the send disc became Stop: yes
  the square went away:      yes
  transcript right after Stop: 3448
  and thirty seconds later:    3448
     3446 user          stop 030429: run the bash command sleep 75 …
     3447 interrupted
     3448 turn_complete
```

The `interrupted` line is the kernel's own, and the count not moving over
thirty seconds is what rules out a turn that merely looks stopped.

In the sweep, so it stays pressed.

**PR:** [#719](https://github.com/unarbos/arbos/pull/719), harness only.

## Cycle 149 — a character the rig drops

The tool fold came from Jacob at build 956 — "I don't want to see these tool
calls unless maybe I expand" — and nothing had checked it since.
`fold37.sh` is cycle-37 scratch: it reads `~/mobile-out/cycle-37` and takes
its tools from `$HOME`, so it cannot run today.

The new check found the fold holds: consecutive calls collapse to
`2 tool calls, · 1 failed`, and opening it adds two rows (M-465).

**Getting there turned up a rig fact worth more than the check** (M-464).
The first run reported "no fold line appeared" — and the kernel's record
showed my request had never arrived at all. `idb ui text` types **nothing**
for a line containing any non-ASCII character:

```
  "plain ascii line"        -> non-ascii bytes: 0   … lands
  "with an em dash — here"  -> non-ascii bytes: 3   … composer still empty
```

My line had an em dash. The scenario sent an empty box, got no answer, and
blamed the fold. That is now guarded for every scenario: `type_line` in
`sim-lib.sh` refuses a non-ASCII line before typing and reads the field back
after, so M-162's lesson and this one are one helper rather than a habit.

**Three more of my own faults came off on the way**, each of which read as a
fault in the app:

- counting rows across the toggle while the reply was still arriving — the
  "extra row left behind" was the model's answer landing;
- calling a **one-call** fold's empty expansion a fault, when the summary
  line *is* that call and an opened fold shows labels rather than output;
- a `case` pattern with a character range that refused a pure-ASCII line,
  because collation is not content.

**PR:** [#722](https://github.com/unarbos/arbos/pull/722), harness only.

## Cycle 150 — a lesson that stayed in one file

Cycle 149 lost a run because `idb ui text` types nothing for a line
containing a non-ASCII character. Today I went looking for who else does it.

**Nobody types a literal em dash** — the audit of every `idb ui text` in the
harness came back clean. But eight scenarios type a **variable**, so the
protection has to be at runtime, not at review.

And the lesson was already written down (M-466). `call-pulled-down.sh` has
carried this since it was written:

> Plain ASCII only. `idb ui text` maps characters to key codes and throws
> "No keycode found" on anything outside the keyboard — an em dash in this
> very line cost a run.

It sat in that file and protected that file. A comment is read by whoever
opens the file it is in, and the next scenario is written by somebody who
has not.

**Worse, the scenario I wrote yesterday typed and sent blind** (M-467).
`stop-a-turn.sh` had no read-back at all — the fault M-162 named, which
every other scenario had already fixed inside its own body. It was written
the day before `type_line` existed and not revisited when it did.

Both are on the helper now, and both still pass: Stop closes the turn
(`3484 user → 3485 interrupted → 3486 turn_complete`, transcript flat across
thirty seconds), and the pulled-down call still types, mutes and closes back
to the chat it came from.

The pattern is worth stating once: **a lesson kept as prose protects one
file; a lesson kept as a function protects the ones not written yet.**

**PR:** [#723](https://github.com/unarbos/arbos/pull/723), harness only.

## Cycle 151 — the tree was right and the app was not

The sweep ran all twenty-one scenarios and every one reached a conclusion.
One failed: `list-rows`, on `phone: '·'` — the separator #711 removed, on a
`main` that contains #711.

I measured it rather than guessing: **6 of 6 dotted**. Then rebuilt from the
*identical commit* by hand: **6 of 6 clean** (M-468).

**The binary was stale.** The cause is this loop's own habit — scp a
modified source to the Mac, build, then `git reset --hard` it away. The
restored file can land with an older timestamp than the object built from
the scratch version, and the incremental build keeps the object. So
`mac-cycle.sh` checked out the right commit, reported the right sha, and
compiled an app from something else.

`mac-cycle.sh` deletes the built products whenever the reset discards
anything. One full compile in that case and nothing the rest of the time.
Verified end to end: `cleared 2 local change(s)`, rebuild, 6 of 6 clean.

**And it rewrites three findings** (M-469). M-451 said two fixes inside the
row "held once and came undone"; M-452 was the third attempt after them.
Every one of those was measured on a build taken straight after a
scp-and-reset, and a stale binary produces exactly that story. A probe label
reached the tree intact — `PROBE-phone, Idle, home, 8m` — which is the
evidence the modifiers were working the whole time.

The app fixes stand. The account of them was wrong, and any measurement from
cycles 141–150 taken right after a scp-and-reset is suspect until re-taken.

That is the most expensive kind of fault this loop can have: not a wrong
answer, but a wrong answer that looks like the app misbehaving, three times
over.

**PR:** [#726](https://github.com/unarbos/arbos/pull/726), harness only.

## Cycle 152 — the worker chat, paired in numbers

The row had been read by eye since cycle 74. Cycle 83 built `style-pair.py`
to retire the eyeball for the list, and the tool itself said row pitch means
nothing on a chat and told the reader to "pair chats by their parts." This
cycle gave it those parts.

**The pair holds** (M-471). Text starts **5.3%** of the width in against
Cursor's **4.9%** — within half a point, on phones of different sizes. Both
headers open with the same round left chevron. Ground is printed and never
compared, dark-only being deliberate on both sides (M-202).

**The headers carry different things, on purpose** (M-472). Cursor centres
the project title and puts a `···` on the right. The worker chat puts a tick
and the worker's *goal*, left-aligned, with no right-hand control. Neither
is a defect: a worker chat has no actions to offer, so a `···` would open
onto nothing, and the goal is the only thing that tells one worker from
another — a centred project name cannot.

**And the tool nearly told me a lie** (M-473). Its first reading put the
header 4.3% down; that was the status bar. Skipping the status bar it said
8.3%, which would have made Arbos's header a quarter the height of Cursor's.
One picture, two confident answers, both wrong — because a band ends
wherever contrast drops, and a header with a dim round button beside bright
text splits into two while a flat one stays whole.

So the header numbers are gone. The tool prints the band count, refuses to
compare it, and names the instrument that *can* answer — the accessibility
tree, which knows a header's parts by name. What is compared is the left
margin, which is the leftmost ink in the body and does not care what the ink
is. The measurements will not run at all until they have read a picture
whose answers are known, and that check has been shown to fail when the
measurement is wrong.

Stills and crops in `media/mobile/cycle-152/`.

**PR:** [#728](https://github.com/unarbos/arbos/pull/728), harness only.

## Cycle 153 — re-measure, and the thing I got wrong yesterday

Cycle 151 found a real symptom: a clean checkout of `main` produced an app
that behaved like code from before a fix that checkout contained, six runs of
six, while a rebuild from the identical commit read clean. I named a cause —
timestamps surviving a `git reset --hard` — and shipped a remedy for it.

**The cause does not reproduce** (M-474). Put a visible marker in
`ProjectsView.swift`, build: twelve rows carry it. `git reset --hard`,
rebuild with nothing deleted: zero rows carry it. The reset is picked up
correctly, every time I ask. Worse, deleting `Build/Products` — the remedy
itself — recompiled nothing, so what "verified" it at cycle 151 was the
manual full rebuild that happened just before.

The remedy is withdrawn from #726 before it merged. The symptom was real. Its
cause is still unknown.

**What replaces it is a question rather than a cure** (M-475). Two things
were never asked. `simctl install`'s result was never checked, and an install
that fails leaves every later run measuring whatever was already on the
device — which fits cycle 151 exactly. And nothing ever asked whether the app
on the device *was* the code that was checked out. Now both are asked: the
install must succeed, and the installed binary is dated against the newest
`.swift` file. Whatever makes the device disagree with the tree leaves an app
older than the code, and the run stops there.

Proven both ways: a clean run says "the app on the device is newer than every
source file"; touch one source and it says "CAUGHT — app older than
ProjectsView.swift".

**My first test of that check was wrong twice** (M-476). It read as passing
when it should have caught, because `find -newermt "@<epoch>"` does not parse
on BSD find — it matches nothing, which looks exactly like a pass. And I was
testing by scp-ing `mac-cycle.sh` onto the Mac and running it, which cannot
work: the script's own `git reset --hard` restores the script from git
partway through. Both fixed; the branch is pushed and checked out before it
is tested.

**On the re-measure**, the rest of cycles 141–150 stands. Those findings were
positive results about long-standing behaviour — the `···` menu opens, Stop
ends a turn, the tool fold folds — and a stale binary invents no such thing.
The two that a stale binary would explain are the separator findings, already
rewritten. M-458 would need the post-barge orb phase changed again, which is
under standing orders not to touch.

**PR:** [#729](https://github.com/unarbos/arbos/pull/729). #726 merged at 05:08,
before the correction was ready, so the withdrawal is its own change rather
than an edit to that branch.

## Cycle 154 — a check that could only ever decline

The sweep has carried one standing "cannot say" for many cycles:
`pill-count-vs-sheet`, last reading "the sheet's paging found 26 of the
pill's 34."

I read the check's own comment and it had already answered itself (M-477).
The pill is `chat.workers.count`. The sheet is a `ForEach` over that same
array. They are two drawings of one number, so they cannot disagree about the
app — every "cannot say" it ever printed was about the rig failing to page a
scrolling sheet or failing to tell two workers apart, and the single
disagreement it filed at cycle 74 turned out to be exactly that.

I looked for an independent referee first. The kernel has no roster verb —
`history`, `read`, `frames`, `hello`, `feedback` — and the app builds its
worker list from the same frames, so counting them would reproduce the app's
own logic rather than check it.

**So the verdict moved to something the run knows independently: three
workers it starts itself**, each goal led by a tag no earlier run used. The
pill must rise by three and the sheet must show all three. A row dropped in
the drawing fails it; a rig that cannot reach the end fails it too, and says
which.

First run: **3 of 3**. `p051856 one`, `p051856 two`, `p051856 three`, all
present, where the old check declined.

**And the totals gap finally has an exact explanation** (M-478). Pill 38,
sheet 30, paging converged over five pages, zero duplicates on any single
screen. The difference is eight, and the project holds eight more agents than
it has distinct goals — twins on *different* pages, which a per-screen
duplicate check cannot see by construction. That reads the rig honestly, so
the numbers stay; they are simply no longer asked to judge the app.

**PR:** [#730](https://github.com/unarbos/arbos/pull/730), harness only.

## Cycle 155 — the recording, and what it got wrong

The standing order asks for a recording every third cycle. The last was cycle
130, so this one was long overdue.

I filmed the workers flow: ask for three workers, watch them start, open the
sheet, open one. Then had it reviewed.

**The review reported a bug that was mine** (M-479). It said the three
workers appeared in the chat, the pill rose from 38 to 41, and none of them
were in the agents sheet. That is exactly what the footage shows — because my
recording script scrolled the sheet once. Paged properly, both workers from a
second run are there: `s052622 alpha, Done` and `s052622 beta, Done`, six
pages down.

A recording that scrolls less than a check does will accuse the app of
whatever it failed to reach. So the recording is now a committed scenario
that pages to the end the way the checks do, and prints what it found before
anyone watches the film. Its run: two asked for, two running in the chat, two
in the sheet, 35 MB of footage.

**Two more things the review raised, both checked before filing.** The first
worker line carries a number the others lack — `2 Working <name>` against
`Working <name>`. That is deliberate and the code says so: "as the desktop
draws them: the first carries the count" (M-480). And it read a line as
middle-truncated with the worker's name lost; the frame at that moment shows
all three lines whole, and these lines are `.tail`, not `.middle` (M-481). A
model reading compressed footage is one more instrument that needs checking
against the source.

**One thing worth someone's attention** (M-482): the two workers started
seconds earlier sat six pages into a 35-row sheet, which is ordered oldest
first. On a phone, the worker you just started is the one you want and the
furthest to reach. Recorded as an observation, not sent anywhere.

Film and stills in `media/mobile/cycle-155/`.

**PR:** [#732](https://github.com/unarbos/arbos/pull/732), harness only.

## Cycle 156 — the number the row was named after

Oldest-first put `background 8 s → foreground` at cycle 86, seventy cycles
back. A check for it does exist, inside `cold-start-and-history.sh`, so the
row was not untested — but reading it showed the row's own question had never
been asked (M-483).

It pressed Home, waited eight seconds, relaunched, **slept a fixed three
seconds**, and compared transcript rows. How long the app takes to be usable
again — the thing the row is named for — was never measured. Worse, the
comparison happened at an arbitrary moment: a screen still being drawn at
three seconds would read as "the chat changed while away", and one that took
ten seconds would read as a pass.

It now waits for a composer and a transcript, times that, and compares once
the screen has settled. A resume that never finishes says so rather than
comparing nothing.

Measured on this build:

    back to a chat you can type in: 0.9s   (after 8s away)
    text rows before: 21, after: 21
    VERDICT cold start:   3.1s to a list of 10 rows (6 live)
    VERDICT long history: chat in 1.1s, pager Show 200 earlier lines
    VERDICT away and back: usable again in 0.9s and the chat is as it was

The fixed sleep was more than three times longer than the app needs, so it
was hiding the number and padding every run that used it.

**One thing to watch in the ledger** (M-484). Oldest-first named `list
composer` (81) and `cold start` (86) as the most stale rows, and both had run
in the cycle-151 sweep. Coverage cells are only written when a cycle names a
row, so anything the sweep covers ages on paper while being tested every
time. The genuinely untested thing here was the timing, not the coverage.

Still: `media/mobile/cycle-156/01-back-from-away.png`.

**PR:** [#733](https://github.com/unarbos/arbos/pull/733), harness only.

## Cycle 157 — the journey, and the build it could not name

Oldest-first put the journey row at cycle 96, and the last run was 21 hours
back, so this cycle ran it.

**It runs clean** (M-485). Fifteen steps pass, two are scored by eye, and
four are unverified with a stated reason each: J4's mid-flight half, J5's
Stop not being exercised by that step, J8 needing a kernel restart the phone
cannot cause, and PUSH being off until the key exists. The kernel was asked
on the attach socket at both ends and answered `c3247332dc4e` both times. All
three phone-only steps pass, with the model naming the photograph: *Ice plant
flowers, predominantly magenta.*

Worth noting in passing: the list row read `phone, Idle, home, now`. The
separator stays gone on a build the loop made itself.

**Two faults, both in the journey rather than the app.**

The run record has always said `"branch": "main@unknown"` (M-486). `APP_BUILD`
was read with a default at the single place it is used and set nowhere, so
every journey record ever written names the kernel it talked to and not the
app it drove. It now carries the checkout's branch and sha — `main@959e5960`.
That is only the app's build while the installed binary is not older than the
sources, so the run asks that question, the same one `mac-cycle.sh` asks
after installing, and writes the doubt into the record when the answer is no.

And I started the run by handing it `157` (M-487). The journey takes a target
— `pod` or `<machine>/<project>` — not a cycle. It accepted the number,
walked six steps, and failed with `J1 FAIL no 157 row on the list`, which
reads like the app lost a project. It now refuses a target that cannot be
one, the same guard `connect-times-by-engine.sh` already carries.

Evidence: `media/mobile/cycle-157/` and the 19 stills in
`media/mobile/journey/0919-062153/`.

**PR:** [#737](https://github.com/unarbos/arbos/pull/737), harness only.

## Cycle 158 — measuring the thing that picks the work

Cycle 156 found the rotation rule pointing at the wrong rows. `list composer`
read as last checked at cycle 81 and `cold start` at 86, and both had gone
through the cycle-151 sweep. A row the sweep covers ages on paper while being
tested every run, so the loop's own scheduler was sending each cycle to
places that did not need it.

This cycle made that measurable (M-488). Every scenario declares the row it
serves, as a `# COVERS:` line, and `coverage-map.sh` reads those against the
table:

    coverage rows:             33
    rows some scenario claims: 24
    rows the sweep reaches every run: 18

The nine rows nothing claims are the genuinely untested ones: AirPods/CallKit
and the TestFlight build, both of which need Jacob's phone; the microphone
path; the two journey rows; `recording`, whose scenario is sitting in
unmerged #732; the whole-app style pair; typing into the composer; and the
loop's own machine.

**The tool caught its own fault before it caught anything else** (M-489). It
read the sweep's list from `SCENARIOS=`, which is the argument array, rather
than `DEFAULT=`, which is the default list — and printed an empty section
under a verdict that said everything matched. It now refuses to report at all
when it has read no scenarios, which is the difference between an empty
answer and a false one. Proven the other way as well: one mistyped
declaration turns the verdict and exits 1.

**It reads and does not write** (M-490). Having it stamp the current cycle
onto every row the sweep touches would be one line, and it would be wrong:
the rows would age correctly while saying nothing true about whether anybody
looked at them. The ledger's worth is that a cycle *named* a row.

Still: `media/mobile/cycle-158/01-the-list.png` — ten rows, the separator
still gone. The map's output is beside it as `coverage-map.txt`.

**PR:** [#741](https://github.com/unarbos/arbos/pull/741), harness only.

## Cycle 159 — the map was reading a quarter of the harness

Cycle 158's map named nine rows as untested. Four of them were not (M-491).
It read `# COVERS:` declarations out of `scenarios/` only, so it missed the
journey, the style pair and the name check — and reported **both journey
rows as uncovered on the morning after a journey run**.

Those entry points declare their rows now. 29 of 33 claimed, against 24. The
four left are real: AirPods/CallKit and the TestFlight build, which both need
Jacob's phone; `recording`, whose scenario is in unmerged #732; and the loop's
own machine.

That left one uncovered row I could take today, and it was the right one
(M-492). Every scenario that sends anything goes through `type_line`, and
more silent passes in this loop have come from typing than from anything
else — `idb` types nothing at all for a line holding non-ASCII and reports
success, and iOS turns a typed quote into a curly one so no read-back
matches. The row had no check of its own.

It has one now, asking the three ways typing fails: a plain line arrives
whole, a long line arrives whole (207 characters, ending intact), and a line
the rig cannot type is refused with the composer left as it was. All three
hold.

**And the third question could not fail** (M-493). I removed the ASCII guard
from `type_line` to watch the check catch it, and the check passed. Without
the guard, `idb` types nothing, the read-back loop gives up, and `type_line`
returns failure regardless — so "it refused" proved only that something
refused, not that the guard existed.

The two differ in time. The guard answers before typing anything; the
read-back spends three attempts of six waits each first. Measured: **0.0s
with the guard, 31.1s without**, and anything over three seconds is now
called what it is.

That is the second time this week a check of mine passed a sabotaged
subject. Both were found the same way, by breaking the thing on purpose, and
neither would have been found by reading the code.

Still: `media/mobile/cycle-159/01-a-line-typed-whole.png`, with the map's
output beside it.

**PR:** [#746](https://github.com/unarbos/arbos/pull/746), harness only.

## Cycle 160 — the machine, and a diagnosis that cost eight cycles

The last reachable uncovered row was the loop's own machine. `check-tools.sh`
asks whether the scripts reach for tools inside the checkout; nothing asked
whether the machine under them has what those tools need.

I wrote that check expecting it to confirm a known gap — cycle 152 recorded
that the Mac had no PIL, which is why `style-pair.py` has been run on this VM
ever since, with screenshots copied across. **The check said PIL was
importable** (M-494).

The Mac has two interpreters. A command that sets no PATH gets Xcode's
python or `/usr/bin/python3`, which has neither PIL nor `websocket`. Anything
setting the harness PATH gets homebrew's, which has both. The module was
installed the whole time; cycle 152 ran the wrong python, read the traceback
as a property of the machine, and moved a coverage row onto another machine
for eight cycles.

So the check asks both interpreters, because a report from the good one alone
looks clean and hides precisely that failure. It names the six files that
break under a bare command. And `style-pair.py` now catches the import and
says which interpreter is running, so the next reader gets a sentence rather
than a traceback.

**Then it failed again, for a different reason** (M-495). With the right
python it printed the arbos numbers and threw a traceback: the Cursor
reference stills live in the store and had never been copied to the Mac. They
are in neither the checkout nor the mirror's file list, so a rebuilt Mac
would hit the same wall with no way to know. The tool now names the missing
still and where it belongs, the machine check counts the references, and they
are copied across. The pair runs on the Mac: **5.3% against 4.9%**, the same
numbers cycle 152 obtained by shipping screenshots elsewhere.

**One self-inflicted wrinkle** (M-496). The dependency scan anchored imports
at the line start, so wrapping style-pair's PIL import in a `try` — done in
this same cycle to improve the error — took that file off its own
dependency's list, and the report blamed `find_row.py` alone. Indentation is
allowed now. The file that handles a missing module gracefully is the last
one you want dropped from the list.

The machine reports clean: every command, every module under both
interpreters, every folder, 12 voice clips, 2 references, 179G free, one
booted simulator.

Stills: `media/mobile/cycle-160/01-the-list.png`, with the machine report
beside it as `check-machine.txt`.

**PR:** [#747](https://github.com/unarbos/arbos/pull/747), harness only.

## Cycle 161 — the sweep of twenty-four, and a fault that keeps its secret

Four checks were outside the sweep with no reason written down, which is the
state the sweep's own header exists to prevent (M-497). Two were plain
oversights — `tool-fold` from cycle 149 and `call-menu` from 147. Two drive a
real call and belong out, but nobody had said so. Every scenario in the
folder is now either in the sweep or named as out, including the one-offs
from cycles 37 to 39 that are kept as a record of how something was measured
once rather than as checks.

`typing-into-the-composer` runs first of all, because everything after it
that sends anything goes through the helper it checks.

**Twenty-four ran, none silent, none without a verdict.** `pill-count-vs-sheet`
gave a real answer rather than declining, which is the cycle-154 rewrite
working in place.

**Two faults. The separator is the interesting one** (M-498). Six of six
dotted during the sweep, on a build the new freshness check certified as
current — and six of six clean an hour later from the same commit. A probe
label reached the tree intact in the clean runs, so the explicit label is in
effect then.

This cycle eliminated every explanation I had:

* a stale binary — the app was certified newer than every source file;
* a second row call site — there is exactly one, used by both sections;
* `spoken` inserting the dot — it joins with `", "` and nothing else;
* the build step — `mac-cycle.sh` produced a clean row minutes after
  producing a dotted one, from an untouched tree;
* two sections being open — a captured run had two headers and a row reading
  Working, with a clean label.

So there is no fourth fix this cycle. What every report of this has lacked is
the screen it happened on: three fixes have each held once and each was
judged on a one-line flag. `list-rows.sh` now keeps the tree, a screenshot,
the row and section counts, and the date of the binary, into a folder, when
it fires. Proven by forcing the judge to report a fault.

**The second fault is the tool fold** (M-499), which showed nothing more when
opened — four rows closed, four open — on a turn with two tool calls, one
failed. Filed rather than chased: a fold with two calls may have nothing
further to show, and the check cannot yet tell that from one that refuses to
open. Its label also reads `2 tool calls, · 1 failed`, which is the same
stray separator in a second place.

Evidence in `media/mobile/cycle-161/`: the sweep summary, the clean list, and
an example of what the new capture keeps.

**PR:** [#750](https://github.com/unarbos/arbos/pull/750), harness only.

## Cycle 162 — four faults in one check, none of them the app

Cycle 161's sweep left `tool-fold` reporting `FAULT: opening the fold showed
nothing more (4 → 4)`. None of it was the app (M-500).

**It counted the wrong thing.** It compared how many `StaticText` rows were
on screen. Opening a fold pushes the transcript up, so it loses as many lines
off the top as it adds below — four to four, with the fold open. It compares
the lines themselves now.

**It made those lines unique.** Two calls that ran the same command have the
same label, so a fold of six would have reported as hiding three. That is the
workers-sheet mistake of twenty cycles ago, in a new place. Duplicates are
kept.

**It tapped words the screen had stopped showing.** The fold's count climbs
while the turn runs. The check read `3 tool calls` early and tapped those
words later, when the screen read `6 tool calls`. It re-reads the line
immediately before each tap and prints what it read.

**And it never checked whether the tap landed.** A tap that found nothing
read as an app that would not open.

All four were load-bearing. The final run reads the fold as 3, re-reads it as
**6** before tapping, and opening reveals **6** lines — including `bash ·
echo one` twice, which the unique count would have called three. Repeated
once more for the record: six again.

**The check also could not reach its own subject** (M-501). It asked for
"three bash commands one after another" and the model sensibly ran one call
holding `echo one; echo two; echo three`. Two runs in a row folded a single
call, and the scenario honestly declined to test opening — which means the
thing it exists for went untested most of the time. Combining is forbidden
now.

**What closed this was the screenshot** (M-502). The tree said the tap
succeeded and nothing changed, which is a fault report with no explanation in
it. The still showed `> 6 tool calls`, chevron closed, a different number
from the one being tapped. Two cycles running, the picture has caught what
the accessibility tree reported faithfully and misleadingly.

Evidence: `media/mobile/cycle-162/01-the-fold-open.png` and
`tool-fold-run.txt`.

**PR:** [#751](https://github.com/unarbos/arbos/pull/751), harness only.

## Cycle 163 — a sixth dead end, and the dot was never the thing I said it was

The separator had one hypothesis left that I could test cheaply: every dotted
reading came from a list longer than the screen, and a scrolling list recycles
its rows. A recycled row losing its accessibility modifiers would fit
everything.

It does not (M-503). Scrolled the `phone` row off the bottom of a ten-row
list and back: clean. That makes six explanations eliminated with evidence —
a stale binary, a second call site, the spoken label itself, the build step,
two sections being open, and now recycling. All six are written into
`list-rows.sh` beside the capture, so the next cycle spends its time
somewhere new.

**And the recording corrected my framing** (M-504). The reviewer looked at the
list and described `Idle · home`. The dot is *drawn*, on purpose, on every row
that has a machine part — rows without one show `Idle` alone. So this fault
has only ever been about whether the row **speaks** it, and I had let that
blur across several cycles of shorthand. The check says so now: anyone who
sets out to remove the dot from the screen has misread the finding.

**The recording itself is clean** (M-505). No clipping, no stuck spinner, no
half-drawn transition; the sheet paged to its end on camera and the worker
chat opened. The reviewer flagged two things, both known and both deliberate:
the first worker line carries the running count where the others do not
(desktop parity, M-480), and one project shows no age on the right (the hub
gives no timestamp for it). Two independent reviewers have now read that count
as a mistake, which is worth a designer's minute even though it is not mine to
change.

Film and still in `media/mobile/cycle-163/`.

**PR:** [#752](https://github.com/unarbos/arbos/pull/752), harness only.

## Cycle 164 — an answer that had been waiting since cycle 32

The rotation's top six were nearly all rows the sweep runs every time. The
oldest row the sweep never touches was `several workers at once, archived
children`, last driven at cycle 88.

**Its first half holds**: four of four workers named on the sheet, and the
pill reading `Agents 40` after going back into the list and reopening.

**Its second half had never been driven at all** (M-506). "Archived children"
was filed as an open question at cycle 32 — a finished worker's chat read
*"Nothing on record yet."*, because the kernel answered `total: 0` for an
archived agent — and left there for 132 cycles.

It is answered. A Done worker's chat now holds its goal as the header and
five lines of what it did: `read · project-context.md`, `read · notes.md`,
`status`, and its closing message. The scenario opens a finished worker every
run now and names which of the three possible answers it got, so this cannot
quietly regress.

**Getting there caught a trap for the third time** (M-507). Picking "the first
Button below some y" taps the sheet's own drag handle — a Button called
`Sheet Grabber`, sitting exactly where a first row is looked for. The run then
stays on the sheet and reports on whatever it finds. Three scenarios have done
this. `first_worker_row` in `sim-lib.sh` picks by what a row says instead.

**And the rotation itself needed fixing** (M-508). It kept nominating rows the
sweep had run an hour earlier, because a row's age is the last cycle that
*named* it and the sweep names nothing. The map prints the actionable list
now — rows the sweep does not reach, oldest first — and its first honest
answer named this cycle's row at 88. Building it turned up a quieter fault:
finding ids look exactly like cycle numbers once the `M-` is gone, so `M-285`
had "the microphone path" reading as cycle 285 and sank rows that are
genuinely older.

Evidence in `media/mobile/cycle-164/`.

**PR:** [#754](https://github.com/unarbos/arbos/pull/754), harness only.

## Cycle 165 — the call row holds, and a fix I had to take back

Two scenarios cover `call — first word and transcription`, neither had run
since cycle 88, and both are out of the sweep because they drive a real call.

**Both hold** (M-509). One breath gives one transcript, one spoken answer and
one kernel answer, three runs of three, 900 of 900 frames. The first word
arrives whole: six runs of six kept every frame, at connects of 1226 to
1564 ms.

**The slow-connect check was too hard on itself** (M-510). With no connect
over two seconds it said "this run says nothing about the case in question" —
having just measured six runs that lost nothing. It reports what it kept and
over what range now, and names only the slow case as untested. Fixing it, I
named my counter `RUNS`, which is the scenario's own argument for how many
runs to do, and reset it to zero before the loop read it: a six-run check
quietly did two and numbered the second one `0`.

**The still found something the checks do not look at** (M-511). The call
screen draws `phone` with a red folder — `(229, 83, 61)` — and the list draws
the same project purple, `(118, 93, 209)`. In code the list keys a project's
face on `KernelTarget.stored`, which is `hub:<machine>/<project>`, while the
call screen's fallback keys on `<machine>/<project>`: a different string, so a
different hash.

I fixed that key. It changed nothing — the glyph stayed at `(229, 83, 61)`.
So the fallback is not the path being taken: the chat's roster identity is
supplying that face, and the two screens are reading identity from different
sources. **The change is reverted.** The finding is measured and stands; the
fix does not, because I cannot show it doing anything.

**And I read an empty grep as success** (M-512). `mac-cycle.sh` refused a
branch that exists only on the Mac — `cannot fetch fix165` — and my grep for
`BUILD` printed nothing, which I took for quiet success. I then measured a
screenshot of the previous app. The guard did its job; I did not read it. The
second reading, with the branch's real name, built and installed properly —
and the colour still did not move, which is how M-511 got its answer.

Stills in `media/mobile/cycle-165/`.

**PR:** [#757](https://github.com/unarbos/arbos/pull/757), harness only —
`ios/` ends this cycle untouched.

## Cycle 166 — two projects, one face

Cycle 165 measured `phone` drawn purple in the list and red on the call
screen, and left the question open. This cycle pulled it.

**The code said one thing and the measurement said another** (M-514). Reading
the source, the story was clean: the list applies the hub roster's face and
fills a missing colour by hashing `hub:<machine>/<project>`, while the chat
takes the kernel's identity straight off the `hello` frame with no filling.
So the list invents and the call screen shows the real thing — and the list
is the screen you look at most.

Then I measured a second project. `demo` draws **0xE5533D** under the orb,
exactly as `phone` does, while the list gives them different faces. A third,
`const`, draws **0x4C8DFF** — the first colour in the palette, which is what
`tint` returns when it recognises no colour at all.

**So the call screen is not drawing the project's face** (M-513). It draws
whatever identity the attached kernel sent, and one kernel serves several
projects, so those projects share a face. Where no identity arrived it falls
to `colors[0]`. The list, hashing each project's own key, is the screen
telling them apart.

**There is no fix in this cycle, because the fix is a decision** (M-515).
Making the kernel's face stick in the list needs two changes at once — the
chat writing the cache the list reads, and the roster not overwriting a
learned colour with a hashed one — and both rest on a question nobody has
answered: should the orb name the project you called, or the kernel you are
talking to? Filed, with the numbers, at
`internal/features-inbox/2026-09-19-one-project-two-faces-which-one-wins.md`.

`call-face.sh` measures it on every run, so whenever the answer comes there
is a check waiting for it. Its first run: three projects, two faces.

Stills in `media/mobile/cycle-166/`.

**PR:** [#759](https://github.com/unarbos/arbos/pull/759), harness only.

## Cycle 167 — a check that had been dead for sixty-five cycles

The work queue named `chat — the call's words read back`, last driven at
cycle 102. It did not run: it stopped at `project row not on screen`.

**The check had rotted** (M-516). It tapped `<project>, Idle`, which was the
entire row label when it was written. The row has since gained its machine
and an age — `phone, Idle, home, 5m` — so the string never matched again, and
every run since has stopped at that line. That is M-314's fault a second
time: a script tapping words the app no longer says.

It reaches the list and taps the project by name now, like everything else.
Re-driven, the row holds: `rows marked Spoken: 9; in this turn the answer
appears 1 time(s)` — one row for the spoken answer, which is the rule.

**Fixing it nearly broke something else** (M-517). I gave `list-rows.sh` a
`reach_the_list` call and it had no `sim-lib` line. The shell would have said
command not found, the helper would have returned 127, and the `|| exit 1`
beside it would have ended the run before it measured anything — while
`check-tools`, which looked only for the words, cheerfully reported that
every scenario reaches the list first.

It now lists any scenario calling a shared helper it has not sourced. It
matches calls rather than bare words, because `pt` appears in a comment about
pt coordinates in two old files and had them reported as callers. Proven by
removing the line again and watching it name `list-rows.sh`.

**And `list-rows.sh` had been hand-rolling the helper** (M-518): one `Back`
tap and carry on regardless, where `reach_the_list` tries four times and
confirms it can see rows. The scenario that guards the list's rows was the
one scenario not using the list helper.

Still: `media/mobile/cycle-167/01-the-calls-words-in-the-chat.png`.

**PR:** [#761](https://github.com/unarbos/arbos/pull/761), harness only.

## Cycle 168 — the chat caught up, and my ruler did not

The work queue named `background minutes/hours → resume`, last driven at
cycle 120.

**Both of its old cases hold** (M-519). Suspended for two minutes: the same
screen, the same last three lines. Process reclaimed, which is what a night
actually does: back in the chat he left, not the list.

**But every case in it left the project idle** (M-520), so "the same last
three lines" was always the right answer — and a chat that had reconnected
was indistinguishable from one merely still showing what it showed before.
The case a person meets after a night is the other one: work finishing while
the phone is in a pocket. Nothing exercised it.

It does now. Start a worker, go Home for ninety seconds, come back and ask
whether the chat knows. It does: left a turn running, returned to `Worked
1m 44s`.

**Getting that answer took two goes, because I used the wrong ruler** (M-521).
The first run said "the turn reads as over but the transcript did not grow",
seven lines then six — and told me to read the still before filing, which is
the one thing it got right. The still read `Worked 1m 44s`. Coming back
raises the keyboard and the view scrolls, so the row count falls while the
content grows. That is exactly the mistake the tool fold made at cycle 162,
six cycles ago, in a new place.

It reads the turn's ending line now — absent while the worker runs, present
on return — and declines when an ending line was already there before he
left, because then coming back to one proves nothing. The corrected run still
showed six lines then five, which is why the count had to go.

Evidence in `media/mobile/cycle-168/`.

**PR:** [#763](https://github.com/unarbos/arbos/pull/763), harness only.

## Cycle 169 — the journey holds, and the ledger sent me somewhere I had been

The work queue named `journey — the phone-only steps P1, P2, P3` at cycle
126. It was due, so I ran the journey.

**It is clean** (M-522). Fifteen steps pass, two are scored by eye, four are
unverified with a stated reason each. All four phone-only steps pass,
including the model naming the photograph — *Ice plant flowers, predominantly
magenta.* The kernel answered `c3247332dc4e` on the attach socket at both
ends, and the run record names the app it drove: `main@e8613d01`. That is
cycle 157's fix working; before it, every record said `main@unknown`.

In passing, J1 tapped `phone, Idle, home, 8m` — no separator.

**But the row should not have been at the top of the queue** (M-523). The
journey covers two rows, and cycle 157 credited only one of them. So `journey
— the phone-only steps` aged on paper from 126 while actually being exercised
at 157, and the rotation sent this cycle to re-run something that had passed
twelve cycles earlier.

That is M-484's fault in a smaller place: a row's age is the last cycle that
*named* it. The journey and the sweep now print the rows they cover, read
from the scenarios' own declarations, so whoever writes the ledger credits all
of them rather than the one they happened to be thinking about.

Neither writes the ledger. That stays a person's judgement, for the reason
cycle 158 gave: a script running near a row is not the same as someone
looking at it.

Evidence: `media/mobile/cycle-169/`, and 19 stills in
`media/mobile/journey/0919-122406/`.

**PR:** [#765](https://github.com/unarbos/arbos/pull/765), harness only.

## Cycle 170 — the orb has no colour for thinking

The queue named `call — the microphone path` at cycle 127, and a recording
was due. They went together.

**The mic path works**: connect 1.5–2.3 s, the clip transcribed correctly,
barge-in cutting playback at 306 ms and done at 351 ms.

**Then a number nearly became a regression** (M-524). `reply_first_audio` read
11.0, 11.1, 12.4 and 13.1 seconds across four runs, against 1249 ms recorded
three cycles ago — a tenfold slowdown, if the two were the same measurement.
They are not. One run of `call-text-in-chat` printed both on the same build:
1244 ms for its small talk and 12223 ms for its delegated question. The
gateway answers a greeting itself; a question about the project waits for the
kernel to think. `mac-voice.sh` asks the second kind, and says so now where
the number is read.

**The recording found the real thing** (M-525). Reviewed, it reported that the
app "looks stalled" during the ten seconds a delegated question takes, and
that thinking could not be distinguished from listening at all.

Measured, by the check's own sampling: connecting `(178,178,178)`, listening
`(229,229,229)`, thinking `(179,179,179)`, speaking `(128,167,218)`. Three
greys and one blue. Thinking and listening differ only in brightness — and
inside a range listening already travels on its own, as the voice rises and
falls.

The check had been comparing phases by distance in RGB, which passes a pair
the eye cannot separate. Two phases now count as sharing a face when neither
has a hue, and it catches this live.

**And its verdict had been reading clean over it** (M-526): the colour section
said "listening and thinking wear the same face mid-call" while the closing
line said "one pass through the phases, in order, no flapping". The verdict
carries it now.

No app change: what colour a thinking orb should be is a product decision,
and this loop's job was to show that it does not currently have one.

Film and still in `media/mobile/cycle-170/`.

**PR:** [#766](https://github.com/unarbos/arbos/pull/766), harness only.

## Cycle 171 — the style row had a tool and no check

The queue named `style pair vs Cursor stills` at cycle 160, the oldest row
that does not need Jacob's phone.

**Both surfaces hold** (M-527). The list's row pitch is 8.7% of screen height
against Cursor's 8.3%; the chat's text starts 5.3% of the width in against
4.9%. Both within half a point, on phones of different sizes. Those are the
same numbers cycle 83 got for the list and cycle 152 for the chat — sixty-nine
and nineteen cycles ago. Ground is printed and never compared, dark-only being
deliberate on both sides.

**The interesting part is why the row keeps ageing** (M-528). `style-pair.py`
has done the measuring since cycle 83, but nothing ever handed it the
pictures. Every reading of this row has been a person taking two screenshots
and typing two commands — so it aged eleven cycles between readings, and
cycle 152 spent an entire cycle discovering the tool could not even run on
the Mac.

It is a check now. `style-pair.sh` captures both surfaces and compares one
number each, with a point as the threshold: twice measured under half a
point, sixty-nine cycles apart, so a whole point is comfortably outside the
noise and far tighter than an eye. It refuses a verdict on a list too short to
have a rhythm, and it says plainly that a still of the wrong screen measures
perfectly and means nothing.

Proven it can fail: tightened to a tenth of a point, the verdict turns.

Stills and numbers in `media/mobile/cycle-171/`.

**PR:** [#767](https://github.com/unarbos/arbos/pull/767), harness only.

## Cycle 172 — three clean checks, and one that was flattering itself

The queue named `the loop's own machine` at cycle 160, so I ran the three
integrity checks.

**All three pass** (M-529). The machine can run the whole harness. Every tool
resolves inside the checkout, every scenario reaches the list first, and none
calls a helper it has not sourced. No control on any screen reads as a symbol
name.

**But the names check was flattering itself** (M-530). Its own comment says
the call "rests entirely on the labels of its four controls", and it examined
three: `Call menu`, `Settings`, and the orb — which reads `Call`, because
tapping it starts one. The fourth is `End call`, and it does not exist until a
call is up. So the screen was examined in the one state where its end control
is absent, and "every control here is named" was true of three quarters of it
without saying so.

A preview call on this simulator will not connect. With no clip the screen
says "No microphone input." and the orb does nothing at all; with an injected
clip it accepts the tap and stays where it is. `End call` is tapped by name in
`call-text-in-chat.sh`, on a call that is actually running, so the label is
covered — just not from here. The check now examines both states and names the
one it cannot reach.

**My first version of that was worse than the problem** (M-531): it counted
the unreachable state as a missed screen, so every run ended "incomplete". A
check that can never be complete is a check people stop reading. Withdrawn
inside the cycle.

**And the `ios/` batch, for whoever decides the next upload** (M-532): it holds
exactly two commits. One moved the project row's label onto the button; the
other put back the `.isButton` trait that move removed. Nothing else has
touched `ios/` since the 2160 build. The first is the separator work, whose
effect this loop has measured as intermittent across six eliminated
explanations; the second is a plain repair and is not.

Still and full report in `media/mobile/cycle-172/`.

**PR:** [#770](https://github.com/unarbos/arbos/pull/770), harness only.

## Cycle 173 — the separator, solved

The sweep had not run for twelve cycles, so I ran it: twenty-five scenarios,
all reaching a verdict, none silent. Two faults. `list-rows` reported the
separator again, and this time the capture built at cycle 161 was armed.

It recorded one line nobody had ever had before:

    the app on the device: ... Sep 18 10:27 ... Arbos.app/Arbos

The installed binary was from the previous day — twelve hours older than the
commits it was supposed to contain.

**Three scenarios reinstall the app** to clear remembered projects, and all
three installed from `/tmp/dd/Build/Products`, a directory some earlier cycle
left behind, holding a day-old build. One of them,
`list-search-filter-refresh`, runs **immediately before `list-rows`** in the
sweep (M-533).

Proven end to end:

| | binary | the row |
|---|---|---|
| the build the loop just made | `2026-09-19 11:37` | `phone, Idle, home, 10m` |
| after that one scenario runs | `2026-09-18 10:27` | `phone, Idle,  · , home, 11m` |
| with the fix | `2026-09-19 11:37` | `phone, Idle, home, 14m` |

**Cycle 151 was right and cycle 153 withdrew it** (M-534). 151 called this a
stale binary. 153 could not reproduce one and withdrew the claim — correctly,
because a remedy for a cause you cannot reproduce is a guess. Neither found
the mechanism, because both of us were looking at the *build*, and the swap
happens in a *scenario*. Six explanations were eliminated by experiment over
six cycles; the seventh was simply written down by a check, the first time it
fired.

All three scenarios now install the build this loop made and refuse rather
than run against whatever is on the device. `check-tools` flags any install
from a literal path outside the loop's derived data (M-535) — its first
version also flagged `mac-cycle.sh`, which assembles the same tail from a
variable and does it correctly, so it matches literal roots only. Proven by
putting `/tmp/dd` back and watching it named.

The other fault, `tool-fold`, was measuring that same stale app.

Evidence in `media/mobile/cycle-173/`: the sweep, the capture that named the
date, and the screen it named it on.

**PR:** [#774](https://github.com/unarbos/arbos/pull/774), harness only.

## Cycle 174 — the film I owed, and the name it cost

The recording was overdue from cycle 173. Made, reviewed, and it earned its
place: it found a real fault, and two of its other three observations were
artefacts of the instrument.

**The worker line was cutting out the worker's name** (M-537). The running
line reads `<count> Working <name> · <step>` on one line with
`.truncationMode(.middle)` — so the middle goes, and the middle is the name:

    2 Working t155034…· Reading notes.md

The step it preserved is the same few words on every worker. The name is the
only part that tells two apart, and it is what a person is looking for. The
step yields now, and the same capture reads `1 Working t155602 alpha`.
Batched with `ios/`; no upload.

**Two reviews reported this and cycle 155 dismissed both** (M-538). I checked
`line()`, which draws the transcript's subagent rows and is `.tail`, and
declared the report a transcription artefact. The live worker rows are a
different view — `WorkerLine` — and that one is `.middle`. The frame I
checked had short steps, so nothing in it was truncated; the claim needed a
long step and never got one.

**And the instrument lost the separator** (M-539). This cycle's reviewer read
the list row as `Idle  home`, with a gap and no dot, and called the gap a
missing character. At full resolution it is `Idle · home`. The demo sent for
review is 400 px wide and a one-pixel dot does not survive the scaling. Fine
typography belongs to the still; the film is for motion, clipping and stalls.

It also reported both worker rows carrying the count. The frame shows one:
`2 Working … alpha` above `Working … beta`.

Stills in `media/mobile/cycle-174/`, film beside them.

**PR:** [#775](https://github.com/unarbos/arbos/pull/775) — the first `ios/`
change in twenty cycles, batched.

## Cycle 175 — what the stale app was hiding, and what it was not

Cycle 173 found three scenarios installing a day-old build and fixed it. This
cycle re-ran the same sweep with the fix in, to see which verdicts had been
resting on that stale app.

**One changed, and it is the one that mattered** (M-540). `list-rows` now
reads "every row names a state, ages read as ages, and nothing speaks a
separator", where the identical check in cycle 173's sweep reported the
separator. Same scenario, same position in the run, one change between them.
#774 is confirmed by a second measurement rather than only by the hand-run
that motivated it.

**The other fault did not change, and my explanation of it was wrong**
(M-541). I wrote in cycle 173 that `tool-fold` "was measuring that same stale
app". It was not. It is main's copy of the *check* — whose repair, cycle 162,
found four separate faults in it and none of them in the app — sitting in an
open PR ever since.

Measured both on one build: main's copy prints `opening the fold showed
nothing more (4 → 4)`; the copy from #751 prints `'6 tool calls' hides 6
line(s) and gives them back`, listing `bash · echo one` twice. The sweep has
been printing a fault that was disproved thirteen cycles ago.

**Two harness fixes are stranded in open PRs** (M-542): #751 for the fold, and
#732 for the recording scenario, which I have been checking out by hand for
every film since cycle 155. The sweep's header now says to check whether a
fault's fix is already written and merely unmerged before chasing it — the
sweep runs what is checked out, and that is `main` unless a cycle says
otherwise.

Twenty-five scenarios, none silent, none without a verdict, one fault and it
is a stale check.

Evidence in `media/mobile/cycle-175/`.

**PR:** [#776](https://github.com/unarbos/arbos/pull/776), harness only.

## Cycle 176 — a check that always opened the oldest worker

The queue named `several workers at once, archived children` at cycle 164.

**Both halves hold** (M-543). Four of four workers named on the sheet, the
pill reading `Agents 48` after going back to the list and reopening, and a
finished worker's chat holding a record of what it did.

**But the archived half was opening the wrong worker** (M-544). It took the
first `Done` row on the sheet, which is always the same ancient one — `count
slowly one to forty`, from some cycle long past. The newest workers sit at the
*end* of the sheet, so this run's four were never on the first screen. The
check was proving that a months-old record survives, and saying nothing about
the four workers it had just watched finish.

It pages to them now: `paged 3 screen(s) to reach this run's workers`, then
`opening: w163711 rivers, Done  (this run's own)`, whose chat holds six lines.
It still falls back to any finished worker when it cannot reach its own, and
says so in the verdict when it does.

**And a suspicion that did not survive counting** (M-545). Three scenarios
spawn workers on `phone` every run, and the sheet now pages five or six
screens, so I set out to file unbounded growth. The numbers say otherwise:
34 at cycle 151, 43 at 161, 33 at 173, 44 at 175 — oscillating in the thirties
and forties rather than climbing. Nothing filed, but recorded, because "the
sheet keeps getting longer" is exactly the sort of thing that gets asserted
later by someone who has not counted.

Still and counts in `media/mobile/cycle-176/`.

**PR:** [#777](https://github.com/unarbos/arbos/pull/777), harness only.

## Cycle 177 — the last two eyeballs

The journey covers two coverage rows and was eight cycles old, past the
four-cycle rule.

**It is clean** (M-548) on `main@3647614d` against kernel `c3247332dc4e`,
asked on the attach socket at both ends. All four phone-only steps pass, the
model naming the photograph again.

**Two steps were scored by eye, and now they are not** (M-546). J6 and J6k
had done the same thing for their whole lives: background the app, relaunch
it, take a screenshot, and record a verdict without reading anything at all.
An acceptance run should not depend on somebody opening a picture, and the
standing order asks for counted evidence.

Both halves of each are in the tree. A chat is intact if it still has a Back
and a transcript; it has reopened at its end if there is a composer to type
into; and the away card says "While you were away" when something arrived
while the phone was down. Measured:

    J6  PASS chat intact on return (12 text rows) and an away card was waiting
    J6k PASS reopened at its end: a composer to type into and no card left pending

The record now reads **16 pass, 4 unverified, 0 eye**.

**One distinction worth keeping** (M-547). The away card is the half that can
legitimately be absent — nothing need arrive in forty seconds — so its absence
is unverified with the reason, not a fault. A chat that does not come back is
a fault, and says so.

The four unverified are the standing ones: J4's mid-flight half, J5's Stop
(exercised by `stop-a-turn.sh`, not by that step), J8's three cases the phone
cannot cause, and PUSH until the signing key exists.

Evidence in `media/mobile/cycle-177/`, and 19 stills in
`media/mobile/journey/0919-165418/`.

**PR:** [#779](https://github.com/unarbos/arbos/pull/779), harness only.

## Cycle 178 — a clean path, and the lesson from 174 made into a tool

The queue named `call — first word and transcription` at cycle 165, thirteen
cycles back.

**Both halves hold** (M-549). One breath gives one transcript, one spoken
answer and one kernel answer, three runs of three. The first word arrives
whole: six of six kept every frame, at connects of 1408 to 1955 ms. The slow
case stayed untested, as it does whenever the network behaves — the check says
so rather than claiming silence.

**The recording was due, and the core chat path had never been filmed**
(M-550). It is clean. The sent line sits right in its own bubble and the reply
arrives on the left as plain text; the reply streams rather than landing in
one lump, pushing the status line down as it grows; the view tracks the bottom
without jumping or overshooting; and the turn closes `Working` →
`Thinking · 5s` → `Worked 10s`, with the stop square reverting to a microphone
and `Follow up…` returning to the composer. Nothing clipped, nothing left
spinning. No fault — worth recording for a path nobody had watched.

**And cycle 174's lesson is a tool rather than a memory** (M-551). Every cycle
that films something has shrunk it by hand with an ffmpeg line typed from
memory, and 174 typed 400 px. At that width the separator dot is one pixel of
ink and disappears — the reviewer read `Idle  home`, called the gap a missing
character, and I spent the first part of a cycle checking an artefact of my
own downscaling.

`review-demo.sh` uses 540 px, keeps the reason beside the number, and closes
by saying what to ask a reviewer for: motion — clipping, stalls, jumps, a
spinner that never stops. Fine typography belongs to a still at full size.
This cycle's demo went out at 540 px and the review came back with no
typography complaints.

Its first version reported file sizes with `stat -f`, which is *format* on BSD
and *file system* on GNU — so on Linux the BSD form succeeded and printed
block counts, and the fallback never ran.

Film and still in `media/mobile/cycle-178/`.

**PR:** [#781](https://github.com/unarbos/arbos/pull/781), harness only.

## Cycle 179 — a verdict resting on an absence

The queue named `chat — the call's words read back` at cycle 167. The rule is
that a turn gets one row for its answer, not two — the kernel's wording and
the spoken one both showing.

**The check passed on a screen with no answer on it** (M-552). It counted how
often the kernel's wording appeared below the last `Spoken` marker, and read
zero as "one row, said in different words". Zero is also what an answer that
is not on screen looks like, and on the first run this cycle that is exactly
what it was: the marker sat last in the tree with nothing under it at all, and
the verdict said the rule holds.

The marker sits *between* a question and its answer, so rows under it are the
thing being counted. None there means nothing was counted, and it says so.

**Then counting rows called the furniture an answer** (M-553). The fix
reported two rows under the marker and still concluded "one row". I looked at
what they were rather than reasoning about it: `Worked 12s`, which every turn
ends with, and the answer. So it counts answers, excludes the turn's own
furniture, and prints the rows it counted so the number can be checked
against the screen.

**Measured properly, the row holds** (M-554): one answer row under the marker,
the kernel's wording appearing once, and the row printed beneath the count.

Still in `media/mobile/cycle-179/`.

**PR:** [#782](https://github.com/unarbos/arbos/pull/782), harness only.

## Cycle 180 — a shape, not four accidents

Four cycles in six found the same fault in four different scenarios: a check
that cannot tell "the thing is right" from "the thing is not there" (M-555).
The composer that was printed and never tested. The refusal check that passed
a helper with its guard removed. The row count that could not see a
transcript grow, because the view scrolled as much as it gained. And last
cycle, zero occurrences of an answer read as "one row, worded differently",
on a screen that had no answer on it.

Each cost a cycle, found by running a row and reading hard. So this cycle
looked for the shape rather than waiting to meet it a fifth time.

**The first attempt flagged twenty of twenty-five scenarios** (M-556), which
is the same as flagging none. A pass gated on zero is usually right — zero
faults, zero duplicates, zero rows left behind. What is suspect is a pass
gated on a count of things *observed* being zero, because nothing observed is
also what a broken rig produces. The two families are told apart by what the
variable counts, which is what its name says. That took it from twenty to
five.

**All five read sound** (M-557). Three of them decline on their zero, which is
the right thing. The fourth and fifth are one scenario passing on "no chips
left after tapping the ×" — the wanted outcome — and it checks separately that
a chip arrived at all before testing its removal.

That reading is in the tool, with the reason for each, so a later run shows
only what is new. A tool that reprints five known-good lines every time gets
skimmed, and then the sixth line is skimmed too. Proven by planting the shape
in `list-sections` and watching it named, then taking it out again.

No new fault today. The shape is not currently anywhere else, and there is
something watching for it now.

Audit and still in `media/mobile/cycle-180/`.

**PR:** [#783](https://github.com/unarbos/arbos/pull/783), harness only.

## Cycle 181 — a walk, and the two faces are bigger than filed

Several cycles running have been check repairs, so this one did the standing
order's purpose check directly: walk the app, film it, and look at it as a
product rather than as a set of measurements.

The list reads calmly — eleven projects, a folded `Read` section, ages where
the hub supplies them. The chat is legible, the composer clear, settings
plain.

**And the two-faces finding is on the chat header** (M-558). The same project
is `0x9A7AFE` purple in the list and `0xE5533D` red in the header — the same
red the orb shows. Confirmed on two projects.

That changes its weight. The call screen is seen rarely. The chat header is
seen every single time a project is opened.

**Reading the code for it corrected cycle 166** (M-559). I wrote then that a
fix would need two halves, the first being the chat writing the kernel's face
into the cache the list reads. That half already exists — `MainChatView` does
it on every identity change. The list *is* told.

It is then untold. `ProjectStore` overwrites it on the next roster refresh
with the roster's face, and `filled(key:)` substitutes a hash of
`hub:<machine>/<project>` when the roster carries no colour. It writes that
back into the cache as well, so a cold start reads the hash too.

**So the question is one line** (M-560), and the line has a comment saying it
is deliberate: "The roster's face (#233) beats the cache and the default."
When the roster's colour is empty, what beats the kernel's answer is not the
roster — it is a hash.

The inbox note asks that now: should a roster face with no colour of its own
beat a colour the project's own kernel supplied? No app change; it stays a
decision, and `call-face.sh` measures the list, the header and the orb every
run so it is not noticed by accident again.

Film and stills in `media/mobile/cycle-181/`; the note is at
`internal/features-inbox/2026-09-19-one-project-two-faces-which-one-wins.md`.

**PR:** [#784](https://github.com/unarbos/arbos/pull/784), harness only.

## Cycle 182 — a verdict that named both answers and chose neither

The queue put the returning-user row at cycle 168, fourteen cycles back.

**The first two cases hold** (M-562). Suspended two minutes: the same screen,
the same last three lines. Process reclaimed, which is what a night does:
back in the chat he left.

**The third failed, and its verdict was useless** (M-561). It came back to a
turn still running after ninety seconds and said "either the worker is slower
than its 60s sleep or the chat did not catch up" — naming both possibilities
and choosing neither, which is the same as saying nothing.

The kernel could choose. It had finished the turn at **2m 9s**, having
compacted 682 turns of history in the middle of it. So the ninety-second
window was too tight, and the app was never at fault.

It waits up to two minutes for the ending now, and when there is none it asks
the kernel: finished there and not here means the chat did not catch up;
still working there means there was no ending to miss. Re-run, the wait alone
settled it — `Worked 1m 43s`, the chat caught up.

**The kernel question is scoped** (M-563). Only `phone` is asked, because that
is the pod's own kernel folded into the roster and the only one this scenario
knows how to reach. Any other project says so rather than asking the wrong
kernel and reporting whatever it happened to be doing.

Evidence in `media/mobile/cycle-182/`.

**PR:** [#785](https://github.com/unarbos/arbos/pull/785), harness only.

## Cycle 183 — two causes wearing one verdict

The sweep had not run in eight cycles, with five of its scenarios changed
since. Twenty-five ran, none silent, none without a verdict (M-564). The app
held everywhere: rows clean, both surfaces within a point of Cursor's, the
core path at card 1.0s / reply 5.7s / Worked 7.2s.

Two faults. `tool-fold` is the check whose repair has sat in an unmerged PR
since cycle 162, already documented. The other was new.

**`notifications` said "no banner was posted within the window — the rest
says nothing"** (M-565), which names no cause at all. This file's own header
names the benign one: iOS keeps a backgrounded app alive for about half a
minute, so a reply slower than that never reaches the socket to be announced.
Earlier in this same cycle the kernel spent 2m 9s on a turn after compacting
682 turns of history.

The console can say which. No notify frame reaching the app means the reply
outran the window — the known limit until push is on. A frame received with
no banner shown is the app, and is the thing this check exists to catch.
Measured on the re-run: zero frames, so not the app.

**Writing that, I filed a false app fault for one run** (M-566).
`grep -c` prints `0` *and* exits non-zero when it matches nothing, so my
`|| echo 0` ran as well and the count became `0\n0` — which is not equal to
`0`, so the branch below announced "the app received 0\n0 notify frame(s) and
posted no banner — that is the app". A line written to avoid an empty
variable produced an accusation. Caught by reading the output instead of
believing it.

**And the verdict audit cried wolf** (M-567). It keyed known-good hits on file
and line, and cycle 179's edit moved one guard from line 105 to 111, so a hit
cleared three cycles earlier reported as new. Keyed on the file and the
variable now — which then surfaced a genuinely new one, cycle 179's own
guard, read and found sound.

Evidence in `media/mobile/cycle-183/`.

**PR:** [#788](https://github.com/unarbos/arbos/pull/788), harness only.

## Cycle 184 — a note under the wrong number

The queue put `call — the microphone path` at cycle 170, fourteen cycles
back.

**It holds** (M-568). Barge-in cuts playback at 287 ms and is done at 332 ms,
both clips transcribe correctly, and the call returns to listening.

**But cycle 170's own fix had drifted** (M-569). That cycle nearly filed a
tenfold regression, discovered that `reply_first_audio` measures two
different things — a greeting the gateway answers itself, and a question the
kernel must think about — and added a note saying so.

One run produces both. This cycle measured 6416 ms after the clip's delegated
question and 1240 ms after the barge. The note explaining the delegated kind
printed at the end of the run, underneath the barge's number. A reader seeing
1240 ms with that note would take a delegated reply to be about a second, and
call the next honest 12-second reading a tenfold regression — which is
exactly the mistake cycle 170 wrote the note to prevent, reintroduced by
where the note sat.

Each number now prints with the words it answered and which side answered
them:

      6416 ms   after "Hello Arbus. What are we working on right now? Give "
                the kernel had to answer
      1240 ms   after "Stop. Wait. One more thing."
                the gateway answered it itself

A lesson written as prose at the bottom of a run is a lesson attached to
whatever happens to be printed above it.

Evidence in `media/mobile/cycle-184/`.

**PR:** [#789](https://github.com/unarbos/arbos/pull/789), harness only.
