# qal-j30 — the docs mirror dropped 168 feedback reports because only `docs/` and `internal/` are guarded

- **status**: fixed in `internal/mirror-docs.sh`, both arms of the control run (08:13–08:18)
- **found**: 2026-09-18 07:40, reading `store-docs` history at a cycle boundary
- **commit that lost them**: `68801ff444a114f2d2e2043b52192e4aaad86419`, pushed 04:52Z
- **restored by**: the next successful pass at 06:03Z

## What happened

`mirror-docs.sh` mirrors the store to the `store-docs` branch. It counts and stages four things:
`docs/`, `notes.md`, `internal/`, and `media/desktop-feedback`. It refuses to push a view that
looks partial — but only for two of them:

| what | shrink guard |
|---|---|
| `docs/` | yes (line 96) |
| `internal/` | yes, two rules (lines 191–205) |
| `media/desktop-feedback` | **none** |

At 04:52 the store listed `media/desktop-feedback` as empty for a single pass. With no guard, the
mirror committed that view:

| commit | feedback entries in the tree |
|---|---|
| parent of `68801ff4` | 168 |
| **`68801ff4` (04:52)** | **0** |
| tip now | 168 |

The commit message for `68801ff4` says `feedback 0 files` in plain text, next to `internal/ 692
files`, and nothing objected. For about seventy minutes the mirror held none of the feedback
reports.

## Why this matters more than an hour of absence

The mirror's own header says why it exists: *"on 2026-09-16 the store dropped `docs/` and
`artifacts/` with no event and no undo. The only documents that came back whole were the four
that happened to be mirrored."* A store fault during that seventy-minute window would have found
the safety net holding zero of the 168 reports.

The `internal/` guard carries a comment dated 2026-09-17 describing the same event — a pass
recording 454 files where the tip held 492 — and was hardened then. Feedback was staged by the
same script, on the same pass, and was not.

## The cause of the partial listing, caught in the act

At **08:03:59Z**, running `mirror-docs.sh check` against the real store, the mirror printed:

```
REFUSED: docs/ lists no .md files. The store view looks broken. Not touching the mirror.
```

`docs/` holds 29 files and listed **zero**. Twenty samples taken immediately afterwards all
returned 29. So the store presents an empty directory listing transiently, the guard catches it,
and the refusal is correct — the same event as 04:52, on a directory that happened to be guarded.

That is the whole mechanism, observed rather than inferred: a transient empty listing on a
**guarded** directory becomes a refusal, and on an **unguarded** one becomes a commit.

The store mount also returns `EAGAIN` ("Resource temporarily unavailable") under traversal.
Measured today from this machine:

- writing a 512-byte file to the store: **3 of 100 attempts** returned `EAGAIN`
- two of my own edits to store files failed with `EAGAIN` and succeeded on retry
- a recursive read of `internal/` produced `EAGAIN` on 11 paths before I stopped it, and left a
  `cp` wedged in uninterruptible sleep
- J6 raised `BlockingIOError` — the same `EAGAIN` — reading a file off the mount, and the second
  reader's unexplained zero-byte FAULTs are the same family

So a `find` across a store directory can return short, and every consumer that counts files needs
to treat a sudden fall as a fault rather than as news. That is what the guards do, and it is why
the twelve `mirror-docs exit 1`/`exit 2` refusals between 04:02 and 06:49 were **correct** — the
safety net working, not failing.

## The fix

The `internal/` guard's shape, applied to feedback, in `internal/mirror-docs.sh`:

```bash
if [ -n "$parent" ]; then
    n_feedback_last="$(git ls-tree -r --name-only "$parent" media/desktop-feedback/ 2>/dev/null | wc -l || true)"
    if [ "$n_feedback_last" -gt 0 ] && [ "$n_feedback" -lt "$n_feedback_last" ]; then
        if [ -z "${MIRROR_ALLOW_SHRINK:-}" ]; then
            die "media/desktop-feedback lists $n_feedback mirrorable files but the mirror holds $n_feedback_last. …"
        fi
        shrink_note "feedback" "$n_feedback_last" "$n_feedback"
    fi
fi
```

### The control (run 08:13–08:18, both arms)

Staging an empty feedback directory needs a store view whose `docs/` and `internal/` are otherwise
complete, and copying `internal/` off the mount is what wedged a `cp` in uninterruptible sleep.
The cheap way round is to move the *other* side of the comparison: `BRANCH` is overridable and the
parent is read from `refs/remotes/origin/$BRANCH`, which can be set locally with `git update-ref`
and never pushed. So the control branch is the `store-docs` tip plus one extra feedback entry —
the parent claims **169** where the store lists **168**.

| arm | result |
|---|---|
| with the guard (installed script) | `REFUSED: media/desktop-feedback lists 168 mirrorable files but the mirror holds 169` — exit 2 |
| same script, guard removed | `STALE: the store differs from the mirror (29 docs)` — exit 1, straight past the shortfall and on toward a push |

So the fix refuses where the unfixed script proceeds. The control ref was deleted afterwards and
nothing was pushed.

### What the control turned up on its own

The unguarded arm had to be run four times to get that clean result. Three of the runs died like
this:

```
FAILED at line 180: [sort > "$list"] exited 1. This is a crash, not a refusal; a refusal says REFUSED.
```

Line 180 is the feedback listing itself — `find "$fb" … -print 2>/dev/null | sort > "$list"` —
identical in both arms. Under `pipefail` a failing `find` fails the pipeline, and `set -e` then
kills the mirror with exit 1 and, before the trap, no word at all.

**That is the six silent exit-1 crashes, reproduced and located**, and it completes the picture:
one flaky traversal of the store gives two different failures. When `find` fails outright the
mirror crashes silently; when it merely returns short the mirror undercounts, and on an unguarded
directory that undercount becomes a commit. 04:52 was the second kind.

## Two more fixes: the refusals were unreadable, and half of them were not refusals

`deploy/mirror-timer.sh` recorded only the exit code:

```
[2026-09-18T04:19:24Z] mirror-docs exit 2 (a refusal is an alarm)
```

Every `die` logs `REFUSED: <reason>` naming the guard that tripped, and the timer discarded it.
The timer now keeps the script's output and repeats the `REFUSED`/`STALE` line beside the code.

With that in place the next pass printed `mirror-docs exit 1 — ` with **nothing after the dash**,
which is the more serious finding. `mirror-docs.sh` sets `set -euo pipefail`, and `exit` does not
raise `ERR`, so:

- **exit 2** is `die` — a deliberate refusal, with a reason.
- **exit 1** is the script **crashing** on an unhandled command failure, silently.

Six passes today exited 1. Those were not the safety net refusing; they were the safety net
falling over without a word, and the timer's label — "a refusal is an alarm" — said the
comforting one of the two. An `ERR` trap now names them:

```bash
trap 'rc=$?; log "FAILED at line $LINENO: [$BASH_COMMAND] exited $rc. This is a crash, not a refusal; a refusal says REFUSED."; exit $rc' ERR
```

**Control, run before installing:** a forced failure prints
`FAILED at line 5: [grep -q nothing /nonexistent-file-xyz] exited 2`, and `die "a staged reason"`
still prints `REFUSED: a staged reason` without firing the trap. Both checked against the exact
line as it appears in the file, after a first attempt whose quoting mangled the message — the
retyped approximation passed while the real line emitted a stray quote, which is its own small
lesson about testing the artifact rather than a copy of it.

A guard nobody can read is a guard nobody trusts, and a crash wearing a refusal's label is worse
than either.
