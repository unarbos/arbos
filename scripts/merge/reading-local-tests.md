# Reading a run before a gate

A gate is only as good as the run behind it. These are the ways a run has lied to this lane,
written down so the next reader does not rediscover them one merge at a time.

They share a shape: a correctness that holds by accident, or depends on something nobody would
guess was load-bearing. None of them has a failing test to announce it when it goes.

## The principle underneath all of them

**Two measurements with the same failure mode are one measurement.**

Internal agreement between two methods is not confirmation until you have asked what both could
miss *together*. The worked case: a unique-name count and a verdict count of the same Swift log
agreed exactly, and were offered as corroborating each other. They agreed because a truncated line
is invisible to both — the agreement was the symptom, not the evidence. Two careful measurements,
corroborating, both short by the same one, with no signal anywhere.

Ask what the two methods share before you treat them as independent. If they read the same stream,
parse the same shape, or trust the same absence, they are one measurement with two names.

## The rule: a test that depends on a path's class must assert that class

`arbos-core/src/approval.rs` sorts every path into a **class** — `Scratch`, `Workspace`,
`Anywhere` — and much of the approval behaviour is a function of that class and nothing else.
A test that needs a path to be one class and never says so is not testing what it reads as
testing. It is testing the class its checkout happens to produce.

The rule is about the class, not about any one directory. Keying it on `/tmp` would cover one
instance and leave the identical hole one class over, where a test meaning `Anywhere` quietly
gets `Workspace`.

### The worked example, because this one actually bit us

On 2026-11-06 a full `-p arbos-agentd` run from a worktree at `/tmp/wt2888` reported
`approval::job_approval_tests::a_job_deleting_a_file_it_created_itself_raises_no_card` as failed:
`left: "allowed"`, `right: "pending"`. The pull request under the gate had not touched
`approval.rs`.

The test uses `concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")` as its stand-in for "a file the
job found rather than made", and `CARGO_MANIFEST_DIR` is baked in at compile time from wherever
the checkout sits. `SCRATCH_ROOTS` is `["/tmp", "/work/tmp"]` (`approval.rs:311`), and a delete
raises no card when every target is in scratch. From a `/tmp` checkout the stand-in **is** scratch,
so "allowed" was the correct answer to a question the test did not mean to ask. Re-run from
`/home/ubuntu/wt2888`, the same commit passed 526/0/2.

### Which direction bites

| Polarity | What happens from the wrong checkout | How you find out |
| --- | --- | --- |
| Asserts `"pending"` | Fails | Immediately, loudly |
| Asserts `"allowed"` | **Passes, asserting nothing** | Never |

The loud one is the lucky one. The quiet one is the reason this is a rule and not a note: a case
reading "a delete outside scratch raises a card" passes from a `/tmp` checkout without raising
anything, and no suite anywhere goes red to say so.

Audited on 2026-11-06: `-p arbos-agentd` has exactly one env-derived absolute path in its approval
tests, `approval.rs:1901`, and it asserts `"pending"` — the loud polarity. So the quiet direction
is not currently present in those counts.

## The safety below it is incidental, and a reasonable refactor removes it

Relative paths in those tests are location-independent today, but not by design:

- `classify_path` returns `Anywhere` for anything not starting with `/`
  (`arbos-core/src/approval.rs:435`).
- `delete_targets_with` seeds its working directory from a constant, not from the process:
  `let mut cwd = WORKSPACE_ROOT.to_owned();` (`:717`). So `rm -rf dist` resolves to `/work/dist`
  wherever the binary runs.

A hardcoded cwd is plainly wrong in a walker that reads `cd`, so giving it a real one is the
natural fix for some unrelated bug. The day that lands, **every relative-path approval test becomes
location-dependent in a single commit**, the quiet direction goes live across the whole file at
once, and nothing fails to announce it.

That is why this is written down rather than left to the suite. A safety a sensible refactor
silently removes is worth more as a warning than as a passing test.

## Operationally, for this lane

Run agentd suites from a checkout outside `/tmp`. That is a consequence of the rule, not the rule.
Record the worktree path beside any local test count reported as gate evidence, so a number can be
re-read later against the class it was measured in.

## Swift logs: the order on the page is not the order in time

**Judge sequence by timestamps, never by position.** `xcodebuild` writes its own stdout into the
same stream as the test reporter, and the two are not synchronised. So a line that reads like an
ending can sit in the middle of the thing it appears to end.

Measured: `** TEST SUCCEEDED **` lands *inside* `SearchSettingsTests`' run of verdicts, and in
#2889's log the last unit verdict sits **148 lines after** it. In #2848's log an
`iOSSimulator: … Failed to launch app` is stamped 20:23:25 — 54 seconds *after* the session
completed at 20:22:31 — so it is teardown noise, though it is printed where a reader scanning
downward will meet it as if it preceded the end.

**This is the one that manufactures a false positive rather than merely hiding a count.** The
other warnings here tell you a number may be short. This one invites the conclusion "the suite
stopped early", which reads as a serious defect, is specific enough to sound researched, and
would be acted on. It was nearly concluded from #2889's log for exactly this reason: verdicts
appearing after `TEST SUCCEEDED` look like a run that was cut off mid-suite. They are not; they
are a run whose output was reordered.

**It is also the mechanism behind the truncation described in the next section.** Interleaving is
one fact with two consequences: a status write that lands between two reporter lines reorders the
log, and a status write that lands *within* a reporter line bisects it. Learn them together —
a reader who knows only that counts can be short will still believe the order, and a reader who
knows only that the order lies will still trust a count.

## Gate 4 on a Swift leg: a stable absence can be a counting artefact

Gate 4 compares test **names** across a rebase and adjudicates anything missing as a dropped test.
On a Swift leg a name can go missing for a reason that has nothing to do with the diff.

xcodebuild interleaves its own status writes into the result stream and bisects result lines. In
the clone phase each case emits exactly **one** line — its verdict, with no separate `started`
line — so a bisected line removes the test from the log entirely. It is invisible to a verdict
count **and** to a unique-name count, which is why two independent counts can agree with each
other and both be short by the same one.

**The victim is not arbitrary, and the invariant is the test — not where it sits.** Across three
logs the cut line was always
`SearchSettingsTests.testEveryListedSettings…`, at lines 12934, 12953 and 12865, with both the
truncation point and the interleaved junk varying. Line numbers differing by up to 88 rule out
run position. Two earlier readings of this — "a different victim each time", then "it correlates
with position in the run" — were both wrong, and are recorded here so neither is reinvented.

That is what makes it dangerous here rather than merely annoying:

- An **arbitrary** miss differs between base and head, so it shows up as noise and invites a
  second look.
- A **repeated** miss is absent from base *and* head. It does not look like noise. It looks like a
  stable dropped test — the exact signature the dropped-test rule exists to catch — and re-running
  reproduces it, so scrutiny confirms rather than dispels it.

The standing rule then asks the author to prove a behaviour is still asserted somewhere, for a
test nobody removed. That is a false accusation that survives review.

**So before adjudicating any dropped name on a Swift leg, check for a truncated line carrying it.**
`swiftcount.sh counts` refuses to print a count while any line is unreadable, and
`swiftcount.sh recover` names the test behind the fragment. Use them. This paragraph is here for
the case the tooling does not cover: the adjudicator greping by hand under time pressure, who is
who it will usually be.

A one-name discrepancy on a Swift leg is more likely to be this than a genuine drop.

**Where to look first, as a hint and not as the mechanism.** Check
`SearchSettingsTests.testEveryListedSettings…` by name. Key the hint on the **test identity**,
never on a line offset: the offsets moved by 88 lines across three logs while the test did not.
It is **not** a substitute for the general refusal — a named site goes stale the moment the suite
changes, while refusing any unreadable line holds for the next victim, whoever it is.

The overwriting text varies and carries no diagnosis: the end-of-testing status write in one log,
an `iOSSimulator: … Failed to launch app with identifier` in another. Do not read the junk as the
cause of anything; it is whatever happened to be writing at that moment.

### The worked arithmetic, from run 42277644010

| | Figure |
| --- | --- |
| Unit verdicts, clean lines | 550 |
| Unit verdict, truncated | 1 |
| UI passes | 55 |
| **Executed and passed** | **606** |
| UI skips | 13 |
| **Declared in the bundle** | **619** |

619 = 606 + 13. Counting only clean lines gives 550 unit and so 605 passing and 618 declared —
both one short, in the same term, with nothing in the log saying so.

**Counting the declared side is its own hazard, and it was got wrong in both directions.** First
an anchored `^\s*func test` missed `@MainActor func testCopyOfAMultiBlockBubbleIsTheWholeMessage()`,
undercounting. Then the unanchored fix over-caught `AddMembersPlanTests.testingGroup`, a private
helper returning a `PeopleChat` that XCTest never discovers as a test, overcounting by one back to
the same wrong total. A declared count is a *guess at what the runner will discover*, made by a
regex that does not implement XCTest's discovery rules — `@MainActor` and other attributes sit
before `func`, and `testingGroup` matches `test…` while being a helper.

Both errors were found by diffing declared names against observed verdicts, not by rereading the
pattern. That is the general move: a count compared against a count hides a mistake that an
itemised difference exposes by name.

Checked against the source on 2026-11-06: `ArbosTests` declares **551** test cases (552 matches
for `func test…(` less `AddMembersPlanTests.testingGroup`, a private helper returning a value that
XCTest does not discover), and **551 executed**.

### Does a green Swift leg ever cover tests that did not run? Measured: no, in three logs

The worry is a case that never ran and so has no failing verdict to find, which a name
enumeration cannot see because the declared list is unaffected. Checked by diffing
suite-qualified declared names against observed verdicts:

| Log | Clones / PIDs | `Failed to launch app` | Declared vs observed |
| --- | --- | --- | --- |
| #2848 green | one, `(18475)` | 1, during teardown | 551 / 551 |
| #2889 green | one, `(27228)` | 1, during teardown | 551 / 551 |
| #2887 red | **two, `(23786)` then `(26149)`** | **0** | 551 / 551 |

In every case the only declared-but-unobserved name was the truncated one, whose test did run.

Two things follow. **A launch failure is not the mechanism to key on** — the red run has none and
the green runs have one each, so it does not separate them, and in the green runs it lands in
teardown after the session completed. **The mechanism worth checking is a crashed test host**: in
the red log the clone PID changes mid-suite, which is a process dying and the harness respawning.
On that one sample the respawn picked up every remaining case — nothing vanished with the dead
process — but it is one sample, and the check that would catch the bad version is **per-clone
verdict counts against declared suite membership**, not a grep for launch failures.

Note while reading these logs that **textual order is not chronological**: xcodebuild's own stdout
is interleaved with the test reporter's, so `** TEST SUCCEEDED **` appears in the middle of a
suite's verdicts and the last unit verdict sits ~150 lines after it. Judge sequence by timestamps,
never by position.

## What no gate reads: a new value in a shared document

All five gates read **one repository at one commit**. A change can therefore be green on every one
of them and still break a different component, because no gate ever looks at two readers of the
same data at once.

The live case is #2890, held before merge on 2026-11-06. It is the first producer of a new
approval state, `cancelled`, written into the document the Mac client reads. The client's enum has
no catch-all, and the failure is not a missing card: `read_message` ends in `from_value(json).ok()?`,
so an unparseable state discards **the whole message row, assistant text included** — permanently,
since the state is terminal. The same PR deletes the expiry sweep that wrote the one automatic
close the client does understand, which turns this from a rare close into the only unanswered one,
and the trigger is a user stopping a job with a card open.

**Every gate passes it.** Gate 4 sees no dropped test. Gate 5 compiles — the producer's repository
is internally consistent. The new state's own test asserts exactly the right thing. Nothing is
wrong *within* the commit under test.

**So ask it of the diff, because CI cannot answer it.** When a PR writes a new value into a shared
document, the question is whether **every** reader already tolerates an unknown value. A reader
that models a wire string as a closed enum does not, and deserialising into one is the tell. The
follow-up that matters is what the reader does on failure: dropping one field is a degraded card,
while dropping the enclosing record is data loss, and the two look identical in the producer's
diff.

## Gate 5 on a Mac-touching PR: look for the compiler before you fetch one

`cargo test --workspace --no-run` fails on this repo under the clang most Linux images default to.
The message is a version guard in `webrtc-sys`' own `build.rs`, not a compile error, and
`arbos-mac` reaches `webrtc-sys` through `libwebrtc`, so there is no crate to scope around it.

**Run `clang21.sh check` first.** On 2026-11-06 a worker met the wall, downloaded the 2 GB asset,
and only afterwards found that `/usr/lib/llvm-21/bin/clang++` had been on the host for hours —
present and working, merely not the default. The directory's birth time was four hours older than
the session that "installed" it. `check` searches `/usr/lib/llvm-*` and the default compiler and
names what it finds; `install` fetches only when nothing clears the floor.

**Clearing the floor and matching CI are different claims, and only the weaker one is usually
true.** The guard asks for "clang 21 or later", so any 21+ compiles the rebased head and gate 5 is
satisfied — gate 5 asks whether the code compiles. It does **not** follow that the compile used
CI's toolchain: that needs the pinned release, and `check` says so when the compiler it found is
not the pin. Report the compile as having cleared the floor, not as having used CI's build.

## `image.yml`: the step that always fails is load-bearing

`Register the Daytona snapshot` in `image.yml` has failed on **every** image run since 0.1.183 —
30 of 30 at one count — with `image/register-snapshot.sh: line 25: DAYTONA_API_KEY: DAYTONA_API_KEY
is required`. **That is expected and does not block a roll.** CI does not register the snapshot at
all: the deploy worker registers it host-side on the relay, running the same script with the key
read from `/etc/arbos/relay.env`. So the key is correctly absent from repo secrets, and an
`image-v0.1.*` tag builds, pushes to GHCR, passes its local smoke, fails that one step, and still
produces a registered snapshot afterwards from the host.

**Do not approve a PR that deletes the step without changing the deploy worker in the same PR.**
A step that fails every single time looks like dead weight, and removing it is the obvious tidy-up.
But the installed deploy worker's fallback — used when the jobs API returns no steps — greps the
job log for the literal `Run image/register-snapshot.sh` and reads the build as **passed only when
it finds it**. Delete the step and the worker reads every good build as failed.

So the failing step is doing two jobs: the one it appears to do, which it never completes, and
carrying a string another system keys on. Only the second one matters.

## See also

- `swiftcount.sh` — the same family on the Swift side: a count that reads complete while missing
  an arbitrary one of its results.
