# qal-j30 — the docs mirror dropped 168 feedback reports because only `docs/` and `internal/` are guarded

- **status**: fixed in `internal/mirror-docs.sh`; the guard's firing is **not yet proven by a control** (see below)
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

## The cause of the partial listing

The store mount returns `EAGAIN` ("Resource temporarily unavailable") under traversal. Measured
today from this machine:

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

**What is proven:** the edited script parses (`bash -n`), and run in `check` mode against the real
store it executes past the new block to reach the `STALE` exit, so the insertion does not break
the mirror's normal path.

**What is not proven:** that the guard *fires* on an empty feedback listing. The honest control
needs a store view whose `docs/` and `internal/` are complete and whose feedback directory is
empty, and building one means copying `internal/` off the mount — which is what wedged a `cp` in
uninterruptible sleep and was abandoned rather than hammer the mount while a cycle was starting.
Until that control runs, this fix is **argued, not demonstrated**, and should be read that way.

## Also fixed, and the reason this took twenty minutes to find

`deploy/mirror-timer.sh` recorded only the exit code:

```
[2026-09-18T04:19:24Z] mirror-docs exit 2 (a refusal is an alarm)
```

Every `die` in `mirror-docs.sh` logs `REFUSED: <reason>` naming the guard that tripped, and the
timer discarded it. Twelve refusals over three hours said nothing about why, and the only surviving
trace of the fault was the words `feedback 0 files` inside a commit message. The timer now keeps
the script's output and repeats the `REFUSED`/`STALE` line beside the code.

A guard nobody can read is a guard nobody trusts. That belongs with the review rule about who
reads a message when it changes — here the message existed and was thrown away one layer up.
