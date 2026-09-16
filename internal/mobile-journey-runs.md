---
cursor:
  subagentId: "bc-08d8261b-fea2-5075-9949-d45f6f9d4acc"
---

# The phone journey — one run, start to finish, scored per step

Jacob (2026-09-16): "The upgrade loops need to actually run full cycles of creating a project, running a challenge, doing follow up etc. Right now these things are flaky at best." From cycle 14 every cycle runs this journey once at least; per-screen checks live inside it. Scores accumulate here so flakiness reads as a rate; a step that fails twice running becomes a named bug (bottom).

**Definition.** QA's `docs/acceptance-journeys.md` (13:52 UTC) is the spine: steps **J1–J8** below carry QA's ids and assertions in the phone's version (J1 = open the project in the list and have the kernel seed the failing project; J6 = background and return; J8c = the link cut, which the phone owns). Runs 1–3 used the earlier 13-step numbering (table kept below for the record). Phone-only steps Jacob asked for — dictate, photo, call — are **P1–P3**. History lines in QA's shape: `internal/mobile-journey-history.jsonl`.

**Runner.** `~/mac-journey.sh <target>` (`pod` direct, or `<machine>/<project>` through the hub) on the loop's Mac (iPhone 15 Pro simulator, iOS 27.0, ArbosLife hub, phone token). Each run writes `~/mobile-out/journey/<run>/` with a still per step, the app console, and `transcript-tail.txt` — the project's root transcript read back through the hub by `~/kernel.py`, which is how PASS/FAIL is decided: the step passes when the kernel's own record shows the expected line after the challenge's sequence number, or when the kernel's `read` frame returns the file — never when the screen merely looks right. `U` = unverified, said so, per QA rule 2. Steps marked EYE are scored from the still. Copies of the run folders: `media/mobile/journey/<run>/`.

## Steps (QA ids, phone version)

| step | the phone | pass when |
| --- | --- | --- |
| J1 | cold open, open the project from the list; ask the kernel (no workers) to seed `journey_<id>/` — `mathlib.area` wrong, a failing unittest, one commit on main, no CHANGELOG | the row is listed; the `read` frame returns `journey_<id>/tests/test_math.py` |
| J2 | QA's challenge prompt scoped to the folder | busy on screen ≤ 20 s; a `spawn` ≤ 120 s — `U` when the root does it itself (QA rule) |
| J3 | watch honestly | the challenge turn ends (`turn_complete`); no two identical assistant lines in a row; no `status "…"` written as prose (#320) |
| J4 | mid-flight: "Also add a line to the CHANGELOG saying who asked for this: QA-<id>."; after: "Summarise what you changed in two lines." | `read CHANGELOG.md` carries `QA-<id>`; no second worker spawned after the line; the summary gets a reply |
| J5 | steer ("British spelling"), read-only ask ("What time is it, roughly?") while it works; Stop | the ask is answered; **Stop: the phone has no Stop control → `U` until it has one** |
| J6 | background 40 s while it works, return; kill and reopen at the end (J6k) | chat intact; badge / away card when the kernel sends `notify` (`U` on kernels that send none) |
| J7 | the result on disk through the `read` frame + one verification command | `mathlib.py` has `w * h`; `CHANGELOG.md` mentions the fix; unittest `OK`; a branch other than main with commits |
| J8 | a: kernel restart mid-turn — `U` (hosted kernel, the phone cannot kill it); b: second project — a tagged line in another project (P-runs); c: **link cut 25 s mid-turn** — the phone's own | c: calm "not answering — waiting" line during the cut; the turn finishes after the link returns |
| P1 | dictate a follow-up (clip); his tap sends | the words arrive as a user line |
| P2 | attach a photo, ask what is in it | the reply describes it; "didn't arrive / nothing at that path" fails |
| P3 | "Call <project>", ask a question (clip) | the spoken turn lands in **this** project's transcript |

## Runs (QA ids)

| run (UTC) | target | build | J1 | J2 | J3 | J4 | J5 | J6 | J7 | J8 | P1 | P2 | P3 | notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 09-16 14:32 | `pod` (direct) | #319 branch | **F** (kernel said "seeded"; the read frame found no `tests/test_math.py`) | U (no worker) | P | **F** (no CHANGELOG at all) | **F** (the time ask never answered) | U (chat intact; this kernel sent no `notify` during the run) | **F** (no OK, no branch) | U/U/**c: P** (calm line under the steer during the cut; turn went on after) | P | P | P (on `pod` — the gateway's own kernel, so JB-3 does not bite here) | 6 m 40 s. The pod root (gemini-2.5-flash) floundered for the whole challenge: 13 `compacted` / `over budget (~19k tokens)` notices, `write` "exit 1" failures, five `rm -rf` + `mkdir` loops, two false "seeded", never a worker. See **candidate JB-4** |

Rates on QA ids after 1 run: J3 1/1, P1–P3 1/1; J1, J4, J5, J7 0/1; J2, J6, J8 unverified.

## Runs 1–3 (earlier 13-step numbering, `demo` on ArbosLife)



| run (UTC) | build | J1 | J2 | J3 | J4 | J5 | J6 | J7 | J8 | J11 | J9 | J10 | J12 | J13 | notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 09-16 13:16 | #309 branch | P | P | P | P | P | P (intact; no away card) | **F** | P | P | P | **F** | **F** (call never started: the screen waits for a tap) | P | 8 m 38 s; J10 auto-check false-positive, corrected by eye |
| 09-16 13:27 | #309 branch | P | P | P | P (45 s) | P | P | **F** | P | P | P | **F** | **F** (call ran; the turn landed in the voice server's own kernel, not `demo`) | P | 9 m 25 s; Jacob typed "Sup" into `demo` mid-run |
| 09-16 13:56 | #319 branch | P | P | P | P (35 s) | P | P | **F** | P | P | H (runner still expected auto-send; the words waited in the field as designed — runner now taps send) | **F** (model searched for the file: not there) | **F** (turn landed in the gateway's kernel) | P | 9 m 48 s |

Rates after 3 runs: J1–J6, J8, J11, J13 **3/3**; J9 **2/2** product (one harness lag); J7 **0/3**; J10 **0/3**; J12 **0/3**. H = harness, not counted against the product.

## Candidate (one run; named if it repeats)

| id | step | what | whose |
| --- | --- | --- | --- |
| **JB-4?** | J1/J4/J7 on `pod` | the pod's root agent cannot carry a four-step task: its context budget reads as ~19–21k tokens ("over budget … nothing old enough to compact" 13 times in one run), the `write` tool returns exit 1 for it, and it loops (`rm -rf`, `mkdir`, "seeded" twice) without ever spawning a worker or producing the CHANGELOG. `demo` on ArbosLife (opus, workers) did the same shape of task in every earlier run | pod kernel config (root budget / model) + `write` tool on that place |

## Named bugs (failed twice running)

| id | step | what | whose |
| --- | --- | --- | --- |
| **JB-1** | J7 | the root never delivers the report it was asked for ("one line per worker") once a follow-up arrives mid-flight; it answers the follow-up and the promise is dropped. Both runs. The workers did finish (J11 proves the files) — it is the report that goes missing | kernel / root agent (steer handling) |
| **JB-2** | J10 | a photo sent to `arboslife/demo` never lands: the kernel answers "nothing at that path", no `written` error, no "unknown frame" — so `put` is accepted and the bytes are lost on the way. The same photo landed on the Mac's own kernel through a rebuilt hub in cycle 6 (M-56 shape: a hub relaying `wire::Frame` drops `data`) | ArbosLife hub / kernel deployment — mesh worker; ask filed `features-inbox/2026-09-16-mobile-journey-photo-and-call-asks.md` |
| **JB-3** | J12 | "Call demo" does not talk to `demo`: the voice gateway attaches to one fixed kernel (the phone kernel), so a question asked on the call from any project's chat lands in that kernel's transcript. The call is not project-scoped | voice gateway (`session.start` needs the target) + app (pass it); same ask file |

Harness notes: run 1's call did not start because the call screen waits for a tap on the disc; the runner taps it now. The photo check now reads the reply after the photo line's sequence number and fails on "didn't arrive / nothing at that path".
