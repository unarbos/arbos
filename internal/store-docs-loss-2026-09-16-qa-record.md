---
cursor:
  subagentId: "bc-f2e2f30d-1298-59f1-a24c-55113322de28"
---
# QA worker's record for the `docs/` loss of 2026-09-16

Companion to `internal/store-docs-loss-2026-09-16.md` (the store-recovery worker's report; not edited here, it is theirs).

## What this worker saw

- **07:22:37 UTC** — `ls -la` of the store root listed `artifacts/ docs/ internal/ media/ notes.md`.
- **07:43:28 UTC** — this worker's last write into `docs/qa-loop-design.md` (a `StrReplace`, batch `43d8569d` section) succeeded; the file was 45,197 bytes by the recovery worker's count.
- **09:10 UTC** — a `StrReplace` on the same file failed with `FileNotFoundError`; `ls` of the store root showed `internal/ media/ notes.md` only; `stat docs` → no such file. `notes.md` still carried 17 links into `docs/`.
- Between those moments the worker ran only scenario code under `/tmp` and `~/arbos-qa`, and wrote under `internal/qa/` (`batch_scenarios.py`, `run.py`, `bugs/qa-039-*.md`, rollouts). No command named `docs/` or `artifacts/`.

## What was rebuilt

`docs/qa-loop-design.md`: rebuilt from `internal/qa/bugs/`, the scenario docstrings, `kickoff-history.jsonl`, the PRs and this worker's history; 27,442 bytes against the original's 45,197. The recovered 5,608-byte fragment (Phases, File-based agent model, Nightly SWE-bench) is kept and marked *evidenced*; everything else is marked *reconstructed* per section. Built in `/tmp` and copied in; `cmp` confirmed the store copy.

## Rule adopted

Every store file this worker writes from now on is built under `/tmp` and copied in (`cp`), never written whole in place: a whole-file write destroys the old content before the new one is durable.

## Second loss, 2026-09-16 ~12:20 UTC (same day)

- **12:10–12:17 UTC** — QA scenarios read `internal/parity/arbosdriver.py` successfully (the `xp-*` desktop probes ran from it).
- **12:23:40 UTC** — a scenario found `internal/parity/` gone; **12:24:52 UTC** — `ls` of the store root: `internal/ media/ notes.md` (no `docs/`); `internal/` down to 15 entries: `docs/`, `internal/mirror-docs.sh`, `internal/features-inbox/`, `internal/parity/`, `in-app-updates-and-signing.md`, `macos-worker-recipe.md`, `mobile-*.md`, `prime-intellect-harness-research.md`, `projects-post-gaps-2026-09-15.md` and others gone. `internal/qa/` intact (172 bug files, 344 rollouts), `notes.md` and `media/` present.
- The mirror branch `store-docs` had been pushed by the loop's 11:00 cycle (`bdb3578f`, 19 docs, 11:00:17 UTC): everything in `docs/` as of 11:00 was safe; nothing of QA's was written to `docs/` after 10:51.
- **12:26 UTC** — restored from the branch, via `/tmp`: all 19 `docs/*.md` (byte-identical to the branch) and `internal/mirror-docs.sh` (the alarm itself had been deleted). Then `mirror-docs.sh` pushed again so the branch carries the current `notes.md`.
- Not restorable from the mirror (out of its scope, other owners): `internal/parity/`, `internal/features-inbox/`, and the `internal/*.md` files above. The QA harness's desktop scenarios need `internal/parity/arbosdriver.py`; until it is back the loop uses the repo's copy at `desktop/driver/arbosdriver.py` (`ARBOS_QA_DRIVER_DIR`).
- Two losses in five hours, both hitting `docs/` and top-level `internal/` entries while `internal/qa/` (deep, busy) survived: the pattern points at the store service, not at any writer.

## Not a third loss: the 14:07–14:08 UTC "vanishing" was a deliberate move — corrected 14:25 UTC

**Correction.** The update worker moved its design out of `internal/` into `docs/kernel-self-update-design.md` on purpose and deleted the `internal/` copy in the same minute the 14:07 snapshot caught it. My restore at 14:16 recreated a duplicate its author had removed; deleted again at 14:24 UTC, `docs/kernel-self-update-design.md` (9,398 B) is the single copy. **Today's count is two store losses, not three.** The section below is kept as written, for the method and the timings; read "loss" there as "vanished from a snapshot", which is all the branch can ever show. Lesson kept: a file that vanishes between snapshots is *vanished, staged, not restored* until its author says which it was. The timer records it that way and does not put anything back.

### Original note, 14:16 UTC — checked on request, restored from the mirror (restore since reverted)

What was reported: `docs/kernel-self-update-design.md`, `artifacts/` and "several other documents" gone; `docs/acceptance-journeys.md` fine.

What the mirror shows (`store-docs` branch, one commit per pass; a commit is a full snapshot, so a file in the previous snapshot and not the next one vanished in that interval):

- `docs/kernel-self-update-design.md` **was never in any mirror snapshot** — the branch had 19 docs at 12:53, 20 at 13:43 (the 20th is `acceptance-journeys.md`), 20 at 13:50, 14:03, 14:07. Its author says in the rewrite that it "is no longer in the store"; it lived and died between two passes, so the branch cannot restore it. The author's rewrite covers it.
- `internal/kernel-self-update-design.md` (the rewrite, author `bc-37bdb830…`, 7,189 bytes) **was captured at 14:07:16Z** (commit `7f5b88f4`, `internal/` 376 → 377 files) and **was gone from disk by 14:08:39Z**, when this worker listed `internal/` — a window of about 80 seconds after the snapshot. Restored byte-for-byte from `7f5b88f4` at 14:16Z (`cmp` clean). No other file present at the 14:07 head was missing from disk.
- `artifacts/` is outside the mirror's scope (docs/, notes.md, internal/ minus bulk), so nothing can be said about it from the branch. Its owner should say what was there.

The gate did **not** refuse on the 14:18 pass (`bde7dd63`, 21 docs, `internal/` 377): one file of 377 is under the 10 % shrink threshold. That gate catches wholesale loss, not a single file — which is what this loss was. Fixed the same hour: `internal/qa/deploy/mirror-timer.sh` diffs each pass's previous head against the new one (`git diff --diff-filter=D`) and records every vanished file in `internal/qa/store-mirror-losses.jsonl`, with a copy staged under `~/arbos-qa/state/mirror-restore/<ts>/`. Not restored automatically — a deletion may be an author's choice; the list is for a human or the owning agent.

Cadence: three losses in seven hours against a mirror that ran at cycle start and end (roughly hourly, with the cycle's own length in between) means an exposure of up to an hour per loss. A pass costs 50 s to 4 min on this mount (the 14:14 pass took 225 s). Since 14:20Z the mirror runs on its own timer every 15 minutes (`tmux` session `store-mirror`, log `~/arbos-qa/logs/store-mirror-timer.log`), independent of the QA cycle, which still mirrors at its start and end. Exposure is now ≤ 15 min plus the walk. Tighter than that would keep the mount busy a third of the time; if losses continue, the next step is not a faster timer but the store owner's fix.

Window: **14:07:16Z (captured) → 14:08:39Z (gone)**. The author has since said: moved, not dropped (see the correction above).

## Third loss (the real one), 2026-09-16 14:20:32Z → 14:23:24Z — caught by the timer's first pass, restored from the mirror

Window: the 14:20:32Z pass pushed `8764ff48` (21 docs, `internal/` 377 files, store healthy). At 14:23:24Z the new timer's first pass found `internal/mirror-docs.sh` gone and raised the alarm (exit 3, `mirror-alarm.py` staged a copy under `~/arbos-qa/state/store-docs-restore-20260916T142326Z`, drafted `bugs/store-docs-mirror-refused.md`). At 14:23:51Z this worker listed the store: `docs/` **absent entirely**, `internal/` 25 → 15 entries.

Missing against `8764ff48`, exactly: **155 files** — `docs/` all 21; `internal/qa/bugs/` 99 of 184; `internal/parity/` all 14 (+ `fake-gh`); `internal/features-inbox/` all 8; 8 files at `internal/` top level (`mirror-docs.sh`, `mobile-*.md`, …); 4 in `internal/qa/` (`batch_scenarios.py`, histories). Untouched: `notes.md`, `media/`, `internal/qa/rollouts/`, the other 85 bug files, `internal/voice/`, `internal/ui-research/`.

Restored 14:25–14:33Z from `8764ff48` with `git archive` → `/tmp/restore3` → `cp` into the store (a full pass over 156 files took eight minutes on this mount). Every file on the missing list is back; nothing still missing. `internal/kernel-self-update-design.md` came back with them because it was in the snapshot — deleted again, `docs/kernel-self-update-design.md` stays the single copy.

Two facts for the store owner: the loss is **selective, not a directory wipe** — 99 of 184 bug files went and 85 stayed, all in one folder; and the `mirror-docs.sh` the alarm depends on was among the lost, which is why the alarm path that keys off the script's absence matters. Day's count: **three losses** (07:43–09:01, ~12:20, 14:20–14:23), the 14:07 event having been a move.

## Reassessment, 14:45 UTC: one certain loss, two uncertain — probably partial views, not deletions

The update worker hit the mirror's refusal and, before forcing it, read the store three times seconds apart: 1092, 1122, 1092 files in `internal/`; the gate had seen 296 against 379; every file it sampled was present and non-empty; a retry minutes later passed. **A healthy store can answer partially.** Re-reading today's episodes with that in mind:

| Episode | Reads that saw it gone | Verdict |
|---|---|---|
| 1. `docs/` + `artifacts/`, 07:43–09:01 | many, over more than an hour; confirmed by Jacob | **loss** |
| 2. ~12:20 — root read as `internal/ media/ notes.md`, `internal/` 15 entries | single reads, 12:23–12:25; by 12:42 `internal/parity/` (14 files) and `features-inbox/` were back on the branch without anyone reporting a rewrite | **uncertain, leaning partial view** |
| 3. 14:20–14:23 — root read as `internal/ media/ notes.md`, `internal/` 15 entries, 155 files "missing" | one read per file (timer alarm 14:23:24, this worker's listing 14:23:51 and the 80 s per-file loop); "selective" pattern — 85 of 184 bug files present — is what a partial listing looks like; the update worker's unstable counts fall in the same minutes | **uncertain, leaning partial view** |

Episodes 2 and 3 gave the **same picture** — root `internal/ media/ notes.md`, `internal/` at exactly 15 entries — which reads as one failure mode of the store's view, not two deletions that happened to leave the same subset.

**What my restores may have cost.** If 2 and 3 were partial views, the files were there and my `cp` from the snapshot wrote older content over them. Any edit made to those files inside these windows is gone, and nothing I have can detect it: (a) `docs/*.md` (19 files), edits between **11:00:17Z** (snapshot `bdb3578f`) and **12:26Z**; (b) the 155 files listed above — all of `docs/`, 99 bug files, `internal/parity/`, `internal/features-inbox/`, eight `internal/*.md`, four `internal/qa/` files — edits between **14:20:32Z** (`8764ff48`) and **14:25–14:33Z**. Owners who wrote into those paths in those windows should re-check their text. This is the concrete cost of restoring by default, and the reason the procedure below changes.

**Procedure from now on, for this worker and for the timer (`mirror-timer.sh`):**
1. Before concluding a file is lost, **re-read it three times, seconds apart**; a file seen in any read is not lost. Unstable counts between reads mean the store is answering partially — and the honest response to a partial answer is to **wait and re-read, and conclude nothing**. Nothing is at risk from waiting: the gate already refuses to push a partial view over the last good snapshot, so the snapshot holds while the store settles.
2. A file absent across all reads is recorded as *vanished, staged, not restored*; the author says whether it was a move, a delete, or a loss.
3. Restore only when the owner asks, or when the absence has held across reads spread over many minutes (episode 1's shape). **Always name the snapshot and the window** (`<commit>`, `<from>Z → <to>Z`): that is the only thing that made today's possible cost knowable — the two owners with writes inside the 14:20 window could be told exactly what to re-check.
4. A gate refusal is the mirror working: it protects the snapshot from a partial view. Never force it.

Day's count as this worker can honestly state it: **one loss, two uncertain**. The two "uncertain" rows stay uncertain unless the store's own logs say otherwise.

## Fourth episode, 2026-09-16 22:37Z → 22:52:16Z — a loss; restored 22:58–23:05Z from `0321076e` (22:15:28Z)

**Seen by three clients.** The features agent (`docs/` gone, `features-inbox/`, `mirror-docs.sh`, `mobile-*`, `desktop-feedback-*`, `parity/` missing), the coordinator (`docs/project-context.md` not found, `notes.md` fine), and this worker: six reads over 22:54:21–22:56:31Z, seconds then minutes apart, identical — `docs/` 0, `internal/mirror-docs.sh` absent, `features-inbox/` and `parity/` empty, 101 of 216 bug files present, `notes.md` intact (40,900 B, written 22:54:31Z by a worker). Against `0321076e`: **191 files missing** — all 22 `docs/`, all 23 `internal/features-inbox/`, all 14 `internal/parity/` + `fake-gh`, 11 top-level `internal/` files, 115 bug files, 4 `internal/qa/`, 1 `internal/mobile/`.

**Window.** Last good push `0321076e` at 22:15:28Z. The timer's pass at ~22:30–22:37Z pushed nothing and raised nothing — no change and no damage — so the store was whole at ~22:37Z. At 22:52:16Z the timer found `mirror-docs.sh` gone and alarmed. **22:37Z → 22:52:16Z.**

**Verdict: loss**, by the procedure above — absent across reads spread over minutes, from more than one client. Not a partial view. The coordinator made the restore call at 22:58Z ("waiting only widens the gap in which agents are working blind").

**Restore.** Tool first: `internal/mirror-docs.sh` from `0321076e`; then the 191 files (`git archive` → `~/arbos-qa/state/mirror-restore/20260916T2254Z-episode4/` → `cp` into the store), 22:58:27Z → 23:05:22Z, nothing still missing afterwards. Mirror pass green at 23:09:07Z: `10b0c618` (22 docs, 594K, `internal/` 435 files); the timer's own pass pushed the identical tree a second earlier (`c4927443`). The diff `0321076e → 10b0c618` is 8 files, additions only: five are edits to files that were never gone (`notes.md`, `internal/qa/landing_scenarios.py`, `symmetry-*`, a new inbox note); three are restored files that their owners had already appended to in the four minutes between the restore and the pass — `docs/swebench-loop.md` (+27), `internal/mobile-journey-history.jsonl` (+2), `internal/mobile-journey-runs.md` (+2) — so the SWE-bench and phone loops were writing again the moment the paths came back. Nothing restored came back different from the snapshot.

**For the owners — paths that came back from the 22:15:28Z snapshot.** Anyone who wrote into one of these between 22:15:28Z and 22:52:16Z should re-check their own text rather than assume: everything under `docs/` (22 files, incl. `project-context.md`, `acceptance-journeys.md`, `qa-loop-design.md`, `kernel-self-update-design.md`); everything under `internal/features-inbox/` (23) and `internal/parity/` (15); `internal/mirror-docs.sh`, `internal/mobile-coverage.md`, `mobile-feedback-log.md`, `mobile-findings.md`, `mobile-journey-history.jsonl`, `mobile-journey-runs.md`, `mobile-mac-host-and-testflight.md`, `mobile-store-loss-2026-09-16.md`, `desktop-feedback-hub-exploration.md`, `desktop-feedback-inventory.md`, `desktop-feedback-log.md`; `internal/mobile/` (1); 115 auto-drafted bug files under `internal/qa/bugs/` (the hash-named ones; the `qa-*`/`ui-*`/`qal-*` files were never gone); 4 files under `internal/qa/`. The features agent's last push was 22:05Z, inside the snapshot.

**The most useful fact about this fault, for whatever goes to Cursor:** *four episodes, and the same subset each time.* The 99 bug files missing at 14:23Z are exactly the 99 missing at 22:54Z, plus every file created since; the 12:24Z and 14:23Z listings both read the store root as `internal/ media/ notes.md` with `internal/` at 15 entries; `notes.md`, `media/`, `internal/qa/` tooling and rollouts, `internal/voice/`, `internal/ui-research/` and the human-named bug files have survived every episode. Whatever selects what vanishes, it is deterministic, not random — a fixed subset of the tree, the same at 12:24, 14:23 and 22:54 (07:43 took `docs/` and `artifacts/` whole and predates the fine-grained records). That is the shape to hand the store's owner.
