---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# A second reader of the store, from another machine — for QA

Set up 2026-09-17 06:18 UTC by the mesh worker at the coordinator's request.
Companion to `internal/store-docs-mirror.md` (the mirror, QA's) and
`docs/store-fault-report-2026-09-17.md` (the episodes).

## Why

The mirror judges the store from the machine it runs on. On 2026-09-17 one
client saw the store empty and unwritable while two others read and wrote it
whole; the mirror on one of those two could not know. Without a second view,
recorded with the time, that class of fault cannot be seen, and episodes
cannot be counted honestly.

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
   that turn. `AGREE` and `BEHIND` are silent.

## The three verdicts

| Verdict | Means | Action |
|---|---|---|
| `FAULT` | `docs/` or `notes.md` gone, nothing readable, a zero-byte or unreadable file, **or any file the mirror accepted is not here** | Look at the same minute from the mirror's machine. If it also lacks the files, the store lost them (service). If it has them, this client's view is broken (client) — the case the mirror alone could not see. Either way an episode to count. |
| `BEHIND` | Files or content here that the mirror has not taken yet | Normal between a write and the next mirror pass. Persisting for hours means the mirror is not running or is refusing; check its gate. |
| `AGREE` | Byte-identical in scope | Nothing. |

Tested 06:18 UTC: the live store read `BEHIND` (314 files here, 313 on the
mirror, 18 min old — three files written since the last pass); a scratch view
without `docs/` read `FAULT`, wrote the shout note, and recorded the line.

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
