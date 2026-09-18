---
cursor:
  subagentId: "bc-b4f4cdba-0146-5dea-9731-24ea2538adcd"
---

# The `checkpoint_refs` flake, with its mechanism — and the six failed runs behind the family

Companion to `internal/kernel-ci-flakes-2026-09-18.md`, which is the iPhone loop's
(`bc-7c66cfa8-381e-5700-9d78-3129f338a4fa`) and is not edited here. That document established the
pattern — the same commit passing and failing, three tests in two hours, none of them the branch's
fault. This one adds the **mechanism** for the one that has now failed twice, and the run table behind
it, from the QA break-and-fix loop on `qa-vm2`.

## The red on [#555](https://github.com/unarbos/arbos/pull/555)

`#555` is `cursor/path-claims-b027` (kernel: the claim frame), merged. Its kernel job failed on

```
test tools::git::tests::checkpoint_refs_are_dropped_by_line_and_by_agent ... FAILED
panicked at crates/arbos-engine/src/tools/git.rs:1981:9
refs/arbos/cp/root/7
```

The branch touches no `git.rs`, the steward reran it, and the rerun is green (run `35302439407`, 03:14).

## What it actually asserts, and what it forgot to

The test (`git.rs:2260`) makes two checkpoints and then checks the refs:

```rust
for (line, name) in [(3u64, "a.txt"), (7, "b.txt")] {
    std::fs::write(dir.join(name), format!("{line}\n")).unwrap();
    let cp = snapshot_turn_record(&dir, &agent_dir, "root", line).unwrap().unwrap();
    snapshot_turn_tree(&dir, &agent_dir, "root", &cp).unwrap();
}
assert!(refs().contains("refs/arbos/cp/root/3"), "{}", refs());
```

The failure message is the whole ref list, and it was `refs/arbos/cp/root/7` — **only the second
iteration left a ref**. That is the mechanism, because of how the ref is written:

```rust
if let Some(w) = &work {
    let _ = Command::new("git").args(["update-ref", &format!("refs/arbos/cp/{safe}/{line}"), w]) …
}
```

**The ref exists only when that snapshot made a work commit.** If the first `snapshot_turn_tree` made
none — the tree read as clean, which under load it can, git's index comparison being what it is — then no
ref for line 3 is written, and the test reports the drop-by-line feature as broken. Nothing in the test
checks that a work commit was made; `snapshot_turn_tree(...).unwrap()` succeeds either way, because
making no commit is a legitimate outcome, not an error.

So this is the family's shape exactly: **an assertion resting on a step that is neither guaranteed nor
checked.** It is the sixth review step from the other side — an assertion must not bound a race — and the
same fault as `#335`'s "a negative assertion must first prove it looked at something", inverted: a
positive assertion that never proved its own setup happened.

The two honest repairs, either sufficient:

1. Assert the precondition where it is cheap and exact: `assert!(cp.work.is_some(), "no work commit for
   line {line}; the ref this test reads is only written when there is one")` inside the loop. The red then
   names the real event instead of the feature, and a person reading it knows in one line whether the
   kernel misbehaved.
2. Make the setup deterministic, so a work commit cannot be skipped — commit a baseline first, or write
   the file and verify `git status --porcelain` is non-empty before snapshotting.

Not this loop's suite to change, so it is written rather than patched. The kernel owner has the fix in one
line if they want it.

## The six failed runs, and what each actually failed on

Read from `gh run view --log-failed` over the 14 most recent failed runs, taking only lines matching
`test … ... FAILED`, so a test merely *listed* in a log is not counted as a failure:

| run | failing test |
|---|---|
| `35297625836` (02:09) | `a_retired_workers_row_links_its_pr_when_it_opened_one` — `page_algorithm_e2e.rs:139` |
| `35296291397` (01:46) | `history_for_an_archived_worker_replays_its_record_and_says_so` — its setup wait, as the iPhone loop found |
| `35296046051` (01:38) | **`tools::git::tests::checkpoint_refs_are_dropped_by_line_and_by_agent`** — #555 |
| `35295665331` | `a_rewind_on_a_checkpoint_without_a_tree_leaves_the_files_and_says_so` |
| `35295335589` | **`tools::git::tests::checkpoint_refs_are_dropped_by_line_and_by_agent`** — the same test, a second time |
| `35294320123` | `a_coordinator_that_spawns_and_leaves_the_page_alone_is_nudged` — `audit_kernel_2_e2e.rs:138` |

Five distinct tests over six runs, with `checkpoint_refs` twice. Added to the iPhone loop's three, the
family now has at least five members in one night, and two of them — `checkpoint_refs` and the archived
worker — fail on their own setup rather than on the behaviour they name.

## `say_title_rename_e2e::a_queued_message_with_a_title_labels_the_turn_it_opens` — the sixth, and it reads a frame

Seen by the steward on [#549](https://github.com/unarbos/arbos/pull/549) (`cursor/cycle-39-aa39`), which
changes three files — `desktop/qa/parity/ui_pass.py`, `desktop/src/model/session.rs`,
`desktop/src/model/workspace.rs` — and **nothing under `crates/`**, so it cannot reach a kernel e2e test.
It passes 5 of 5 on `main`. It was the **first-attempt** red on that PR; the steward reran, attempt 2 was
green, and it merged as `be90ecfd`. So this is the strongest form the evidence takes — the same input
giving both answers, on the same run id, minutes apart.

That also corrects a method note in this document, and the correction matters to anyone else scanning CI
for flakes: **a rerun hides the red.** `gh run list` reports a run once, with its latest conclusion, so a
scan of failed runs cannot see a flake the steward has already reran — which is exactly why the 14-run
scan below found `say_title_rename_e2e` only in test *lists* and never on a `... FAILED` line. A scan of
failed runs under-counts this family by precisely the members someone has already dealt with. To see
those, the attempts have to be asked for by run id (`gh run view <id> --attempt 1`), or the steward's own
account is the record.

The mechanism is at `crates/arbos-kernel/tests/say_title_rename_e2e.rs:169`:

```rust
let status = a
    .wait(Duration::from_secs(40), |f| {
        f["type"] == "status" && f["agent"] == worker && f["step"] == "Add the hex codes"
    })
    .expect("the title is the live line of the turn the request opened");
```

It waits for a **transient frame**: the live line, which is a broadcast of what the agent is doing *now*.
And the worker's scripted reply for that very turn is

```
{"agent":"count-the-colours","content":"adding hex codes",
 "calls":[{"name":"bash","arguments":{"command":"sleep 6","wait_ms":30000}}]}
```

so the live line becomes the `bash` command almost immediately. The title is the live line only for the
instant between the turn opening and its first tool call, and nothing makes that instant observable — the
order in which the two status frames are emitted is not a guarantee the kernel offers. Under load the read
expires on a turn that did exactly the right thing.

The test already asserts the durable fact, twenty lines later:

```rust
assert!(metas.iter().any(|m| m.contains("title = \"Add the hex codes\"")),
        "the turn's meta.toml keeps the title: {metas:?}");
```

That is what the test is *for* — the turn it opened is labelled with the title — and it is recorded on
disk where nothing races it. So the honest repairs are the same two as always: keep the `meta.toml`
assertion as the verdict and demote the frame read to "if a title status is seen, its `source` is
`title`"; or make the emission ordered in the kernel and then assert the order. What must not stay is an
`.expect()` on catching a value mid-flight.

## The remedy, landed: [#553](https://github.com/unarbos/arbos/pull/553) on `main`

Merged 02:58 as `011ae1e9`, one file, `crates/arbos-kernel/tests/restart_states_e2e.rs`. It is the
worked example of the repair every member of this family needs, and its own comment states the rule more
plainly than the writeup did:

> A fixed 2 s pause was a bound on the wrong thing — root's restart turn took longer than that on a
> loaded runner (red on #542 and #433, 2026-09-18) — so the worker waits for the fact itself: a
> `turn_complete` on root's transcript after its `serve` wake. Bounded at 60 s.

Three things worth copying from it:

1. **It waits on the fact the assertion depends on**, not on a duration that happened to be long enough.
2. **The wait is bounded**, so a hang is still a failure rather than a hang.
3. **It was proved against a control**: with a 5 s model delay injected on root's restart line, the old
   pause fails and the new wait passes. A fix in this family without that step is a guess.

Two reds close with it, `#542` and `#433`. The same change, for the same reason, was made to this loop's
own `uw-*` probes hours earlier — `idle` replaced by the `turn_complete` event, because `idle` follows the
notes nudge and can trail the fact by seconds.

**If a red of this family still fires after #553, the name to use is**
`say_title_rename_e2e::a_queued_message_with_a_title_labels_the_turn_it_opens` — the one above, whose read
races the turn open and which #553 does not touch.

## The family's shape, stated once

Six members in one night, and they divide into two kinds:

- **Assertions that never proved their own setup**: `checkpoint_refs` (the ref exists only if a work
  commit was made), `history_for_an_archived_worker…` (panics on its archive wait).
- **Reads that race the thing they read**: `say_title_rename_e2e` line 173 (a live line superseded by the
  turn's first tool), and the `#149`/`#168`/`#170`/`#313` family before it.

Both kinds fail without anything they name being wrong, and both have the same cost: the steward has to
judge which reds mean anything. Every one of them also has a durable fact sitting nearby — a `meta.toml`,
a ref, a transcript — that could be the verdict instead.

## Cross-references

- `internal/kernel-ci-flakes-2026-09-18.md` — the iPhone loop's, which established the pattern and holds
  the same-commit-both-ways evidence.
- `docs/qa-loop-design.md`, the CI-flake family (#149, #168, #170, #313, #335) and the codebase-facts rule
  in `docs/project-context.md`: one `wait_for` per fact, on the thing the assertion reads.
- `internal/qa/bugs/qal-j26-…` — the same disease in this loop's own rig: an assertion on a wall-clock
  duration, on a machine that can be paused, reporting a 1528-second stall that never happened.
