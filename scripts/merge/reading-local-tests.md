# Reading a run before a gate

A gate is only as good as the run behind it. These are the ways a run has lied to this lane,
written down so the next reader does not rediscover them one merge at a time.

They share a shape: a correctness that holds by accident, or depends on something nobody would
guess was load-bearing. None of them has a failing test to announce it when it goes.

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

## Gate 4 on a Swift leg: a stable absence can be a counting artefact

Gate 4 compares test **names** across a rebase and adjudicates anything missing as a dropped test.
On a Swift leg a name can go missing for a reason that has nothing to do with the diff.

xcodebuild interleaves its own status writes into the result stream and bisects result lines. In
the clone phase each case emits exactly **one** line — its verdict, with no separate `started`
line — so a bisected line removes the test from the log entirely. It is invisible to a verdict
count **and** to a unique-name count, which is why two independent counts can agree with each
other and both be short by the same one.

**The victim is not arbitrary.** Measured across two logs from different pull requests, the cut
line was the *same* test both times — `SearchSettingsTests.testEveryListedSettingsPa…`, at line
12934 and line 12953, with different junk interleaved. It correlates with position in the run,
where the end-of-testing status writes land.

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
