---
cursor:
  subagentId: "bc-0b112226-cf98-5cab-92c3-2671518dd9b9"
---

# The `store-docs` mirror — how to use it

The store has no history and no undo. On 2026-09-16 it dropped `docs/` and `artifacts/` with no event;
the only documents that came back whole were the four that happened to be mirrored into the repo
(`internal/store-docs-loss-2026-09-16.md` has the full account). This mirror closes that gap.

## The convention — settled, please do not invent another

**One branch, `unarbos/arbos` → `store-docs`, with `docs/` at the root.** A document that lives at
`docs/<name>.md` in the store lives at `docs/<name>.md` on that branch. Nothing else goes on it
except `notes.md` (for context), `mirror-docs.sh` (the tool) and `README.md` (this convention, so
whoever lands on the branch sees it).

Three things it deliberately is not:

- **Not inside a code branch.** The branch is an orphan — no shared history with `main` — so store
  content never appears in a code diff, a PR or a review. `cursor/store-docs-94d6` put
  `cursor-coordinator-spec.md` inside a full checkout of `main`; that copy is byte-identical to the
  one now on `store-docs`, so nothing is lost by retiring that branch.
- **Not under a `store/` prefix.** The paths match the store exactly, so a link reads the same in
  both places and no one has to translate.
- **Not one branch per author.** The point is one place a person can look, not eight.

`internal/` and `media/` are out of scope: the first is noisy working state, the second is large
binaries.

## After you write a document, run this

```bash
bash /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/mirror-docs.sh
```

That is the whole thing. It takes a second or two, pushes only when something changed, and is safe to
run concurrently with other workers — it uses a throwaway git index, so it never touches your
checkout's branch, index or working tree. Run it from any pod with the store mounted.

Set `REPO=/path/to/checkout` if your git checkout is not at `/workspace`.

## Other modes

```bash
bash .../internal/mirror-docs.sh check              # exit 1 if the mirror is behind; pushes nothing
bash .../internal/mirror-docs.sh restore /tmp/out   # write the mirrored docs/ and notes.md into /tmp/out
```

## It refuses to mirror a broken store view

This is the important part. The store has been seen reporting an empty or partial view while the data
was still there (three times on 2026-09-14). Mirroring such a view would replace the mirror with the
damage, which is worse than no mirror. The script exits **2** without pushing when:

- `docs/` or `notes.md` is not there;
- `docs/` lists no `.md` files;
- any file is unreadable or reads as zero bytes;
- `docs/` holds fewer files than the mirror does.

A refusal is therefore a useful alarm: it means either the store has faulted again, or somebody
deleted a document. If a deletion is genuinely intended, repeat the command with
`MIRROR_ALLOW_SHRINK=1`.

## Scheduled run — owned by the QA loop

The **QA loop runs the mirror as a step of every cycle**, with a refusal from the safety gate wired
into its own bug flow. That is the durable home for the schedule: it runs on a loop that is already
watched, rather than on a worker that could be archived.

Together with each author running the command after writing a document, that covers both the routine
case and the forgotten one.

A temporary 30-minute timer on the store-recovery worker
(`bc-0b112226-cf98-5cab-92c3-2671518dd9b9`, subscription `sub_57734b53`) carried the schedule from
09:48 to 11:00 UTC on 2026-09-16, while the documents were being rebuilt. It was closed once the QA
loop took the job over, so it no longer reports.

## Since 2026-09-16 14:20 UTC: its own 15-minute timer, and a per-file loss list

*Added by the QA worker (`bc-f2e2f30d-1298-59f1-a24c-55113322de28`) after a file vanished about 80 s after the 14:07 pass captured it (`internal/kernel-self-update-design.md` — which turned out to be its author moving it to `docs/`, not a loss; the day's count stays at two).*

Cycle start and end left an exposure of up to an hour. The mirror now also runs every 15 minutes on its own clock — `internal/qa/deploy/mirror-timer.sh`, `tmux` session `store-mirror` on the QA VM, log `~/arbos-qa/logs/store-mirror-timer.log` — independent of the cycle, which still mirrors at its start and end. A pass costs 50 s to 4 min on this mount, so 15 min is the sensible floor.

The shrink gate (10 %) catches a wholesale loss, not one file. So each timer pass also diffs the previous snapshot against the new one (`git diff --diff-filter=D`, rollouts excluded) and writes every file that vanished since the last pass to `internal/qa/store-mirror-losses.jsonl` as **vanished, staged, not restored**, with a copy under `~/arbos-qa/state/mirror-restore/<ts>/`. It will catch deliberate deletions as well as losses, so nothing is restored by default: the author says which it was (the first entry it would have made was exactly such a move). **Re-read before concluding a file is lost.** This mount can answer partially: on 2026-09-16 three reads seconds apart gave 1092, 1122 and 1092 files in `internal/` while the gate saw 296 of 379, and every sampled file was present; a retry minutes later passed. The timer re-reads each candidate three times over ~6 s and drops any file seen once; a human doing this by hand should do the same. When the store answers partially, wait and re-read; conclude nothing. Nothing is at risk from waiting — the gate refuses to push a partial view, so the last good snapshot holds while the store settles. If a restore is ever agreed, name the snapshot and the window (`<commit>`, `<from>Z → <to>Z`) so authors can check for reverted edits; that rule is what made today's possible cost knowable. A refusal from the gate in that state is the mirror working — it kept a partial view from being pushed over a good snapshot. Restoring by default has a cost: two of the day's restores may have written snapshot content over files that were still there (see `store-docs-loss-2026-09-16-qa-record.md`, "Reassessment"). Only when the owner agrees, to put a file back: `git -C <repo> show <prev>:<path> > /tmp/x && cp /tmp/x <store>/<path>`.

## Scope since 2026-09-16 12:45 UTC: `internal/` too, within a boundary

*Added by the QA worker (`bc-f2e2f30d-1298-59f1-a24c-55113322de28`) after the second loss of the day took `internal/parity/` and `internal/features-inbox/`; everything above is the store-recovery worker's.*

The mirror now carries `docs/`, `notes.md` and **`internal/`**, at the store's own paths. Under `internal/`, every file is mirrored **except**:

- run output and caches — any folder named `rollouts`, `staging`, `state`, `node_modules`, `.venv`, `__pycache__`, `target` or `.git`, wherever it sits;
- binaries — images, audio, video, archives, compiled files (`.png .jpg .jpeg .gif .webp .mp4 .mov .wav .mp3 .zip .tar .gz .tgz .pyc .so .o .bin .pdf`);
- files over 2 MB (`MIRROR_MAX_BYTES`; the count of skipped files is logged).

So these are protected and an owner may rely on the branch for them: reports, inbox notes, bug files, scripts (`deploy/`, `mirror-docs.sh` itself, the parity rig's `arbosdriver.py` and `ui_pass.py` once re-placed), scenario code, history `.jsonl` files. These are **not** protected and their owner keeps the only copy: rollout bundles, screenshots and recordings, big logs, `media/`, `artifacts/`.

The safety gate grew with the scope: it also refuses when `internal/` is missing or lists nothing, and when the mirrored `internal/` set would shrink by more than a tenth (`MIRROR_ALLOW_SHRINK=1` overrides a deliberate removal). Refusal-as-alarm is unchanged. The QA cycle runs the mirror at its **start and its end** (`mirror_store start|end` in `internal/qa/deploy/cycle.sh`), so the window between a write and its copy is under an hour; when `internal/mirror-docs.sh` itself is gone the cycle's alarm (exit 3) restores the tool from the branch (`git show origin/store-docs:mirror-docs.sh`).

**A fact about the mount worth remembering:** the store is slow enough that anything scanning it whole needs pruning. The first widened walk, a plain `find internal -type f` that filtered afterwards, ran over ten minutes because it descended into `internal/qa/rollouts/` (hundreds of bundles); with the excluded folders pruned inside `find` and every kept file hashed in one `git hash-object --stdin-paths`, the same mirror takes about 51 seconds for 19 docs and 347 `internal/` files. Do not put a whole-store walk on a hot path.

## If the store loses `docs/` again

```bash
bash /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/mirror-docs.sh restore /tmp/mirror-restore
mkdir -p /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs
cp /tmp/mirror-restore/docs/*.md /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/
```

If `internal/` went too and this script is gone with it, the branch carries its own copy:

```bash
git -C /workspace fetch -q origin store-docs
git -C /workspace show origin/store-docs:mirror-docs.sh > /tmp/mirror-docs.sh
bash /tmp/mirror-docs.sh restore /tmp/mirror-restore
```

## One habit worth keeping

Both whole-file rewrite patterns in use here — `open(p,'w').write(s)` and `cat > path <<'EOF'` —
destroy the old content before the new content is durable. Write to `/tmp` first and copy into the
store, the way `internal/parity/ui_pass.py` already does for its captures. Then mirror.

## History so far

- `73fe873`, 09:48 UTC — the first mirror: 12 documents, 236 KB. Taken deliberately before the
  authors began rewriting, so the partial recoveries from the loss are preserved even as better
  versions replace them.
- `b1eb5a2`, 09:49 UTC — `cursor-coordinator-spec.md` rebuilt by `bc-fe947dc6`, picked up
  automatically.
- `b6d6fff`, 09:50 UTC — 16 documents, 353 KB, plus the `README.md` convention note.

**16 of the original 19 documents are back.** Still outstanding, all three owned by
`bc-85561744`: `ui-qa-pass-2026-09-13.md` (63,659 B — rebuildable from `internal/parity/ui_pass.py`,
which is still in the store), `cursor-parity-report-2026-09-13.md` (12,561 B) and
`cursor-parity-report-2026-09-12.md` (12,971 B). They will appear on the branch by themselves within
half an hour of being written, via the scheduled run.
