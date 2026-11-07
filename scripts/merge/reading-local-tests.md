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

## Derive the value that must follow something else; assert the constraint that guards it

"Derive, don't assert" is right about **values that have to track an external source**. A pinned
version copied into a second file is correct today and silently wrong the day the first one moves,
so read it from the source instead and refuse when you cannot.

Applied to a **guard**, the same instinct destroys it. `workspace-test-deps.sh` keeps the floor
`webrtc-sys` requires as its own constant, deliberately not derived from the release being
installed, and the author's reason is the whole point: *a check against the version you asked for
passes whatever you asked for*. Derive that floor from `LLVM_RELEASE` and `LLVM_RELEASE=18.0.0`
sails through the guard that exists to catch exactly it.

**So the two halves are different jobs.** The pin must agree with CI, so derive it. The floor must
be able to disagree with the pin, so assert it independently. A guard derived from the thing it
guards is not a guard — it is a restatement, and it passes for the same reason its subject does.

That is the same fault as the rest of this document wearing different clothes: a check that cannot
fail for the reason it names. Before deriving anything, ask which of the two it is.

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

## The allow-list is a cache of authorizations, never a source of them

A token entitles a merge only because the coordinator named that pull request in a message. The
allow-list exists so the queue need not be re-derived from the transcript before every call. It
follows that a token written there by anybody else authorises nothing: it is not in the only
place authority lives, and a sibling agent cannot forge a coordinator's message.

That sentence is what makes it safe to keep the live allow-list somewhere durable and shared.
Durability and authority are separate problems; keeping a digest of the file bytes only moves
the second one, because on a fresh VM there is no trustworthy local digest to compare a restore
against. The anchor has to be outside the file either way.

`allow-audit.sh` runs before any pull request is judged and compares the **token set** against a
ratified reference, asymmetrically:

- **a token nobody ratified** halts the entire lane, not just the merge that was asked for — an
  unexplained token is a tampering signal, not a queue entry;
- **a ratified token that has gone missing** is reported but does not block, because an absent
  token is already unauthorized and that direction is fail-safe.

Set and not bytes, deliberately: prose, headers and the hold and freeze blocks are commentary,
not authority. They can be rewritten freely and must not move the gate — which is exactly what
leaves this lane able to write down *why* something is held without needing ratification to do
it. A missing reference set is a **setup gap** (exit 4), never a refusal, for the same reason
everything else in the preflight is.

### A refusal must name the action it refuses

"Refused by name" is not a verdict on a branch; it is a verdict on *something you might do to*
a branch. Written bare, it gets read as "untouchable" — and the safe-looking reading is the
damaging one, because the branches this lane refuses are usually **negative controls**, which
exist to be run.

A negative control is evidence. **A rule that blocks the control blocks the proof.** The lane
refuses `cursor/capture-rig-negative-control-66a0` from ever merging, and it must never acquire
a pull request — but it also has to be *dispatched once*, with `ARBOS_SHOT_DIR` deliberately
withheld, so the skip guard is observed firing instead of assumed to work. It is #2876's
negative control, and #2876 fixes a capture rig that had run four times and captured nothing.
A worker who declines to dispatch it, out of caution, withholds the one thing that can show the
fix works — and blocks the pull request while appearing to protect it.

So every refusal entry states **which actions are refused and which are required**, not just
that something is refused. An unexplained refusal invites two opposite errors and makes them
look equally responsible: somebody lifts it because no reason is given, or somebody
over-applies it because none is given either. Over-application is the quieter of the two, which
makes it the more likely to survive.

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

## Gate 1 reads the step list, not the rollup — so protect the step list

A check's conclusion is a rollup. It says a job succeeded; it does not say which commands ran
inside it. Gate 1 asks the narrower question — *did `cargo test --workspace --locked` actually
execute?* — and the only thing in CI that answers it is the per-step list. A job can be green
because its test step was skipped, conditioned out, or never reached, and the rollup is green
in exactly the same way it would be if the tests had run and passed.

So read the steps. Every time, including when the conclusion is `success`.

### The consequence: never accept a refactor that collapses the steps gate 1 reads

The gate's whole resolution is the step list, so anything that merges steps destroys evidence in
proportion to how much it tidies. This is not hypothetical — it is the first thing that went
wrong when costing AF-1349. Replacing CI's inline toolchain install with a call to the shared
provisioner invites using the script's `--ci` mode, which runs `fmt`, `clippy` and `test`
itself. That turns three named, separately-timed, separately-red steps into one opaque
`workspace-test-deps.sh --ci`, and gate 1 can no longer tell a run where the tests passed from
a run where the step list simply stopped saying.

The fix there was to keep the three steps and have the provisioner write `CC`/`CXX` to
`$GITHUB_ENV` instead — four lines, and the evidence survives. Generalise it: **a convenience
that merges steps eats the proof that the tests ran.** When a refactor would collapse a step a
gate reads, the question is not whether the refactor is tidier; it is what the gate can still
see afterwards. If the answer is "less", the refactor needs a different shape, not a waiver.

## A gate that cannot move is not a gate: vacuity, in gate 3 and gate 4

Two different ways a gate reports PASS while measuring nothing. Both have bitten, both were
caught by asking "could this number have come out differently?" rather than by reading it.

**Gate 3 over an empty comparison.** Gate 3 compares the cumulative diff across a rebase. When
the pinned head already sits on live `main`, there is no rebase and nothing to compare, and the
`cmp` will of course be clean. Do not report that as PASS. Report that **gate 3 did not run,
because there was no rebase to compare across** — and then re-read gate 2 immediately before the
merge call anyway, because if `main` moved in the meantime the rebase is real and gate 3 becomes
live. A gate reporting PASS over an empty comparison is worse than one reporting it did not
apply: the first is a claim, the second is an accurate absence of one.

**Gate 4 over a suite the extractor cannot see.** `0 before, 0 after, 0 dropped` is not a pass.
Neither is any unchanged count, until you have shown the extractor can see the suite at all. On
#2894 the TypeScript enumeration came back 0 → 0 — which looks exactly like "this PR touches no
TypeScript", and was in fact the wrong directory (the tests are under `harness/arbos-pi-ext/`,
not `packages/`). Redone against the right path it was 14 → 14, which is the same verdict backed
by a number that could have moved.

So the non-vacuity check has two halves, and the first is the one that gets skipped: **show a
non-zero population before you report a zero delta.** Say the population out loud in the report
(`Rust 2296 → 2304`, `Swift 621 → 621`, `TypeScript 14 → 14`) so a zero that means "nothing
changed" is distinguishable on the page from a zero that means "nothing was looked at".

A cheap way to close the loop on the Rust half: a static extractor's declared count and the live
run's `passed + ignored` should agree exactly. On #2894 both were 2304, which makes each a check
on the other rather than two readings of the same thing.

### A repetition harness must prove it selected a population before it counts anything

The same defect, in the tool you reach for to *diagnose* a flake. A loop that runs a filtered
test N times and tallies outcomes is counting two different things under one name: how often the
test failed, and how often no test ran at all. Those print as the same number and mean opposite
things.

Measured on 2026-11-07, diagnosing an `arbos-agentd` flake: the harness reported **20 failures
out of 20**, which was not believable because it could not move. `cargo test -p arbos-agentd
<bare_name> -- --exact` needs the full module path, so the filter matched nothing, every run
produced `running 0 tests`, and the loop scored each silent success as a failure. With
`cancel_handles_tests::` prefixed it was **25 passes out of 25** — the opposite verdict, from the
same host, minutes apart.

So, before the loop counts a single outcome:

```sh
cargo test --locked -p <crate> "$T" -- --exact --list   # must say exactly "1 test"
```

and inside the loop, treat "no test ran" as a distinct, loud outcome rather than folding it into
failure. A harness that cannot tell an absent test from a failing one is not measuring the flake;
it is measuring its own filter. This is the vacuity rule above turned on the diagnostic tooling,
and it was committed by the person who had written that rule an hour earlier — which is the
reason it is worth its own heading.

## Reading a Release run: no line in the log is load-bearing

Three outcomes have to be distinguished after a merge, and the log distinguishes none of them.

1. **Not yet attempted.** The Release run fired by the *merge push* does not release. It logs
   `no release: push <sha> to main is not a hotfix; it ships through the CI-green path` and
   concludes **success**. The bump comes from the later `workflow_run` Release run, which fires
   only once CI on `main` is green. So "the Release workflow ran and succeeded after my merge" is
   not evidence that anything was published, and a missing bump shortly after a merge is not
   evidence that the release failed.
2. **Tag-only.** `auto-stable.sh` pushes the tag unconditionally, then attempts an unforced
   push to `main` inside an `if`. If `main` moved during the debounce the bump stays under its
   tag, on no branch — and the failure branch's log line *also* leads with the word *released*.
3. **Fast-forwarded**, the one you are waiting for.

Only ancestry separates all three:

```sh
git fetch -q origin main && git merge-base --is-ancestor "$BUMP_SHA" origin/main
```

## When the event you await can destroy the subscription that would report it

A CI subscription keyed to a commit drops its result if that commit is no longer the head of
`main` when delivery is attempted. The release bump **is** a push to `main` — so the very event
being waited for is capable of silently swallowing the wake-up about it. The same applies to a
subscription on a PR head any time `main` can move underneath it.

The general rule: **when the thing you are waiting for can invalidate the subscription that would
tell you about it, a subscription is not a wait.** Arm a timer backstop, and have the backstop
**re-derive the fact** — here, ancestry of the bump — rather than re-ask for the event. Re-asking
for the event inherits the same failure mode, and two measurements with the same failure mode are
one measurement.

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

### Answer a path-filter question by reading `paths:`, never by grepping the file

Asking whether a workflow is triggered by a crate is a question about its `paths:` block, and a
grep over the file will cheerfully answer it wrong. `grep arbos-mac .github/workflows/ios.yml`
returns three hits, which reads as confirmation that the filter covers that crate. All three are
the self-hosted runner label — `runs-on: ${{ inputs.runner || 'arbos-mac' }}` and two mentions of
it in prose. The `paths:` list does not contain `crates/arbos-mac/**` at all, and the practical
consequence is large: a Mac-only change triggers no TestFlight build.

A false confirmation is worse than no answer, because nobody re-checks a question they believe is
settled. Read the block.

## "Did the release land?" is a question about ancestry

When the lane is frozen waiting for a release to reach `main`, test it with ancestry and nothing
else:

```sh
git fetch -q origin main && git merge-base --is-ancestor "$BUMP_SHA" origin/main
```

A commit titled `release: X.Y.Z` is not evidence, and neither is the tag. `auto-stable.sh` pushes
the tag unconditionally on its own line **before** attempting the `main` push, and that second
push is unforced inside an `if` — so when `main` moves during the debounce the bump ends up
reachable from its tag alone, on no branch. Both outcomes produce the tag and a commit with that
title, and the log line leads with the word *released* either way.

The consequence to recognise on sight: the next release takes its number from the **tags**, so a
tag-only bump spends that number. The following successful release comes out one patch higher and
the spent version is published but on no branch. And because `ios.yml` triggers on a push to
`main`, a tag-only bump produces no TestFlight build — the tag cannot substitute for one.

## A push during an iOS or Mac leg is a seventy-minute decision

The branch's concurrency group cancels the in-flight run when a new commit lands on it. Before an
iOS or Mac leg starts that costs nothing and two changes batch for free. Once `fastlane test` is
running, a one-line push throws away the queue wait **and** the run — about seventy minutes,
measured on 2026-11-06 when a worker pushed a small commit roughly forty minutes into
`build-xcframework + fastlane test` and the group cancelled the leg.

So the rule is about **when**, not whether: while a verdict is owed or a leg is in flight, the
push waits; when several changes are coming, land them before the leg rather than during it.

Nothing is lost in the cases where the next run is a superset and the PR is held anyway — but
that is luck about which change it was, not a property of the decision. Treat the cancellation as
the cost and batch around it.

## Gate 5 on a Mac-touching PR: look for the compiler before you fetch one

`cargo test --workspace --no-run` fails on this repo under the clang most Linux images default to.
The message is a version guard in `webrtc-sys`' own `build.rs`, not a compile error, and
`arbos-mac` reaches `webrtc-sys` through `libwebrtc`, so there is no crate to scope around it.

**Run `scripts/workspace-test-deps.sh` — the repository already has an installer, and you should
not write a second one.** It landed in #2878 and installs the GPUI apt packages and the pinned
clang together, skipping the download when a usable compiler is already unpacked. A lane worker
missed it on 2026-11-06, wrote a second installer, and spent 25 minutes downloading a 2 GB asset
onto a host that already had `/usr/lib/llvm-21/bin/clang++` — present and working, merely not the
default. Two installers of one toolchain is the same trap as two copies of one pin.

**And look before you fetch, which is also how that was diagnosed.** The tell was not the file
list: the directory held exactly the extract list, which looks identical whether this session
created it or an earlier one did. `stat` settled it — the directory's birth time was four hours
older than the session that "installed" it, and `lib/clang` kept its original mtime while all 379
files below it moved. The obvious check was not a check, and a second route was needed.

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
