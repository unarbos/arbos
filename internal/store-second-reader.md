---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# A second reader of the store, from another machine — for QA

Set up 2026-09-17 06:18 UTC by the mesh worker at the coordinator's request.
Companion to `internal/store-docs-mirror.md` (the mirror, QA's) and
`docs/store-fault-report-2026-09-17.md` (the episodes).

## What it is for — restated 2026-09-17 08:10 UTC, after its first catch

**A guard against our own tools, not only against the service.** That is the
more useful of its two jobs, and the one nothing else does.

It was built for the second job: the mirror judges the store from the machine
it runs on, and on 2026-09-17 one client saw the store empty while two others
saw it whole, so a view recorded from another machine was needed to see that
class at all. An hour after it started, at 07:07 UTC, it read `FAULT` — 237
files the mirror had accepted were gone from `internal/qa/bugs/`. That was
true, and it was not the service. The QA loop's attack list held
`cd / && rm -rf *` marked "not caught, decide whether it should be"; its test
agent ran it, seven times over two days; as a normal user it failed on system
directories and succeeded on the first writable tree, which was this store.
The loop's own file-operation log shows delete bursts from that client at
exactly the seven episode times. The restores that followed wrote older mirror
copies over other workers' newer files — the "older version being served"
signature. The loop that was deleting was also the loop reporting all clear,
because the mirror only judges what its own client sees.

So: a reader on a machine that runs none of the loops, comparing against a
floor the loops cannot lower, is the check that catches *us*. Whatever the
store's engineers do about the service (see the last section), this stays.

## What it does, every 30 minutes (at :07 and :37)

From the mesh worker's cloud VM — a different client, a different machine from
the QA loop's — `store-second-reader.sh`:

1. **Reads** the paths the episodes took, plus the two never taken as
   controls: `docs/`, `internal/features-inbox/`, `internal/parity/`,
   `internal/qa/bugs/`, `internal/mirror-docs.sh`, `notes.md`. Every file is
   hashed (`git hash-object`), so content is compared, not names.
2. **Compares** with what the mirror's client last accepted: the tip of the
   `store-docs` branch, which only moves when the mirror's safety gate passed,
   so it is a floor for what should be in the store.
3. **Records** one JSON line on the orphan branch `store-watch`, file
   `readers/cloud-mesh-3b98.jsonl`: time, verdict, reason, file counts on
   both sides, mirror tip and its age, whether `docs/` and `notes.md` were
   there, and the first 25 missing paths. The script itself rides on that
   branch root, because the store copy under `internal/` is taken in every
   episode (as `mirror-docs.sh` is).
4. **Shouts** on `FAULT`: writes
   `internal/qa/inbox/<date>-store-second-reader-fault-<hhmm>.md` (if the
   store will take it) and the mesh worker reports it to the coordinator in
   that turn. `AGREE` and `BEHIND` are silent — and silent means the timer's
   turn ends with zero characters, not "nothing to report". Only `FAULT`, or
   the reader itself failing to run, earns words. A line every half hour
   saying nothing is wrong is how a real fault gets read past.

## The three verdicts

| Verdict | Means | Action |
|---|---|---|
| `FAULT` | `docs/` or `notes.md` gone, nothing readable, a zero-byte or unreadable file, **or any file the mirror accepted is not here** | First ask **which of our clients wrote or deleted in the store in the minutes before** — on 2026-09-17 every such fault was one of our own loops. Then look at the same minute from the mirror's machine: if it also lacks the files and no client deleted them, the store lost them (service); if it has them, this client's view is broken (client). Every case is an episode to count. |
| `BEHIND` | Files or content here that the mirror has not taken yet | Normal between a write and the next mirror pass. Persisting for hours means the mirror is not running or is refusing; check its gate. |
| `AGREE` | Byte-identical in scope | Nothing. |

Tested 06:18 UTC: the live store read `BEHIND` (314 files here, 313 on the
mirror, 18 min old — three files written since the last pass); a scratch view
without `docs/` read `FAULT`, wrote the shout note, and recorded the line.

**First real catch, 07:07:26 UTC:** `FAULT — 237 file(s) the mirror accepted
are not here` (86 here, 323 on tip `e8289af5`), all `internal/qa/bugs/<hex>.md`;
shout at `internal/qa/inbox/2026-09-17-store-second-reader-fault-0707.md`.
Cause: the QA loop's own `rm -rf` (above). By 07:37 the store read `BEHIND`
again after their restore. The record line is on `store-watch`.

## What remains the service's

Two things from the fault report survive as genuinely the store's, and go to
its engineers as a small note in place of the large wrong one: the 502s from
the S3-backed service, and the one client that saw an empty store for about
twenty minutes with no deletion from anyone. This reader will show the second
kind as a `FAULT` on one machine while the mirror's client and the record from
any third reader stay whole at the same minute — which is exactly how to tell
it from our own deletions.

## How to read the record

```bash
git -C /workspace fetch -q origin "+refs/heads/store-watch:refs/remotes/origin/store-watch"
git -C /workspace show origin/store-watch:readers/cloud-mesh-3b98.jsonl | tail -20
# or: bash internal/store-second-reader.sh show
```

To count episodes: `grep -c '"verdict":"FAULT"'` on that file; to line them
up with the mirror, compare `ts` with the mirror branch's commit times
(`git log --format='%cI %s' origin/store-docs`).

## To run it from a third machine

Any client with the store mounted and a checkout with push rights to
`unarbos/arbos`:

```bash
git -C /workspace fetch -q origin store-watch
git -C /workspace show origin/store-watch:store-second-reader.sh > /tmp/store-second-reader.sh
CLIENT=<a-short-name-for-this-machine> bash /tmp/store-second-reader.sh
```

Each client writes its own `readers/<CLIENT>.jsonl`; three readers give three
independent views per half hour. The QA loop is welcome to run it as a cycle
step under its own `CLIENT`; the mesh worker's timer keeps the second view
going meanwhile and stops when that worker is archived, so QA should adopt it
before then.

## Limits, stated

- It runs on the mesh worker's timer, so its life is that worker's. Adoption
  by the QA loop is the durable home.
- The mirror branch is the reference, not a live second client. A file the
  mirror never accepted (written and lost between two passes) is invisible to
  both; the shorter the passes, the smaller that window.
- A deliberate deletion (`MIRROR_ALLOW_SHRINK=1`) reads as `FAULT` once, until
  the mirror moves. That is the right side to err on.
