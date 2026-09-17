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
