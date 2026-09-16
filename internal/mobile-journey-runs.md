---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# The phone journey — one run, start to finish, scored per step

Jacob (2026-09-16): "The upgrade loops need to actually run full cycles of creating a project, running a challenge, doing follow up etc. Right now these things are flaky at best." From cycle 14 every cycle runs this journey once at least; per-screen checks live inside it. Scores accumulate here so flakiness reads as a rate; a step that fails twice running becomes a named bug (bottom).

**Definition.** QA owns `docs/acceptance-journeys.md`; that file does not exist in the store at 13:40 UTC (not in `docs/`, not on the `store-docs` mirror branch). The steps below follow the coordinator's brief word for word and will be renumbered to QA's the moment its file appears, so desktop, phone and QA measure one journey.

**Runner.** `~/mac-journey.sh <project>` on the loop's Mac (iPhone 15 Pro simulator, iOS 27.0, ArbosLife hub, phone token). Each run writes `~/mobile-out/journey/<run>/` with a still per step, the app console, and `transcript-tail.txt` — the project's root transcript read back through the hub by `~/hub-history.py`, which is how PASS/FAIL is decided: the step passes when the kernel's own record shows the expected line after the challenge's sequence number, not when the screen looks right. Steps marked EYE are scored from the still. Copies of the run folders: `media/mobile/journey/<run>/`.

## Steps

| step | what the phone does | pass when |
| --- | --- | --- |
| J1 | cold open | the list shows the project within 7 s |
| J2 | pick the project | header names it, history at the tail (EYE) |
| J3 | a real challenge: two workers, one that can fail (`fizzbuzz_<id>.py` + pytest; `haiku_<id>.txt`) | the kernel echoes the line ≤ 30 s |
| J4 | workers appear | two `spawn` records after the challenge ≤ 60 s; pill / Working lines on screen |
| J5 | follow up mid-flight | echo ≤ 30 s |
| J6 | background 40 s while it works, come back | chat intact; away card / notification if the kernel notified (EYE) |
| J7 | workers finish, the root reports | an assistant line naming both results ≤ 240 s |
| J8 | follow up afterwards | echo ≤ 30 s |
| J11 | the work really happened | the reply prints the haiku and the pytest "passed" line ≤ 120 s |
| J9 | dictate a follow-up (injected clip; second tap sends) | the spoken words arrive as a user line ≤ 30 s |
| J10 | attach a photo, ask what is in it | the reply describes it; "didn't arrive / nothing at that path" is FAIL |
| J12 | "Call demo", ask the project a question (injected clip) | the spoken turn lands in **this project's** transcript ≤ 40 s |
| J13 | kill the app, reopen, open the project | the chat ends where it ended; no pending cards (EYE) |

## Runs

| run (UTC) | build | J1 | J2 | J3 | J4 | J5 | J6 | J7 | J8 | J11 | J9 | J10 | J12 | J13 | notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 09-16 13:16 | #309 branch | P | P | P | P | P | P (intact; no away card) | **F** | P | P | P | **F** | **F** (call never started: the screen waits for a tap) | P | 8 m 38 s; J10 auto-check false-positive, corrected by eye |
| 09-16 13:27 | #309 branch | P | P | P | P (45 s) | P | P | **F** | P | P | P | **F** | **F** (call ran; the turn landed in the voice server's own kernel, not `demo`) | P | 9 m 25 s; Jacob typed "Sup" into `demo` mid-run |
| 09-16 13:56 | #319 branch | P | P | P | P (35 s) | P | P | **F** | P | P | H (runner still expected auto-send; the words waited in the field as designed — runner now taps send) | **F** (model searched for the file: not there) | **F** (turn landed in the gateway's kernel) | P | 9 m 48 s |

Rates after 3 runs: J1–J6, J8, J11, J13 **3/3**; J9 **2/2** product (one harness lag); J7 **0/3**; J10 **0/3**; J12 **0/3**. H = harness, not counted against the product.

## Named bugs (failed twice running)

| id | step | what | whose |
| --- | --- | --- | --- |
| **JB-1** | J7 | the root never delivers the report it was asked for ("one line per worker") once a follow-up arrives mid-flight; it answers the follow-up and the promise is dropped. Both runs. The workers did finish (J11 proves the files) — it is the report that goes missing | kernel / root agent (steer handling) |
| **JB-2** | J10 | a photo sent to `arboslife/demo` never lands: the kernel answers "nothing at that path", no `written` error, no "unknown frame" — so `put` is accepted and the bytes are lost on the way. The same photo landed on the Mac's own kernel through a rebuilt hub in cycle 6 (M-56 shape: a hub relaying `wire::Frame` drops `data`) | ArbosLife hub / kernel deployment — mesh worker; ask filed `features-inbox/2026-09-16-mobile-journey-photo-and-call-asks.md` |
| **JB-3** | J12 | "Call demo" does not talk to `demo`: the voice gateway attaches to one fixed kernel (the phone kernel), so a question asked on the call from any project's chat lands in that kernel's transcript. The call is not project-scoped | voice gateway (`session.start` needs the target) + app (pass it); same ask file |

Harness notes: run 1's call did not start because the call screen waits for a tap on the disc; the runner taps it now. The photo check now reads the reply after the photo line's sequence number and fails on "didn't arrive / nothing at that path".
