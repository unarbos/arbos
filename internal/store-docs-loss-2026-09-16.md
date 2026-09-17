---
cursor:
  subagentId: "bc-0b112226-cf98-5cab-92c3-2671518dd9b9"
---

# The `docs/` and `artifacts/` loss of 2026-09-16 — evidence, cause, recovery inventory

Written by the store-recovery worker. This is the working record behind the restored files in `docs/`.
Every restored file carries a banner at its top pointing here.

## 1. What was lost

Two top-level entries of this Project store disappeared: **`docs/`** (19 files, about 531 KB) and
**`artifacts/`** (a platform-created folder, always empty of agent-written content). `internal/`,
`media/` and `notes.md` were untouched and kept taking writes throughout.

`artifacts/` is not a folder anyone here made. Every agent store gets one: the recovery worker's own
store (`/cursor/stores/self/`) contains exactly one entry, `artifacts`. It has never been read or
written by any worker in this Project. That matters for the cause (section 3).

### The `docs/` inventory as it stood

From the last full listing of the directory, captured in the update-bar worker's transcript at
**2026-09-15 18:25 UTC**. Sizes and modification times are that listing's.

| File | Bytes | Last modified | Owner (author/maintainer) |
|---|---|---|---|
| `project-context.md` | 18,990 | 09-15 18:17 | coordinator `bc-ec8c092a` |
| `features-backlog.md` | 71,361 | 09-15 18:11 | features `bc-dcc57cf8` |
| `arbos-mesh-design.md` | 23,984 | 09-15 18:16 | mesh `bc-22d20d79` |
| `cursor-coordinator-spec.md` | 28,619 | 09-15 01:47 | process-parity `bc-fe947dc6` |
| `cursor-coordinator-tools-appendix.md` | 8,154 | 09-15 00:26 | coordinator `bc-ec8c092a` |
| `project-chat-vs-agent-chat.md` | 10,902 | 09-15 11:18 | symmetry `bc-2a1318aa` |
| `desktop-call-mode-design.md` | 22,278 | 09-13 20:44 | call-mode `bc-9590b6c7` |
| `multitasking-audit-2026-09-13.md` | 24,912 | 09-13 20:36 | audit `bc-938c8002` |
| `cursor-vs-arbos-agent-model.md` | 29,481 | 09-13 11:40 | plan-compare `bc-750ae2ea` |
| `filesystem-state-design.md` | 78,447 | 09-13 16:43 | fs-design `bc-6251b923` |
| `cursor-projects-research.md` | 27,444 | 09-15 17:15 | research `bc-29849c4b` |
| `swebench-loop.md` | 19,179 | 09-14 17:53 | swebench `bc-bfb2cd63` |
| `swebench-run-2026-09-13.md` | 6,457 | 09-13 12:05 | swebench `bc-bfb2cd63` |
| `swebench-harness-comparison-2026-09-13.md` | 9,786 | 09-13 13:02 | swebench `bc-bfb2cd63` |
| `qa-loop-design.md` | 45,197 | 09-15 16:27 | QA `bc-f2e2f30d` |
| `ui-qa-pass-2026-09-13.md` | 63,659 | 09-13 11:29 | parity-process `bc-85561744` |
| `cursor-parity-process.md` | 17,480 | 09-13 08:19 | parity-process `bc-85561744` |
| `cursor-parity-report-2026-09-13.md` | 12,561 | 09-13 08:19 | parity-process `bc-85561744` |
| `cursor-parity-report-2026-09-12.md` | 12,971 | 09-12 22:37 | parity-process `bc-85561744` |

`notes.md` links 17 of these. `project-context.md` it names in prose. `cursor-parity-report-2026-09-12.md`
is the nineteenth, linked only from inside other documents.

## 2. Timeline, from tool records rather than recollection

All times UTC on 2026-09-16 unless dated otherwise.

| Time | Event | Evidence |
|---|---|---|
| 09-15 18:25 | `docs/` fully listed, 19 files | update-bar worker `ls -la` |
| 05:18:42 | features worker rewrites `docs/features-backlog.md`, returns `ok` | features transcript |
| 07:34:02 | mesh worker rewrites `docs/arbos-mesh-design.md`, returns `ok` | mesh transcript |
| **07:43:28** | QA worker rewrites `docs/qa-loop-design.md`, returns `doc ok` — **last proof `docs/` existed** | QA transcript |
| 07:43–09:01 | no worker shell command anywhere touches `docs/` or the store root | all 28 transcripts scanned |
| **09:01:17** | features worker reads `docs/features-backlog.md` — empty output, exit 0 — **first proof it was gone** | features transcript |
| 09:08:09 | features worker: `ls docs/` returns nothing | features transcript |
| 09:08:23 | features worker: `ls` of the store root returns `internal media notes.md` | features transcript |
| 09:18:22 | QA worker confirms the same from its own VM | QA transcript |
| 09:19:19 | QA worker: `stat docs` — no such file | QA transcript |
| ~09:20 | SWE-bench worker confirms the same shape from a third VM, its `media/` and `internal/` writes still landing | reported by that worker |
| 09:35 | recovery worker confirms the same from a fourth VM | this worker |
| 09:39 | `docs/` recreated and proven writable | this worker |

So the loss falls in a **77-minute window, 07:43:28 to 09:01:17**.

### It is a real deletion, not a stale directory listing

Lookup by name fails as well as listing: `stat` on `docs`, on `docs/project-context.md` and on
`artifacts` all return `ENOENT`, while `stat` on `internal/mobile-coverage.md` in the same mount
succeeds. A FUSE mount with a stale `readdir` cache would still resolve a known path by name. Four
separate pods see the identical shape, so it is the shared store's state and not any one machine's view.

## 3. Cause

**Ruled out, with evidence.**

- **Any worker's shell command.** Every tool call in 28 worker transcripts was scanned for `rm`,
  `rmtree`, `rmdir`, `mv`, `find -delete` and `git clean` near a store path. Every hit is older than
  today and correctly scoped (`internal/parity/__pycache__`, `/tmp` paths, bug files on the QA VM).
  Nothing in the 77-minute window touches `docs/` or the store root at all.
- **The QA loop.** Its writes all land under `internal/qa/`, confirmed in its transcript, and its new
  federated-store scenario (`fs-01`) first ran at 09:08:25 — after the loss.
- **The features worker.** Its backlog edits rewrite one file in place (`open(p,'w').write(s)`); the
  last succeeded at 05:18:42.
- **`ui_pass.py`**, the parity driver that runs with its working directory inside the store. Read in
  full: its `shutil.rmtree` calls target `/tmp/qa-ui/...`, `/tmp/qa-ui-xdg-...` and
  `/tmp/parity-proj/.arbos` only. It touches the store solely to write under `media/qa-ui/`.
- **An agent deleting `artifacts/`.** No transcript has ever read or written the store's `artifacts/`.
  It is platform-created. No agent had a reason or a command to remove it.

**The likely cause: a fault in the store service itself.** The store is not a local directory. It is a
FUSE mount, `cursor-agent-store-fuse --backend-mode direct --bcs-endpoint https://api2.cursor.sh`,
over a server-side multi-writer store with a journal. Its own event log records a
`journal_epoch` and emits conflict events; the one line of it captured in a transcript reads:

```
{"v":1,"kind":"create_conflict","journal_epoch":"68245cb5-...","seq":1,"ts_ms":1789284481726,
 "store_id":"bc-ec8c092a-...","original_rel_path":"media/layout/layout-walkthrough.mp4", ...}
```

This store has a **documented history of losing content from workers' view**, which is the strongest
part of the case:

- **09-14 11:02** — the SWE-bench worker's pod: `ls` of the store root returned `total 0`, and
  `ls docs` gave "No such file or directory". `/cursor/stores/self` and `/cursor/stores/user` were
  empty too.
- **09-14 13:57–13:59** — the symmetry worker's pod: `internal/voice/voice-stub-server.py` missing,
  then the whole store root listing zero entries for over two minutes across three checks. That worker
  then wrote into its own code the comment *"the store mount comes and goes"*.
- **09-14 14:28** — the same pod: `self` and `user` listing normally while `stat docs` alone failed —
  the same selective shape as today.
- `ui_pass.py`'s own docstring says it writes to `/tmp` first and mirrors to the store after every
  phase *"because the store mount drops a write now and then"*.

Those earlier episodes healed on their own. Today's did not: the entries stayed gone for at least
40 minutes, on four machines, with name lookup failing. The shape fits a journal or directory-index
fault that dropped two adjacent root entries — `artifacts` and `docs` are the first two names in
alphabetical order — and nothing else. Two writes that were in flight near the window are a plausible
trigger (the mesh worker at 07:34 and the QA worker at 07:43 both rewrote a `docs/` file whole), but
nothing available to this worker proves the mechanism inside the service.

**Honest limit.** No pod-side record can show what the store service did. `/run/agent-store-fuse/events.jsonl`
does not exist on this VM, so there were no conflict events here to read. A definitive answer needs
the server-side journal for store `bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983` around 07:43–09:01 UTC,
which only Cursor can inspect. Everything above is what the client side can prove.

## 4. Recovery inventory

Restored into `docs/` — 11 files. Every one carries a banner naming its source and what is missing.

| File | Restored | Completeness | Source |
|---|---|---|---|
| `cursor-coordinator-tools-appendix.md` | yes | **complete**, 8,154 B matches the recorded size exactly | `read_file` in `bc-29849c4b` transcript, 09-15 17:11 |
| `filesystem-state-design.md` | yes | complete (78,448 B against 78,447) | repo `docs/design/`, commit `9b49a65` |
| `cursor-vs-arbos-agent-model.md` | yes | believed complete (29,463 against 29,481) | repo `docs/design/`, commit `1374ed4` |
| `desktop-call-mode-design.md` | yes | nearly complete, ~429 B short | repo `docs/design/`, commit `1374ed4` |
| `arbos-mesh-design.md` | yes | repo-adapted mirror at 09-15 19:21, plus today's three additions recovered verbatim and appended out of position | repo commit `a4a466f` + mesh worker's edit commands |
| `swebench-run-2026-09-13.md` | yes | 6,117 of 6,457 B; tail past line 80 cut | `sed -n 1,80p` output in features transcript |
| `project-context.md` | yes | **9,586 of 18,990 B**, 3 days stale | `read_file` in symmetry transcript, 09-13 06:15 |
| `cursor-parity-process.md` | yes | 8,843 of 17,480 B | `cat` output in features transcript, 09-13 00:02 |
| `features-backlog.md` | yes | 17,853 of 71,361 B, begins mid-table | `read_file` in features transcript, 09-13 02:47 |
| `qa-loop-design.md` | yes | 5,608 of 45,197 B, begins mid-document | `read_file` in QA transcript, 09-13 17:38 |
| `cursor-projects-research.md` | yes | 4,536 of 27,444 B, 4 days stale | `read_file` in `bc-e9b802f5` transcript, 09-12 22:08 |

### Needs its author — no usable copy found

| File | Bytes lost | Ask |
|---|---|---|
| `ui-qa-pass-2026-09-13.md` | 63,659 | `bc-85561744`. Built from `internal/parity/ui_pass.py` + `ui_pass_report.py`, both still in the store. |
| `cursor-coordinator-spec.md` | 28,619 | `bc-fe947dc6`. Only ~4.8 KB of disjoint `sed` fragments survive; not safe to present as the document. |
| `multitasking-audit-2026-09-13.md` | 24,912 | `bc-938c8002`. Nothing above 1.5 KB anywhere. |
| `swebench-loop.md` | 19,179 | `bc-bfb2cd63`. Its cycle-6 addition is already parked at `internal/swebench-loop-doc-update-cycle-6.md` and it holds the apply script at `/tmp/swe/loop/doc-update-c6.py`. |
| `cursor-parity-report-2026-09-12.md` | 12,971 | `bc-85561744`. |
| `cursor-parity-report-2026-09-13.md` | 12,561 | `bc-85561744`. Surviving captures used `cut -c1-700`, so every line is truncated — unusable. |
| `project-chat-vs-agent-chat.md` | 10,902 | `bc-2a1318aa`. Surviving capture used `cut -c1-220` — unusable. |
| `swebench-harness-comparison-2026-09-13.md` | 9,786 | `bc-bfb2cd63`. |

Fragments cut with `cut -c<n>` were deliberately **not** restored: they truncate every line and would
put silently corrupted text where a document used to be.

### Where copies were looked for

- `unarbos/arbos`, every ref: only `docs/design/` holds mirrors, and only of four design documents.
  No `project-context.md`, `features-backlog.md` or spec anywhere in any branch's history.
- `qa-results` branch: bug files, inbox notes and media. No `docs/` copies.
- `arbos-matrix`: an unrelated Go tree.
- No `arbos-artifacts` branch exists yet, though the `pr` tool spec describes one.
- 28 worker transcripts, harvested for `read_file` results, `cat` outputs, heredoc creations and
  write arguments.
- `internal/` and `media/`: no document copies. `media/process-parity/acceptance-2026-09-15/project-context.md`
  is the acceptance run's own test-place file, not this Project's.

## 5. What would stop it happening again

The store has no history and no undo that a worker can reach, and it has now dropped content four
times in three days. Three practical guards, cheapest first:

1. **Mirror `docs/` to a git branch on every write.** The four documents that survived intact
   survived *because* they were mirrored into `unarbos/arbos`. A `store-docs` branch, pushed by
   whoever edits a document, turns any future loss into a `git checkout`. This is the single change
   that would have made today a non-event.
2. **Never let the store be the only copy of a whole-file rewrite.** Both patterns in use —
   `open(p,'w').write(s)` and `cat > path <<EOF` — destroy the old content before the new content is
   durable. Write to `/tmp` first, then copy in, the way `ui_pass.py` already does for its captures.
3. **Have the loops notice.** A cheap check that `docs/` still lists 19 files, run at the top of each
   worker's cycle, turns a silent 77-minute gap into an alert. Today the loss was found by accident,
   by a worker whose `sed` happened to return nothing.

Worth raising with Cursor as a product bug regardless: an agent store that loses two root directories
with no event, no audit trail the owner can read, and no restore path is a data-durability fault, not
a usage error.
