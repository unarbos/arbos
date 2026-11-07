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

### The companion principle, and the one that says what to do about it

**The quantity that settles a question must come from outside the thing being questioned.**

The rule above tells you when two readings are secretly one. This one tells you where to get a
reading that is genuinely separate: *not from the subject*. It has turned up on three unrelated
surfaces in a single night, which is the only reason it is stated here as a general form rather
than three times as three discoveries. The instances are worth knowing by name, because the next
one will not look like any of them either:

1. **Gate 4's denominator.** Counting a Swift log's results against a total the same log emits is
   circular — both move together when the log is wrong. The remedy was an **external** denominator
   with neither side treated as one. See *Only the UI bundle emits a rollup*.
2. **The agentd premise check.** Establishing "this test fails for reason R" by running that test
   through the code under repair cannot distinguish a repair from a flake from a false premise.
   The remedy was artefacts of the **original** run. See *A premise check must not run through the
   code the test is about*.
3. **A count that disagrees with its own enumeration.** A document said "nine" over a list of ten.
   Recounting either list settles nothing, because the dispute is *about* those lists. It was
   settled by a third total neither list contains — ten free plus nine held equals nineteen, the
   ratified token count — which is outside both and agrees with one.

**The operational form is a question, not a procedure: *what would I accept as an answer that this
thing cannot influence?*** If there is no such answer, say so rather than producing a number.

And note the asymmetry in case 3, because it is the part that is easy to get wrong. When no
external total exists, the rule does not license picking whichever figure you like. **The
enumeration wins over the summary word** — a list is the primary record and a count of it is a
derived claim — so the correction is to rewrite the word. If a count has no population behind it
at all, it cannot be adjudicated in either direction and must simply be **withdrawn**. That is
what happened to a companion claim of "four stacks" in the same document: four was the count of a
*different* population (PRs not based on `main`), and labelling one population's cardinality with
another population's name leaves nothing to check it against.

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

## A method that reads outcomes can only count the lines that survived

This is a failure mode in its own right, separate from truncation. Truncation is the *cause* in
the Swift case; this is the *class*, and it applies to any log-derived count.

Counting outcomes — verdict lines, result records, anything the run emitted — measures **what the
log retained**, which is not the same as what ran. When a line is lost, the count is short and
**nothing in it says so**. The usual integrity checks cannot find the hole:

- **Internal consistency** passes, because the log really is self-consistent. Fewer lines, all
  agreeing.
- **A duplicate check** passes, because a missing line creates no duplicate.
- **Repeating the run** reproduces it when the loss is systematic rather than random.
- **A second person counting the same way** gets the same number, which feels like corroboration
  and is not. *Two measurements with the same failure mode are one measurement.*

So: **reconcile an outcome-reading count against a declaration-reading count.** The two fail in
opposite directions — outcome counting is blind to a line that was destroyed; declaration
counting only ever reports what a regex believes the runner will discover, and has been wrong
both high and low. Agreement between them is informative precisely because neither alone is.

Worked instance: on #2881 and #2883 an outcome count gave 617 and a declaration census gave 618.
The method that produced 617 was **sound, carefully checked, and still silently one short** — one
verdict line had been bisected. The arithmetic is in the gate 4 section below. The diagnosis to
carry is the classification: a one-test gap between two counting methods is as likely to be a
destroyed line as a dropped test, and the two are indistinguishable until the methods agree.

## Only the UI bundle emits a rollup, so gate 4's unit arithmetic is circular

`Test Suite 'ArbosUITests.xctest' … Executed 68 tests, with 13 skipped` is in the log. **`Test
Suite 'ArbosTests.xctest'` appears zero times** — the unit bundle runs under the parallel-clone
format and prints no total. Measured on run 42286751308: 0 occurrences against the UI bundle's 2.

So for the bundle holding ~550 of the 618 cases, **there has never been a declared figure in the
log at all.** Both sides of the unit arithmetic come from counting verdict lines in one stream,
and **a gate cannot detect a loss in the stream it derives both sides from.** That is why the
bisected line needed a cross-method reconciliation to find instead of falling out of the gate.
A count of survivors is not a count of what should exist.

### The remedy: an external denominator, with neither side treated as one

This is instance 1 of *the quantity that settles a question must come from outside the thing
being questioned* — see the companion principle at the top of this document.

`internal/2881-declared-swift-tests.py` in the project store parses `Class.testName` from the
test sources and diffs that set against a log. It needs a test directory and a log, no repo
state:

```
python3 2881-declared-swift-tests.py <ArbosTests dir> <ArbosUITests dir> --log <xcodebuild-test.log>
```

**Adopt it under this constraint, which matters more than the tool: neither side is a
denominator.** The evidence is the *agreement* of the two, with **every disagreement classified
into a closed set of causes**. The blindnesses are inverse — a log cannot see a destroyed line,
and the source cannot see what the compiler and the bundle actually included — so a disagreement
is discharged only once it is both named **and** assigned a cause. **An unclassified
disagreement is exactly where a real drop hides.**

In particular, "declared but not reported" has **two opposite causes that must never collapse
into one bucket**:

| | Cause | Verdict |
| --- | --- | --- |
| `LOST VERDICT` | the verdict line was destroyed mid-write | benign |
| `NEVER BUNDLED` | the test never reached the bundle | the inert-test defect |

Validated at `32fb53d11`, where it named both holes and nothing else:

```
declared 619   never bundled 1   runnable 618   observed 617
  NEVER BUNDLED   TranscriptSpacingUITests.testTheAvatarLaneHoldsItsColumnWhateverTheKeyboardDoes
  LOST VERDICT    SearchSettingsTests.testExactKeywordPutsSettingsFirstAndNamesTheKeyword
```

### Known limits of the declared set — check these before relying on it

**Every regex in that parser is a place the declared set can go wrong, and it can go wrong in
two directions that break the verdict in opposite ways. Only one of them is loud.**

| Direction | What it does | How it reads |
| --- | --- | --- |
| **over-match** — something counted that is not a test | **manufactures** a drop | loud: a name you can chase, and chasing it disproves it |
| **under-match** — a test silently omitted from the declared set | **hides** a drop | silent: the gap simply never appears, and the gate reports clean |

The first is the one people guard against, and it did happen: the tool's first parser counted
`private func testingGroup() -> PeopleChat` as a test, read 620, and manufactured a phantom
dropped test — the `AvatarPickerTabsTests` phantom from the opposite end. Its current `FUNC`
regex excludes non-`Void` returns and anything taking arguments, which fixes that one.

**The second is the more dangerous one, because a shrunken declared set makes the reconciliation
agree.** Fewer declared names means fewer disagreements means a clean report. The gate's output
in that case is indistinguishable from a correct one.

So when auditing this parser, **do not only ask what it might wrongly include. Ask what it
cannot see at all** — the table below is that list, and it is a list of silent failures.

Four more ways the source set can be wrong. **Checked against this tree at `9a09c8166` on
2026-11-07; none applies today, and all four remain live hazards the moment the tree changes:**

| Hazard | Effect | Status 2026-11-07 |
| --- | --- | --- |
| a test method inherited from a base class runs once per subclass, under a different name | declared once, reported N times | **absent** — all 111 test classes subclass `XCTestCase` directly; the only other class is `FakeSocket: RoomSocket`, a fake |
| a method inside `#if`, or commented out, is declared and never compiled | declared, never reported | **absent** — zero `#if` and zero commented-out `func test` in either test dir |
| a suite using Swift Testing's `@Test` instead of `func test` is invisible to the parser | whole suites missing | **absent** — zero `@Test`, zero `import Testing` |
| a class declaration carrying an attribute (`@MainActor class FooTests: XCTestCase`) does not match the parser's `CLASS` regex, silently dropping that class's tests | whole classes missing from declared, surfacing as `NOT DECLARED` | **absent** — no attributed class declarations today |

### The classifier's own blind spot: a single-test class

The `LOST VERDICT` / `NEVER BUNDLED` split is decided by asking whether any **sibling** of the
class reported. For a class with exactly one test that heuristic cannot separate the two causes
at all — a lost verdict line looks identical to a class that never bundled, and the tool will
call it `NEVER BUNDLED`, i.e. report a defect where there may be none.

This is live in this tree. Single-test classes at `32fb53d11`:

- **`ComposerDraftLifetimeTests`** — in the **unit** bundle, where **there is no rollup to
  corroborate against**. This is the sharp case: nothing in the log can settle it.
- `CaptureRigUITests` and `TranscriptSpacingUITests` — UI bundle, where the rollup can.

That is why `TranscriptSpacingUITests`'s `NEVER BUNDLED` verdict is trustworthy and an
equivalent unit verdict would not be: it is corroborated by the UI rollup (68 executed against
69 declared) and by a separate audit of why it never compiled, not by the heuristic alone.

**So treat a `NEVER BUNDLED` verdict on a single-test unit class as unclassified, not as a
defect, until something outside the heuristic settles it.**

#### How to close it properly: ask the bundle, which is the thing that knows

The bucket stays unclassified only because **both current sources are indirect about bundling**
— the source tree says what was *written*, the log says what *spoke*, and neither is the bundle.
The built `.xctest` knows exactly what it contains, and that is precisely the tie this turns on:
**present in the bundle with no verdict line is a lost verdict; absent from the bundle is never
bundled.** No heuristic, no siblings, and it works for a one-test class as well as a forty-test
one.

Candidate: `xcodebuild test-without-building -enumerate-tests -test-enumeration-format json`,
with `nm` over the built binary as the cruder fallback.

**Status 2026-11-07: NOT VERIFIED, and deliberately not attempted. Do not wire it in on this
description.** What is known:

- The command family **is** already used in this repo on the EC2 Mac —
  `scripts/feedback/ios-gate.sh:60` runs `xcodebuild test-without-building -xctestrun …`, and
  the `.xctestrun` plumbing exists (`scripts/ci/xctestrun-env.py`). So the approach is
  plausible and the inputs it needs are already produced.
- Whether `-enumerate-tests -test-enumeration-format json` is available and well-formed on the
  hosted image is **unknown**, and nothing in the repo or in any captured log exercises it.

Why it was not tested: verifying it needs a macOS runner. `swift-core-check.yml` runs on
`macos-15`, GitHub's **shared hosted pool** — note the contention is the pool, not the
concurrency group, which is per-ref. `ios.yml` carries a documented incident from 2026-10-23
where six branch runs queued ahead of `main` and no TestFlight build processed for 90 minutes.
Spending pool capacity on a **latent** hazard, while client legs and a release path need it, is
the wrong trade.

**The acceptance test, for whoever does run it — and it is not optional.** An enumeration that
silently omits a class would be a *fourth indirect source pretending to be a direct one*, which
is strictly worse than the honest gap above, because it would resolve ties confidently and
wrongly. So it must be validated against a known population before it is trusted, exactly as any
other filter must: run it at a commit whose answer is already established — `32fb53d11`, where
`TranscriptSpacingUITests` is known never-bundled and the other 618 are known present — and
require it to reproduce **that exact split and nothing else**. If it does, wire it as the
tie-breaker and the causes table loses its unclassified bucket. If it does not, leave the verdict
unclassified and record why.

## Never carry a total without its composition

**There are two different 619s in this material, and a reader who matches them concludes the
rebase changed nothing.**

| | Composition |
| --- | --- |
| 619 at `32fb53d11` | **source-declared**, of which 1 never reaches the bundle → **618 runnable** |
| 619 after rebasing onto `main` | **runnable**, after `main`'s `AvatarPickerTabsTests` rewrite |

Same number, different quantities. So: **state declared, minus never-bundled, equals runnable,
each named, every time.** A bare total invites exactly the false match above, and it is also how
the next person avoids re-deriving all of this.

## The Swift per-case log is an artefact, not `gh run view --log`

`gh run view --log` on a Mac leg returns a **truncated** archive. On runs 42286751308 and
42286753339 it came back as 453 KB ending mid-teardown, with **zero** `Test case` lines in it.
Every per-case extraction over it returns 0, and 0 is indistinguishable from "the suite did not
run" — a vacuous count that looks like a catastrophic one.

The real output is an upload: `gh run download <run-id>` gives
`swift-core-check-logs-<sha>/xcodebuild-test.log`, ~1.39 MB, with every verdict line in it. Check
`gh api repos/<owner>/<repo>/actions/runs/<id>/artifacts` for expiry first.

So: **a Swift count of zero is a tooling result, not a measurement.** Before reporting any Swift
number, confirm the log you read actually contains verdict lines at all. This is the vacuity rule
applied to the source rather than the filter, and it is the cheapest of the lot to check.

## Gate 3 and gate 4 must compare against the base that will actually land

A PR's diff is only meaningful against the commit it will be merged into. Two ways this goes
wrong, with the same root cause:

**1. A PR based on another PR's branch.** When head's base ref is not the trunk, comparing it to
the trunk measures the parent PR's work as though it were this PR's. Treat the pair as a stack:
merge the parent first, then retarget and rebase the child, and take the pre-rebase pin against
the **parent's head** and the post-rebase pin against the trunk only once the parent is in it.
Never pass a PR whose base is another PR's branch through gate 3 against the trunk.

**2. A trunk that moved after the branch forked.** This one is subtler because nothing in the
PR's metadata looks unusual. Worked example: #2881 at `32fb53d11` diffed against live `main`
showed `AvatarPickerTabsTests` losing three names and gaining two — a net dropped test, exactly
the signature the dropped-test rule exists to catch. Diffed against its **merge-base**
`4cb9567db`, that file is byte-identical and the PR never touched it. Two commits (`45ae4846b`,
`0a59a2f30`) had landed on `main` after the fork and rewritten those tests. The "drop" was `main`
moving forward, read backwards as the branch moving back.

**So derive the base, never assume it:** `git merge-base origin/main <head>`, and if it is not
`origin/main`, every name that differs is suspect until you have checked whether the trunk or the
branch changed it. `git log <merge-base>..origin/main -- <path>` answers that in one command.

The general form: **a difference has two ends, and a gate that names only one of them is
guessing.** Before adjudicating any dropped name, establish which side actually moved.

### The repo already implements this rule — cite it rather than the kit

`scripts/release/swift-gate-applies.sh` arrived at the same principle independently, and a rule
the codebase already follows somewhere is far easier for the next worker to accept than one that
exists only here. It does three things worth copying:

1. it extracts the quoted patterns with an awk block **shared verbatim** with `checks.sh`'s
   `gate_patterns` — "unchanged on purpose. The two read the same bytes and must agree", so the
   two cannot drift into disagreeing about what the gate covers;
2. it judges applicability on the **three-dot** diff — what the merge would introduce, not what
   the two tips happen to differ by;
3. it **unions this head's `paths:` block with `origin/main`'s**, so a pattern added to `main`
   after a branch forked still gates the merged result. Judging by either list alone "reopens the
   hole from the other side".

**Be precise about what the union defends, because it is narrower than it looks.** It defends
against **pattern drift** — the gate's own definition changing under the branch. It does *not*
defend against **content staleness**: a leg that ran green at an old base is still a verdict
about old content, however correctly the gate decided it applied. That residue is what gate 3
and the rebase-then-rerun discipline cover. The two are complementary, not redundant, and a
worker who reads the union as "the base question is handled" will skip the rebase.

### "May this merge" and "where would it merge to" are independent questions

Every hold, freeze and trigger-set classification in this lane answers the first one. **None of
them answers the second**, and a PR can be unconditionally clear on every hold while its base ref
points somewhere that is not the trunk. Classify a queue by what it touches and the base ref is
never consulted, so a mistargeted PR lands in whichever group its paths put it — including the
free group, where the next thing that happens is a merge.

The variant to look for is the one that does not look like a stack. A base whose parent PR is
**merged** is a *dead base*, not a stack: there is no parent to wait for, nothing is pending, and
every stack-shaped heuristic passes it.

#### The measurement, because it decides how serious this is

Taken on #2875, 2026-11-07, whose base ref was `cursor/crash-loop-leftover-tests-stale-prose-2fe2`
— the head branch of #2874, merged some eight hours earlier:

- the branch **still exists** on the remote at `9c8ba357e`. Merging #2874 did not delete it;
- `git merge-base --is-ancestor 9c8ba357e origin/main` → **true**;
- `git log origin/main..9c8ba357e` → **empty**.

So the branch is not stale, not diverged and not conflicted. It is strictly behind the trunk,
which means the git operation underneath the merge is a **fast-forward that succeeds**. Exit
zero. No error, no conflict, no warning, a green merge in the UI — and `main` never receives the
commit. Every layer that reports anything reports success.

#### Therefore `merge.sh`'s base refusal is load-bearing, and must not be relaxed

```sh
base=$(gh pr view "$PR" -R $R --json baseRefName --jq .baseRefName)
[ "$base" = main ] || { echo "REFUSED: #$PR targets '$base', not main ..."; exit 2; }
```

**Read that as the only thing standing between this lane and a merge that returns exit zero while
the trunk never receives the commit.** It is not belt-and-braces over an operation that would
otherwise fail loudly; the operation it prevents is a silent success. Nothing downstream would
catch it, because there is nothing to catch — the gates all passed, the merge all passed, and the
only evidence is a commit's continued absence from a branch nobody re-reads.

**This line must not be made conditional, and the temptation to soften it is predictable.** A
future reader meets an unconditional `[ "$base" = main ]`, notices it blocks legitimately stacked
PRs, and relaxes it to something like "allow a base that is another open PR's head branch". That
instinct is not wrong about stacks — but a relaxation phrased over the base's **name**, or over
the mere existence of a PR pointing at it, readmits exactly the dead-base case, because a dead
base *has* a plausible name and *had* a real PR. If the line is ever relaxed, the condition must
be about the base's **liveness**, checked directly and at merge time:

- the parent PR is **`OPEN`**, re-read now rather than remembered; and
- the base branch's current oid **equals that parent's head oid**; and
- the base is **not already an ancestor of the trunk** — the dead-base signature, and the one
  check that stands on its own even if the first two are somehow satisfied.

Until a relaxation carries all three, the unconditional form is the correct one. Retarget the PR
instead; that is a one-field edit and it is always available.

Two further things follow. **Do not route around the refusal** — the fix is to retarget, never to
merge where it points. And **do not rely on it to find the problem for you**: it fires at the
merge call, after the gates have been run and the window spent.

#### Therefore: the base-ref audit is a standing step, not a reaction

**Enumerate the base ref of every PR in the queue whenever the queue changes or the window is
shut.** One `gh pr view --json baseRefName,headRefName,headRefOid` per PR, a few seconds for a
queue of twenty, and it can be run at the one time when nothing else can be: while merging is
blocked. It is the cheapest read in the lane and the only one that answers *where would this go*.

Run it as the two-way check, because `!= main` alone is vacuous on a stack. For each PR the base
must be **either `main`, or the head branch of a parent you have independently confirmed `OPEN`
right now** — and in the second case the base branch's current oid must **equal** that parent's
head oid. A base branch that has moved past its parent's head is its own separate problem and
this is the only step that would see it.

**The argument for making it standing is not the dead base itself — it is what the audit joined
up.** The mis-based PR sat in this lane's free group, apparently ready. Its dead base was *also*
the last blocker on a second piece of work in another worker's lane, whose follow-up was waiting
on it and believed the dependency sat with the coordinator. Neither side could see the join:
this lane had a PR it thought was ready, the other had a block it thought was someone else's,
and both beliefs were locally consistent. **A cross-owner dependency is invisible from inside
either owner's view by construction** — each sees one end — so no amount of care on either side
surfaces it. Only a read across the whole queue does.

Generalise past base refs: **a cheap read over the entire queue finds couplings that no
per-PR check can, because the coupling is not a property of any single PR.** Prefer one such
sweep, run at a time when it costs nothing, over per-item diligence that is individually
thorough and structurally blind. When the window is shut and there is time to fill, that sweep is
the highest-value thing available.

The audit is a two-way check, not a `!= main` scan. For each PR, the base must be either `main`
or **the live head branch of a parent you have independently confirmed is still open**, and that
parent's head oid must equal the base branch's current oid — a stack whose base branch has moved
past its parent's head is its own, separate problem.

## Equal name-set sizes prove nothing about meaning

Gate 4 compares test **names**. A rename is therefore the cheapest thing in the world to get
wrong: one name out, one name in, counts unchanged, "nothing dropped" — and the behaviour under
test can have been reversed.

**The cheapest tell of an inversion is a name whose subject and object have swapped places.**
Worked example, from #2881:

| | |
| --- | --- |
| before | `testAPeopleRowsCountPillWinsTheSlotOverTheSpinner` |
| after | `testTheSpinnerWinsAPeopleRowsSlotAndNoPillCompetesForIt` |

The pill won; now the spinner wins. The same two nouns, the same verb, the roles exchanged. A
name-set diff reports this as "one renamed, nothing dropped", which is **true and still hiding a
behavioural reversal**. A size comparison cannot see it at all, because the sizes are equal by
construction.

Score such a pair as **neither a drop nor a clean rename**. Hold it as unresolved and require
three things from whoever owns it:

1. the new behaviour stated **positively** — what is now true, not merely that the old assertion
   is gone;
2. evidence the inversion was **intended**, pointing at the production change that motivates it;
3. the citation clause discharged — nothing left in the tree asserting or citing the old
   behaviour.

#2881 discharged all three in the test itself: the comment names the production change and its
date, the body asserts the new order positively, and it adds `XCTAssertNil(body.range(of:
"UnreadMark("))` so the absence of the pill is asserted rather than merely no longer checked. The
production code moved in the same commit, from `if count > 0 { … } else if streaming {` to `if
streaming { … } else if unread || count > 0 {`.

### The cheapest discriminator: did the deletion remove a dissenter from an existing invariant?

**Check this first. It is cheaper than anything else here and much harder to fake.**

**When a deleted assertion contradicted an invariant asserted elsewhere, and that other
assertion predates the branch and passes untouched, the deletion is a convergence rather than a
reversal.**

A test bent to match a regression **cannot produce that shape**. It has no older sibling already
asserting the new behaviour — if such a sibling existed and passed, the "regression" would be the
codebase agreeing with itself. The shape is self-authenticating in a way a comment is not.

What to establish, all of it from the tree and none of it from the change's own prose:

1. an assertion of the invariant that **exists at the merge-base**;
2. is **byte-identical at the head** — diff the function, not the file, since the branch may
   legitimately touch the file for other reasons;
3. and that the deleted test **contradicted** it.

Worked instance, #2881. The deleted
`testAPeopleRowsCountPillWinsTheSlotOverTheSpinner` said the count pill outranks the spinner.
Meanwhile:

- `WorkingSpinnerTests.testTheRowStatusShowsOneThingInThisOrder` is **byte-identical at
  `4cb9567db` and `9a09c8166`** and asserts `RowStatus.of(streaming: true, unread: true) ==
  .working` — streaming wins.
- Swift `RowStatus.of` in `HomeView.swift` is byte-identical across the branch: `if streaming {
  return .working }` first.
- Rust `RowStatus::of` in `crates/arbos-mac/src/ui/sidebar.rs` is byte-identical across the
  branch and does the same, on the other client.

So **both clients had always put streaming first**, asserted by a test the branch does not touch,
while `PeopleRow` was the single dissenter running its own chain and never consulting the shared
rule. The app held two contradictory rules with a passing test for each, because the dissenting
row never asked the shared one. Deleting the dissenter is convergence.

The diagnostic worth generalising beyond this case: **a passing test is not evidence of a single
rule.** Two contradictory rules can both be green indefinitely, as long as nothing makes one
consult the other. When a deletion resolves such a pair, the "dropped test" is the bug report.

### When there is no such sibling: requirement 2 needs a record the author did not write

This is the **fallback**, for an inversion with no older sibling to converge on. Use the
discriminator above first.

The three checks establish the inversion is **coherent**. They cannot establish it is
**wanted** — and that gap is the whole risk, because **a test bent to match a regression scores
identically on all three.** It states its new behaviour positively, production "moves with it"
(that is what the regression *is*), and no stale citation remains (the author removed them).

**A comment naming a ruling is the change's own testimony, exactly like a PR body claiming
green.** The lane does not accept a PR body's word for CI; it reads the step list. Apply the same
standard here.

So requirement 2 above is not discharged by the comment. It is discharged by **naming the
external authorization and where it was found** — a record the author could not edit as part of
the change. #2881's was discharged from the issue tracker, not from the diff: **AF-1312, "Rule on
the unread mark, since the reference contradicts two standing instructions", in Done** — an
escalation the user answered — and **AF-1317, "Give the collapsed rail's picture badges the same
unread dot", open** — the same ruling propagating a dot where a pill used to be. Two rows this PR
cannot touch, agreeing with the comment's citation of Jacob on 2026-11-05.

**"The comment says Jacob asked for it" is not a discharge.** Name the record and where you read
it, or the requirement is outstanding.

## Gate 4 on a Swift leg: a stable absence can be a counting artefact

Gate 4 compares test **names** across a rebase and adjudicates anything missing as a dropped test.
On a Swift leg a name can go missing for a reason that has nothing to do with the diff.

xcodebuild interleaves its own status writes into the result stream and bisects result lines. In
the clone phase each case emits exactly **one** line — its verdict, with no separate `started`
line — so a bisected line removes the test from the log entirely. It is invisible to a verdict
count **and** to a unique-name count, which is why two independent counts can agree with each
other and both be short by the same one.

### There is no hint. Do not look for the victim by name.

This section used to end with a named test to check first. **It no longer does, deliberately.**

Three readings of which test gets cut have been recorded here, and each was wrong on the next
log: "a different victim each time", then "it correlates with position in the run", then "the
invariant is the test, `SearchSettingsTests.testEveryListedSettings…`". Every one was true of
every log available when it was written.

Over the same five logs the **mechanism** — refuse to count while any line mentioning a test case
is unreadable — was right every time, and it needed none of those specifics to work.

That asymmetry is the finding, and it is worth more than any victim name: **when a mechanism that
needs no specifics has held every time and the specifics have been revised every time, the
specifics are history, not a hint.** A named victim is actively harmful here, because it invites
the next worker to shortcut the mechanism and grep only that suite — which is exactly the move
that would have missed the fifth log.

**So: the mechanism must never come to depend on the observations below.** They are recorded as
dated observations, for anyone investigating *why* the truncation happens. They are not a
shortcut for detecting *that* it happened, and no gate may be run off them.

| Date | Logs | Observed cut |
| --- | --- | --- |
| up to 2026-11-06 | three logs, lines 12934 / 12953 / 12865 | `SearchSettingsTests.testEveryListedSettings…` |
| 2026-11-06 | runs 42286751308, 42286753339 (#2881, #2883 green legs), lines 12941 / 13039 | `SearchSettingsTests.testExactKeywordPutsSettingsFirstAndNamesTheKeyword` |

Line offsets span 174 lines, which rules out run position. Both tests sit in `SearchSettingsTests`
and nothing yet explains why that suite — treat even *that* as an open observation, not a rule,
since it is the same kind of generalisation the three wrong readings were.

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

**There is no "where to look first".** See the subsection above: the refusal is the whole
procedure. Running it costs one command and holds for a victim nobody has seen yet.

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

### Two counting methods that differ by one: the worked reconciliation

Two workers counted the same green legs (#2881 at `32fb53d11`, #2883 at `4a101489f`) and got
**618** and **617**. A difference of one between a declaration census and a verdict count is
indistinguishable from a dropped test until the methods are reconciled, so neither number may be
adjudicated on. Resolved as follows, identically on both legs:

| | #2881 | #2883 |
| --- | --- | --- |
| `ArbosTests` regex matches | 552 | 552 |
| less `AddMembersPlanTests.testingGroup` (private, returns `PeopleChat`) | −1 | −1 |
| **discoverable unit cases** | **550** | **550** |
| `ArbosUITests` regex matches | 69 | 69 |
| less `TranscriptSpacingUITests` (never compiled) | −1 | −1 |
| **UI cases** (confirmed by the bundle's own `Executed 68 tests, with 13 tests skipped`) | **68** | **68** |
| **declared total** | **618** | **618** |
| unit clean verdict lines in the log | 549 | 549 |
| unit verdict lines truncated | 1 | 1 |
| UI passed / skipped | 55 / 13 | 55 / 13 |
| **observed total** | **618** | **618** |

**618 is right, and the two methods agree exactly once the truncated line is restored.** The
missing name is
`SearchSettingsTests.testExactKeywordPutsSettingsFirstAndNamesTheKeyword`, bisected at line 12941
of #2881's log by an `xcodebuild[17898:53966] [MT] IDETestOperations` write and at line 13039 of
#2883's by an `iOSSimulator: 📱<DVT` write. It is **a real case that ran and passed** — not a
duplicate, not a parameterised expansion, and not a drop.

The classification worth remembering is the fourth one: it is an **artefact of where each method
reads**. Verdict counting cannot see a case whose only line was destroyed, and nothing in the
total says so; declaration counting can only see what a regex believes XCTest will discover. The
two fail in opposite directions, which is exactly why agreeing them is informative and why either
alone is not.

Note also that the 619 recorded above for run 42277644010 is **not** wrong — that tree declared
551 discoverable unit cases (552 matches less `testingGroup`) against these two legs' 550, and
551 + 68 = 619 just as 550 + 68 = 618. Each figure is correct for its own tree. **Never carry a
Swift total between commits; re-derive it.** A stale total is the most expensive kind of
phantom drop, because it arrives already believed.

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

And check the required checks are *present*, not just green. A gate that is absent because the
head is stale and a gate that is absent because a path filter excludes it look identical from
the outside: in both cases there is simply no run, and the remaining badges are green. The
boring explanation is usually the true one, which is exactly why it should never be allowed to
stand in for looking. Confirm the gate ran at the head you are merging — not at an earlier
head, and not by inference from why it probably would.

### The canonical illustration: fifteen green conclusions over one run of the job

This is the worked example to reach for, because the two questions have different answers *on
the same page* and the misleading one is the one that looks reassuring.

The question asked was whether the Swift leg had run green on `main` recently. `swift-core-check`
showed **fifteen consecutive `success` conclusions** on `main`. The honest answer is **once**, at
`2890272b9`, five hours earlier. In the other fourteen, `swift-core-check.yml`'s applicability
gate decided the push did not touch the Swift surface, so the real `build-xcframework + fastlane
test` job was **skipped** — and a workflow whose only substantive job skipped concludes `success`,
indistinguishably from one that built the xcframework and ran the suite.

Note what each layer reports, because the deception is structural rather than a bug:

| what you read | what it says | what it does not say |
| --- | --- | --- |
| the check rollup | `success` ×15 | whether any job did work |
| the run's job list | the real job is `skipped` | — this is where the answer is |
| the step list | the build/test steps never ran | — |

So: **"is this gate green" and "did this gate run" are different questions, and the gate-presence
rule exists because only the second one is load-bearing.** A skipped job is not a weaker pass; it
is not a pass at all, and it is the *only* outcome that gets more reassuring the further you stand
back from it. Red fails loudly, absent leaves a visible hole in the list — skipped-rendered-green
is the one failure mode whose summary is actively wrong.

Operationally: to answer "has this gate run recently", list runs and then **open them and read the
job conclusions**, taking the most recent run in which the substantive job was neither `skipped`
nor `cancelled`. Never answer it from `gh run list`'s conclusion column. The column cannot
distinguish the two cases, so a count taken from it is a count of pushes, not of verdicts.

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

### Release also runs on a `*/15` cron, so a NO-GO on "Release in progress" may be 25 seconds old

`release.yml`'s triggers are not only `workflow_run`, `push` and `workflow_dispatch`. There is
also:

```yaml
  schedule:
    # Backstop for continuous delivery: a green commit whose Release run was lost
    # (cancelled while pending, main moved during the bump) ships within 15 min.
    - cron: "*/15 * * * *"
```

Two consequences, and they point in opposite directions.

**For waiting:** this is a *second* path to the bump. If the `workflow_run` Release is lost —
cancelled while pending, or `main` moved during the bump, which is exactly outcome 2 above — the
cron picks the green commit up within fifteen minutes and publishes anyway. So a bump that has
not appeared is not yet evidence of anything until a cron tick has also passed. Do not conclude
a release failed inside that interval.

**And withdraw the manual remedy, because a scheduled fallback makes it actively harmful.** This
is the consequence that is easy to miss, and it generalises past this workflow. The standing
instruction here was: *if no Release appears within five minutes of a green rerun, dispatch
`gh workflow run release.yml` by hand.* That was correct before the cron was known about. It is
now wrong — the hand dispatch cannot make the release happen sooner than the cron already will,
and it **adds an in-flight `release.yml` run, which NO-GOs the merge window** for as long as it
lasts. Pure cost.

**The pattern, worth carrying to the next workflow that grows one: a scheduled fallback trigger
does not merely make a lost event survivable — it makes the manual remedy for a lost event
harmful.** Any runbook step of the form "if X has not happened by T, trigger it yourself" must be
re-read the moment a fallback schedule is added, and in most cases deleted outright. The step
stays plausible forever on its own terms, because the thing it does still works; what changed is
that it is now redundant *and* has a side effect the author never priced in. Audit for those
steps when you discover a cron, rather than only recording that the cron exists.

**For merging:** `merge-window.sh` NO-GOs on *any* in-flight `release.yml` run, and the cron
starts one regardless of what this lane does.

The arithmetic, so no operator is surprised by it. A declining scheduled run is short — measured
at **25s** (`42324383811`, 01:30:04Z → 01:30:29Z) and **20s** (`42299302848`) — because the plan
job declines and every downstream job skips. Firing every fifteen minutes, that is a closed hole
of roughly twenty to twenty-five seconds in every nine hundred, about 2–3% of wall clock, placed
where nobody chooses it. So:

- a NO-GO naming a seconds-old Release run on a `schedule` event is the **expected case**, not a
  surprise and not a fault;
- the correct response is to **wait it out and re-run `merge-window.sh`** — a few tens of seconds;
- if the verdict survives the wait, it was never the cron, and it is a real release.

**Never reach past it.** The cheapness of the obstacle is exactly what makes overriding it
tempting and exactly why overriding is unnecessary: a wait that costs under a minute does not
need a waiver, and a waiver built for this case would be available in the case that matters. The
distinction is `event == "schedule"` plus all publish jobs `skipped`, readable in one call, and
it is an observation rather than a judgement.

### Why the chain terminates: the two pushes to `main` carry different credentials

The release chain is only finite because of an asymmetry that is invisible in both the workflow
file and the log, and it answers two questions at once.

GitHub does not start workflow runs for events pushed with Actions' **default `GITHUB_TOKEN`**.
That is a documented anti-recursion rule, not an accident of configuration. `release.yml`'s plan
job checks out with `actions/checkout@v4` carrying no `token:` override and no
`persist-credentials: false`, so the credential left in `.git/config` — the one `auto-stable.sh`'s
push to `main` then uses — is that default token. Therefore:

- **the bump push triggers nothing.** Zero runs. Not a filtered-out run, not a skipped job: no
  run object is created at all. This is why the release does not loop, and it is the mechanism
  behind the observation that a bump commit has no CI of its own.
- **a merge push made with the kit's PAT triggers everything its paths match**, including the
  Release run that then declines with `not a hotfix`.

Two consequences for this lane, and they pull in opposite directions, which is why the mechanism
is worth carrying rather than just the observation:

1. **Never reason about a bump commit's CI.** There is none, and its absence is not a signal of
   any kind. A bump that rewrites `Cargo.toml`/`Cargo.lock` version lines matches
   `swift-core-check.yml`'s and `ios.yml`'s `paths:`, and still produces no run, so "those paths
   match, therefore the leg will run" is false for this one commit specifically.
2. **A merge push is not equivalent to a bump push, and the difference is the credential, not
   the content.** Any cost model that reasons "the release just pushed to `main` and nothing
   fired, so my push won't fire anything either" is comparing two pushes that differ in exactly
   the way that decides the question.

To check the mechanism rather than trusting this paragraph: take a bump commit's sha and ask for
its runs. An empty list is the signature — distinguishable from a filtered run, which still
appears with a `skipped` job.

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

### Answer a runner question from the `runs-on` expression, never from the note explaining it

Same rule, one field over, and it bites harder because the misleading text is *more* convincing
than a stray grep hit: it is a prose comment written by someone who knew what they were doing.

`ios.yml` carries a detailed note about the night of 2026-10-23, when six branch runs queued
ahead of `main` on GitHub's hosted `macos-15` pool and no TestFlight build processed for 90
minutes. Read as current, it says the TestFlight build and every branch Mac run compete for one
shared pool. **It is not current.** `14e84cff4` was the response to that incident, when `ios.yml`
did run on the hosted pool; `53a9ef257` later moved it to the self-hosted EC2 Mac. The comment
survived both commits. The line itself says:

```yaml
runs-on: ${{ inputs.runner || 'arbos-mac' }}
```

**A comment citing an incident is evidence about the past. The `runs-on` line is the fact.**

The practical shape of the answer, when it was finally taken from the expressions:

| Workflow | `runs-on` | Resource |
| --- | --- | --- |
| `ios.yml` — produces the TestFlight build | `${{ inputs.runner \|\| 'arbos-mac' }}` | self-hosted EC2 Mac |
| `swift-core-check.yml` — the Swift leg on PRs | `macos-15` | GitHub hosted pool |
| `release.yml` Mac job | `${{ inputs.runner \|\| vars.ARBOS_MAC_RUNNER \|\| 'macos-15' }}` | resolves to `arbos-mac` in practice |
| `ios-capture.yml` | `${{ inputs.runner \|\| 'macos-15' }}` | hosted pool |

Note the third row: an expression can depend on a **repository variable**, which is not in the
tree and which a read-only token cannot fetch (`gh api …/actions/variables` returns 403). When
that happens, say the expression is unresolved and find an observation of what it resolved to —
`ios-capture.yml` records that `release.yml`'s Mac job "resolved to the `arbos-mac` label on each
of its four most recent runs". **Do not quietly take the literal default as the answer**; it is
the last branch of the expression, not the value.

This is the same class as the truncation hint and as authorization-by-comment: explanatory prose
ages independently of the thing it explains, and nothing fails when it goes stale.

## A gate on a recorded verdict is not satisfied by fixing the thing that produced the verdict

The merge window asks: **what is the recorded CI verdict of the commit already on `main`?** It
does not ask what will be green next. Those are different objects, and substituting one for the
other produces an argument that is entirely sound and still does not open the window.

Worked instance, 2026-11-07. `main` was red at `ecb04054b` on exactly one failing test. #2897
repaired that very test — verified: the failing case reads `ok` at its head, no `FAILED` line
anywhere, and agentd goes `525 passed; 1 failed; 2 ignored` to `526 passed; 0 failed; 2 ignored`,
so the counts close and the green is the repair rather than a population change. The reasoning
was that merging the fix removes the cause and therefore beats re-running. It does remove the
cause. The window still refused:

```
VERDICT: NO-GO — ecb04054b JUDGED AND FAILED CI since v0.6.399
NOT MERGED: verdict is not GO
```

Only a fresh green run **on `ecb04054b` itself** changes that record. The fix cannot retroactively
alter the verdict of a commit that already landed.

**So the rerun was not avoidable, and skipping it did not save a step — it blocked the fix.** The
tell to learn: when a refusal names a **commit** and a **recorded result**, no change to any
*other* commit will clear it. Ask what would rewrite that record, and the answer is almost always
"re-run the thing that wrote it".

**And check no override exists before assuming one does.** `accept-red` looks like it fits and
does not: it is consumed *after* the verdict, against the pull request's own rollup only, and
`merge.sh` says so outright — "it cannot waive a required gate: that refusal already happened,
above, and took no arguments". `merge-window.sh` has no override flags of any kind. That is
deliberate, and the one recorded attempt to route around a refusal — reading it as waivable and
falling back to `gh pr merge` on 2026-11-05 — landed a merge inside a closed release window.

### The corollary that is easy to miss: the rerun is also sufficient

A green run on the red commit does two things at once. It opens the window, **and** it lets the
`workflow_run` Release fire at that commit, so the version bump comes out of the rerun itself.
The fix is then a follow-on, not a precondition, and nothing about the release waits on it.

Which creates a narrow window that must not be merged into: **between CI going green and the
bump landing on `main`.** A push in that gap takes `auto-stable.sh`'s "main moved meanwhile"
branch and leaves the release published under its tag and on no branch — spending the version
for nothing. Sequence it as: bump lands, ancestry check discharges, then merge.

### A premise check must not run through the code the test is about

The rerun above is also a *premise check*: it asks whether `ecb04054b` is genuinely red, and the
whole argument for merging #2897 rests on the answer. The discipline that makes such a check
worth running is that **it must not be evaluated by executing the code under repair.**

The failure is self-confirmation, and it is quiet. If the premise is "this test fails for reason
R" and the check runs the same test through the same code path, a pass is equally well explained
by the repair, by a flake, by a path-class difference in where it ran, or by the premise being
wrong all along — and the four are indistinguishable from the result. The measurement shares a
failure mode with the thing it is supposed to adjudicate, and two measurements with the same
failure mode are one measurement.

What a sound premise check looks like here: the record of the run that produced the verdict, the
failing case's own log line, and the before/after counts closing exactly — all **artefacts of
the original run**, none of them re-derived by re-executing the repaired code.

This is instance 2 of *the quantity that settles a question must come from outside the thing
being questioned* — see the companion principle at the top of this document, where the other two
instances and the general form are recorded.

The practical test before accepting any premise: **ask what a pass would rule out.** If the
answer is "nothing I did not already believe", the check is decorative.

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
