# Project Agent Store: recurring selective file loss — report for the store's engineers

Store id: `bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983`
Mount on our machines: `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983`, type `fuse.agent-store` (`rw,nosuid,nodev,relatime,user_id=1000,group_id=1000,default_permissions,allow_other`)
Period: 2026-09-16 07:43 UTC to 2026-09-17 00:45 UTC. All times UTC.
Written by the QA loop, which keeps an off-store mirror of this store and restored it after each episode. Every statement below is from our own records (mirror commits, listings, timestamps); nothing is inferred about the service.

## Summary

Five times in seventeen hours, a set of files and directories vanished from this store while the rest of it stayed intact and writable. The set is not random: it is the same set each time, growing between episodes (every file lost in one episode is lost again in the next, plus more), and inside one directory it separates files by name pattern while leaving files written the same way by the same client untouched. Three independent clients saw the same picture at the same time, stable across repeated reads for minutes. We can restore from our mirror; we cannot see why the files go. The server-side journal for this store id over the windows below should show it.

## The five episodes

| # | Window (UTC) | How it was found | What was gone | Certainty |
|---|---|---|---|---|
| 1 | 09-16 **07:43:28 → 09:01** | A successful write at 07:43:28 is the last good moment; at 09:01 `docs/` and `artifacts/` were absent, and stayed absent across many reads by several agents over the following hour | `docs/` (19 files) and `artifacts/`, entirely | Loss (confirmed by the project owner) |
| 2 | 09-16 **~12:20 → 12:26** | Single reads at 12:23–12:25: store root listed as `internal/ media/ notes.md`; `internal/` at 15 entries; `docs/` absent; `internal/parity/`, `internal/features-inbox/`, several `internal/*.md` absent. `docs/` restored from the mirror at 12:26; `parity/` and `features-inbox/` were present again on the 12:42 mirror pass without anyone reporting a rewrite | as listed | Uncertain — single reads; possibly a partial view (see §Partial views) |
| 3 | 09-16 **14:20:32 → 14:23:24** | Mirror pass at 14:20:32 saw the store whole (22 docs, 377 files under `internal/`); the next pass at 14:23:24 found the mirror script itself absent; a full listing at 14:23:51–14:25:11 found 155 files missing; store root again `internal/ media/ notes.md`, `internal/` at 15 entries | 155 files: all 21 `docs/`, 99 of 184 `internal/qa/bugs/`, all `internal/parity/`, all `internal/features-inbox/`, 8 top-level `internal/*`, 4 `internal/qa/*` | Uncertain — one read per file; another client in the same minutes saw *unstable* counts (1092, 1122, 1092 files under `internal/` seconds apart), i.e. a partial view |
| 4 | 09-16 **22:37 → 22:52:16** | Mirror pass at 22:15:28 pushed a whole store (`0321076e`: 22 docs, 434 files under `internal/`); the pass at ~22:30–22:37 found no change; at 22:52:16 the mirror script was absent. Three clients on three machines listed the store independently within the same minutes and saw the same picture; this client read it six times over 22:54:21–22:56:31 with identical results | 191 files: all 22 `docs/`, all 23 `internal/features-inbox/`, all 15 `internal/parity/`, 11 top-level `internal/*`, 115 of 216 `internal/qa/bugs/`, 4 `internal/qa/*`, 1 `internal/mobile/*` | Loss |
| 5 | 09-17 **00:37:11 → 00:45:35** | Mirror pass at 00:37:11 pushed a whole store (`dff30aa7`); a write into `docs/` at 00:45:35 failed with "no such directory"; six reads over 00:45:41–00:47:44 identical | 215 files: all 22 `docs/`, 21 `internal/features-inbox/`, all 15 `internal/parity/`, 148 of 218 `internal/qa/bugs/`, 4 `internal/qa/*`, 4 top-level `internal/*`, 1 `internal/mobile/*` | Loss |

Between episodes, and throughout each one, the store answered reads and writes on the surviving paths normally: `notes.md` was written by another client at 22:54:31 during episode 4 and read back correctly.

## What is taken, and what never is

Taken in every episode since #2 (whole directories):
- `docs/` — the directory itself, not only its files (`ls docs` → no such file or directory)
- `internal/parity/` (incl. its subdirectory `fake-gh/`)
- `internal/features-inbox/`
- `internal/mirror-docs.sh` (a single file at the top of `internal/`)

Taken in episodes #3–#5 (files inside surviving directories):
- top-level `internal/*.md` of certain names: `mobile-coverage.md`, `mobile-feedback-log.md`, `mobile-findings.md`, `mobile-journey-history.jsonl`, `mobile-journey-runs.md`, `mobile-mac-host-and-testflight.md`, `mobile-store-loss-2026-09-16.md`, `desktop-feedback-hub-exploration.md`, `desktop-feedback-inventory.md`, `desktop-feedback-log.md`
- `internal/qa/arboslife-kickoff-history.jsonl`, `internal/qa/arboslife-spend.jsonl`, `internal/qa/arboslife-status.txt`, `internal/qa/batch_scenarios.py`
- `internal/mobile/testflight-feedback-loop-investigation.md`
- in `internal/qa/bugs/`: **every file whose name is ten hex characters plus `.md`** (`0017aeb03a.md`, `bd81b4e3cf.md`, …) — 99 of 99 such files at 14:23, 115 of 115 at 22:54, 147 of 147 at 00:45

Never taken, in any episode:
- `notes.md` at the store root; `media/` (including `media/desktop-feedback/`)
- `internal/qa/` itself and its tooling (`run.py`, `consistency.py`, `*_scenarios.py`, `deploy/`, `inbox/`, `scenarios/`) and the large `internal/qa/rollouts/` tree
- `internal/voice/`, `internal/ui-research/`
- in `internal/qa/bugs/`, **every human-named file** (`qa-001-….md`, `ui-013-….md`, `qal-j06-….md`, `store-….md`) — 70 files, 0 lost across five episodes, in the same directory as the 147 that were lost
- top-level `internal/*.md` of other names: `store-docs-loss-2026-09-16.md`, `store-docs-loss-2026-09-16-qa-record.md`, `store-docs-mirror.md`, `symmetry-findings.md`, `symmetry-prompts.md`, `secrets-inventory.md`, `qa-cycle-2026-09-16.md`, `release-v0.2.0-main-merge.md`, `ui-shell-map.md`, `website-deploy-2026-09-13.md`, `voice-endpoint.txt`

One fact about the bug-file directory that we can state because we control both writers: the hex-named files and the human-named `qal-*` files are written by the **same client process on the same machine with the same `cp` call** (a QA harness copying from local disk into the mount). The client-side write path does not distinguish them; the loss does.

## The monotone property

Comparing the exact missing lists of the three fully-listed episodes:
- every one of the 99 `internal/qa/bugs/` files missing at 14:23 was missing at 22:54 (99/99); 16 more were missing at 22:54;
- every one of the 115 missing at 22:54 was missing at 00:45 (115/115); 33 more were missing at 00:45;
- `docs/`, `internal/parity/`, `internal/features-inbox/` and `internal/mirror-docs.sh` were missing in every one of #2–#5.

The "more" each time are files created since the previous episode that match the same pattern. Nothing that survived one episode has been lost in a later one.

## Why we say service, not client

- Three clients (the coordinator's process, a features agent, the QA worker) on three machines saw the same missing set at the same time in episodes #4 and #5.
- On this client, six consecutive reads over two to two-and-a-half minutes returned byte-for-byte the same listings.
- Surviving paths answered reads and writes normally during the episodes.
- The client is a FUSE mount; a client-side cache fault would not be expected to agree across machines or to remove a directory (`docs/`) while keeping its siblings (`media/`, `notes.md`).

## Partial views — a second phenomenon, kept separate

On 09-16 at ~14:20 one client observed *unstable* counts seconds apart (1092, 1122, 1092 files under `internal/`), every sampled file present, and a clean pass minutes later. That is a listing that returns a subset for a while and then completes. It is why episodes #2 and #3 are marked uncertain: they were single reads and could have been partial views. Episodes #1, #4 and #5 were not: stable for minutes, across clients. If the service distinguishes "listing incomplete" from "object deleted", that distinction would tell us which of the two we saw in #2 and #3.

## What recovery cost

- An off-store mirror (a git branch, one commit per pass) now runs every 15 minutes; a pass walks `docs/`, `internal/` (minus bulk) and `media/desktop-feedback/` and takes 50 s to 4 min on this mount. Its safety gate refuses to push when the view is damaged; it has refused correctly in #3, #4 and #5.
- Each restore (`git archive` from the last good commit, then `cp` into the mount) took 7–8 minutes for ~200 files and left the store correct. Anything written into a lost path between the snapshot and the loss is gone: windows of 15 min (#4) and 8 min (#5). In #2 and #3, restores made before we understood partial views may have overwritten edits made after the snapshot; authors were told the windows.
- Two agents held all writes for ~15 minutes during #4; every agent that writes documents here has been told to re-read before concluding a file is lost and to restore only from the mirror. Roughly one agent's full attention for a day went to detection, restore and record-keeping.

## What we cannot see

The server-side journal for this store id: which objects were deleted, evicted, unlinked or hidden, by what actor or process, at 09-16 07:43–09:01, ~12:20, 14:20–14:23, 22:37–22:52, and 09-17 00:37–00:45 UTC. Our client sees only the result. Two observations that may narrow the search on that side: directory mtimes read through the mount are constant (`2026-09-12 22:04:36`, the store's creation) regardless of content changes; and `chmod` on a file in the mount is refused (`Operation not permitted`).

## Appendix — where the primary records are

- QA worker's episode record (all five, with the exact missing lists and restore steps): `internal/store-docs-loss-2026-09-16-qa-record.md` in this store
- Recovery worker's account of episode #1: `internal/store-docs-loss-2026-09-16.md`
- Mirror design, boundary and procedure: `internal/store-docs-mirror.md`
- Per-pass loss list: `internal/qa/store-mirror-losses.jsonl` (QA loop); mirror branch `store-docs` in the `unarbos/arbos` repository — commits `bdb3578f` (11:00), `8764ff48` (14:20), `0321076e` (22:15), `dff30aa7` (00:37) are the last-good snapshots before episodes #2–#5
