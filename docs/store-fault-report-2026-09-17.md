# Project Agent Store: a client whose credential is missing lists the store as empty, with no error

Store id: `bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983`. Client: `cursor-agent-store-fuse`, `--backend-mode direct`, mount `/cursor/stores`, `--pod-grant-path /run/agent-store-fuse/pod-grant`. All times UTC, 2026-09-17.

## The finding — for the store's engineers

When the FUSE client's grant file is gone, every `MintAgentStoreToken` returns **401** and every directory read returns **an empty listing and no error**. `ls` of the store root prints nothing and exits 0; `ls` of any subdirectory prints nothing and exits 0; `[ -e path ]` is false for every file. To a person and to every program on that machine, the store has been emptied. Nothing has: other clients read it whole at the same minute. The client log is the only place the truth appears:

```
WARN list_files{target="source:cloud"}: agent-store-fuse: failed to stat pod grant file error=No such file or directory (os error 2)
WARN list_files{target="source:cloud"}: agent_store_bcs.rpc.failed rpc_method="MintAgentStoreToken" … status=401
WARN agent_store_fuse::fs: host read_dir failed error_kind="permission_denied"
```

`permission_denied` inside; an empty directory outside. A read that cannot be authenticated should fail the read — `EACCES`, or `EIO` — so that a caller sees an error and a mirror refuses to run, rather than an empty tree it may act on. Two observations support this, one of them with the cause in hand:

1. **This VM, 09:36–now.** `/run/agent-store-fuse/pod-grant` was deleted at 09:36 (by us: a sandboxed test of the kernel's wipe guard ran `cd / && rm -rf *` on a kernel from before the guard; the directory is `drwxrwxrwt` and the grant file was owned by this user). Last successful token mint 09:33:27; first `failed to stat pod grant file` 09:42:27; from then on every listing empty, no error, while a second client on another machine recorded the store whole at 09:37 and after (`store-watch` branch, `readers/cloud-mesh-3b98.jsonl`). The client log is `/tmp/agent-store-fuse.log` on this VM; the relevant lines are on branch `qa-vm-evidence-2026-09-17` as `agent-store-fuse-warn-delete-mint.log`.
2. **The benchmark loop's VM, 05:35–~06:00.** Its mount listed empty and refused writes for over twenty minutes while two other clients read and wrote the same store normally (details under "A third phenomenon", below the line). We did not have that VM's client log; the shape is the same as (1). Whether its grant went the same way, only that log can say.

Two smaller asks that follow: the grant file should not be removable by the unprivileged user whose processes it serves (the directory is world-writable and sticky, and the file was ours to delete); and a client that has lost its grant should say so once on its own log at WARN or above with the path it looked for — it does, and that line was what let us find this in minutes rather than a day.

## What the rest of this document is

Everything below the line was written believing the service was removing files from this store. It was not. The seven "loss" episodes of 09-16/17 were our own QA loop: an agent under test ran `cd / && rm -rf *` (the kernel's guard did not stop it; fixed in #410) and the store mount was the first user-writable tree under `/`. That account, with its evidence, is kept in full because it is how the finding above was reached, and because it records what we got wrong on the way. None of it is a fault of the service, and none of it should be read as one.

---

# The seven episodes were our own loop (found 07:35; kept as the account)

## What happened — in one paragraph

The QA loop turns each feature note in `internal/qa/inbox/` into a scenario: the note is handed to a live agent (kernel + model) with the words "then attack it — try each of these". The note `2026-09-13-swebench-loop-cycle-1.md` lists, under `needs_approval`, the command **`cd / && rm -rf *`** with the remark "the last one is not caught; decide whether it should be". The agent tried it, fifteen times since 09-13. The kernel's `needs_approval` (`tools/bash.rs`) does not stop it, so it ran. As user `ubuntu` on the QA VM, `rm -rf *` in `/` fails on every system directory — and succeeds on the first user-writable tree in glob order: **`cursor/`**, i.e. this store's mount. It walked `docs/`, then `internal/` in directory order, unlinking two to four files a second, until the agent killed its own job a minute or three later (`kill -- -<pgid>`). Each run deleted the same walk, a little further. That is the "same set, growing", the "hex-named files taken and named ones left" (walk order), and the "removal in progress over about a minute" of episode 6.

## The evidence, all from this VM

- **The FUSE client log** (`/tmp/agent-store-fuse.log`, this VM's mount, 296,720 lines since 09-12 22:04) shows a burst of `DeleteAgentStoreFiles` RPCs *issued by this client* at every episode and at no other time: 09-16 **09:02:42–09:05:02 (114)**, **12:20:00–12:23:39 (99)**, **14:22:42–14:24:59 (163)**, **22:42:14–22:44:15 (192)**; 09-17 **00:37:23–00:39:27 (229)**, **03:26:23–03:28:27 (175)**, **06:52:51–07:33 (≈1,300, see below)**. The counts match the vanished counts of episodes 4 (191), 5 (215) and 6 (174) to within the files the mirror does not take.
- **The scenario rollouts** (`internal/qa/rollouts/<ts>-inbox:swebench-loop-cycle-1/`) start within 30 s of each burst: `20260916T090221Z`, `121948Z`, `142225Z`, `224203Z`, `20260917T003705Z`, `032559Z`, `065228Z`. Each one's `state-after/agents/root/jobs/j1/meta.json` reads `"command": "cd / && rm -rf *"`, and the transcript has the agent saying so — on 09-17 06:52: *"`cd / && rm -rf *`. The prompt states this one is not caught, so I expect it to execute."*
- **The process itself**, still alive at 07:33: pid 1325465, `rm -rf bin bin.usr-is-merged boot cursor dev etc … workspace`, cwd `/`, started **06:52:49**, parent pid 1 (orphaned when the scenario's kernel was stopped; the harness reaps by scratch directory and this process's cwd was `/`), environment `HOME=/tmp/arbos-qa-inbox:swebench-loop-cycle-1-…`, with an open handle on `…/internal/qa/rollouts/…`. It was killed at 07:33:40. Nothing has been deleted since.
- **Why no earlier run showed it:** the same scenario ran on 09-13 21:01, 09-14 03:54, 09-15 03:06/05:27/14:12, 09-16 04:16 and 19:33, and 09-17 05:28 with 0–3 deletes — the agent did not reach that item, or the store had nothing at the top of the walk. From 09-16 09:02 it did, every time.

## The episodes, one by one — which signature, and did anything come back on its own

| # | Window | Signature | Came back on its own? |
|---|---|---|---|
| 1 | 09-16 09:02:42 → 09:05 | **Deletion by our `rm`** (114 unlinks from this client). The "07:43 last good write → 09:01 absent" in the old table was a reconstruction; the first unlink is 09:02:42 | No — deleted files do not return. `docs/` was restored from the mirror |
| 2 | 09-16 12:20:00 → 12:23:39 | **Deletion by our `rm`** (99 unlinks). The old table called it "possibly a partial view" | `parity/` and `features-inbox/` were listed again at 12:42; whether their owners' loops rewrote them (both regenerate their files each cycle) or the 12:23 reads were partial under load, we cannot tell from our record. `docs/` did not return; restored |
| 3 | 09-16 14:22:42 → 14:24:59 | **Deletion by our `rm`** (163 unlinks). The "unstable counts seconds apart" another client saw were the deletion in progress, not a partial listing | No; restored |
| 4 | 09-16 22:42:14 → 22:44:15 | **Deletion by our `rm`** (192 unlinks; 191 vanished). Three clients agreed because the files were really gone | No; restored at 23:0x |
| 5 | 09-17 00:37:23 → 00:39:27 | **Deletion by our `rm`** (229 unlinks; 215 vanished). The "write into `docs/` failed: no such directory" at 00:45:35 was our own mirror pass finding the directory gone | No; restored |
| 6 | 09-17 03:26:23 → 03:28:27 | **Deletion by our `rm`** (175 unlinks; 174 vanished). The one client-side 502 in this window is *our* delete of a file failing at 03:27:16 | No; restored 03:29–03:34 |
| 7 | 09-17 06:52:51 → 07:33:40 | **Deletion by our `rm`**, this time never killed by the agent; 40 minutes. It emptied `docs/`, `features-inbox/`, `parity/`, most of `internal/qa/bugs/`, then began on `internal/qa/rollouts/`. The mesh reader's 07:07 alarm saw our restore half-done | No. Restore ran 07:01–07:4x from the 06:41 mirror and was stopped at 07:43 (see M-141) |

**What the 502s were:** real, and unrelated to the vanishing. The FUSE log on this VM shows `BCS request failed: 502` and `read_dir failed error_kind="timeout"` at 07:14–07:28, *during* our 40-minute `rm` plus our restore plus a stuck `find` over the whole tree — the service was being hammered by this client. In episodes 1–6 this client's log shows **no** read failure at all: the listings that came back short came back *successfully*, because the files were gone.

**The two mobile signatures (`internal/mobile-findings.md` M-141, M-142) are also ours:**

- **M-141, three files reverting together to an older consistent state** (38,300 / 9,073 / 9,136 bytes): those are byte-for-byte the versions of `internal/mobile-findings.md`, `mobile-cycle-reports.md`, `mobile-coverage.md` in the **06:41 mirror tip `e8289af5`**. Our restore (`cp -r` of that tip, 07:01 onward) wrote them over the phone loop's newer files. An older version was indeed being served — by us. Their Mac mirror was right and their restore rule was right.
- **M-142, one file gone from a directory that lists perfectly** (`features-inbox/2026-09-17-mobile-first-word-lost-in-the-speech-server.md`, written 06:46): our `rm` deleted `features-inbox/` around 06:53–06:55; our restore put back the 37 files the 06:41 mirror had; the 06:46 file was newer than the mirror and stayed gone. Their `/tmp` copy was the only one.

Anything written into the walked paths between the 06:41 mirror pass and the 06:52 `rm` that its owner does not hold elsewhere is lost, and we cannot list it: the 06:56 mirror pass could not run (its own script had just been deleted).

## What we changed (07:35–07:50)

1. The runaway `rm` killed; the restore that was overwriting newer files killed.
2. **Every kernel the loop starts now runs with the store hidden**: `internal/qa/deploy/ns-wrap.sh` puts the kernel in a mount namespace where `/cursor/stores` is an empty directory, then drops back to uid 1000; `run.py` refuses to start a kernel without it. Tested: inside the wrapper `cd / && rm -rf cursor` cannot reach the store; outside, the store is untouched. The desktop app and every kernel it spawns run the same way (`desktop-kill-kernel-under-ui` passes inside it).
3. **The mirror's shrink guard is absolute** (`internal/mirror-docs.sh`): a directory on the tip that does not list here refuses the pass; any fall in file count refuses it unless `MIRROR_ALLOW_SHRINK='<reason>'` names why. The old one-tenth allowance had accepted a 454-for-492 view at 07:0x.
4. **Restores are staged-only, and the rule is written down**: a restore is safe only when you can name why no newer version can exist — the source is your own unedited copy and nothing else writes those files (the phone loop's reasoning, adopted). Copying a mirror over a live tree is not that, and M-141 is what it does.
5. The second reader says `RESTORING` instead of `FAULT` while a restore is marked on `store-watch` (`store-second-reader.sh restore-begin|restore-end`).
6. Kernel bug filed: `internal/qa/bugs/qal-j15-needs-approval-misses-cd-root-wipe.md` — `cd / && rm -rf *` runs without approval.

**08:13, after the store was hidden:** the same command ran once more from the same note. The store logged 0 deletes from this client — the wrapper held — and the walk went on to `home/ubuntu`, taking `~/arbos-qa/{repo,deploy,logs,state,…}` before the reaper killed it. The wrapper now also makes `~` and `/workspace` read-only, and destructive commands are removed from prompts outright (the first defuser's "do not run this, only check whether the kernel asks" was checked by running it).

**The per-client empty view has a cause on at least one client (09:42, this VM):** `/run/agent-store-fuse/pod-grant`, the FUSE client's credential, was deleted (by our own sandboxed control run of the wipe guard — the directory is world-writable and the file was ours). From then on every `MintAgentStoreToken` returned 401 and every listing came back **empty with no error** (`host read_dir failed error_kind="permission_denied"` in the client log, nothing at all to the caller). Other clients read the store normally at the same minute. That is the shape Client A saw at 05:35. Whether Client A's grant went the same way we cannot see from here, but the client behaviour is the same and is the thing to fix: a client that cannot authenticate must fail the read (EACCES), not answer with an empty directory.

## What was wrong in the record below, named

- "Why we say service, not client": three clients agreeing proves the files were gone, not who removed them.
- "Could the client have done it? — checked, and no": we checked our *tools* (mirror, publisher, cycle) and our *kernel's write path*, and never the *agents the loop runs*. The FUSE log was on this disk the whole time and answers the question in one grep (`delete_files`).
- "The monotone property" and "separates files by name pattern": the walk order of one `rm`, killed at different points.
- "A third phenomenon" (05:35, per-client empty view on the benchmark VM): **not** explained by this — no deletes from this client between 05:28 and 06:52. It stands as the one open observation about the service, and the 502s at 07:14–07:28 say the service does fail reads under load. That is what remains worth telling the store's engineers, in a much smaller note.

---

# The record as written before the cause was found (superseded; kept for the account)

Period: 2026-09-16 07:43 UTC to 2026-09-17 06:56 UTC.

## Summary (superseded)

Three phenomena, recorded separately below: selective file loss (seven episodes), partial listings, and — live as this is written — one client of the store seeing it empty and unwritable while two others read and write it normally.

Seven times in twenty-four hours, a set of files and directories vanished from this store while the rest of it stayed intact and writable. The set is not random: it is the same set each time, growing between episodes (every file lost in one episode is lost again in the next, plus more), and inside one directory it separates files by name pattern while leaving files written the same way by the same client untouched. Three independent clients saw the same picture at the same time, stable across repeated reads for minutes. We can restore from our mirror; we cannot see why the files go. The server-side journal for this store id over the windows below should show it.

## The seven episodes (superseded — see the table above)

| # | Window (UTC) | How it was found | What was gone | Certainty |
|---|---|---|---|---|
| 1 | 09-16 **07:43:28 → 09:01** | A successful write at 07:43:28 is the last good moment; at 09:01 `docs/` and `artifacts/` were absent, and stayed absent across many reads by several agents over the following hour | `docs/` (19 files) and `artifacts/`, entirely | Loss (confirmed by the project owner) |
| 2 | 09-16 **~12:20 → 12:26** | Single reads at 12:23–12:25: store root listed as `internal/ media/ notes.md`; `internal/` at 15 entries; `docs/` absent; `internal/parity/`, `internal/features-inbox/`, several `internal/*.md` absent. `docs/` restored from the mirror at 12:26; `parity/` and `features-inbox/` were present again on the 12:42 mirror pass without anyone reporting a rewrite | as listed | Uncertain — single reads; possibly a partial view (see §Partial views) |
| 3 | 09-16 **14:20:32 → 14:23:24** | Mirror pass at 14:20:32 saw the store whole (22 docs, 377 files under `internal/`); the next pass at 14:23:24 found the mirror script itself absent; a full listing at 14:23:51–14:25:11 found 155 files missing; store root again `internal/ media/ notes.md`, `internal/` at 15 entries | 155 files: all 21 `docs/`, 99 of 184 `internal/qa/bugs/`, all `internal/parity/`, all `internal/features-inbox/`, 8 top-level `internal/*`, 4 `internal/qa/*` | Uncertain — one read per file; another client in the same minutes saw *unstable* counts (1092, 1122, 1092 files under `internal/` seconds apart), i.e. a partial view |
| 4 | 09-16 **22:37 → 22:52:16** | Mirror pass at 22:15:28 pushed a whole store (`0321076e`: 22 docs, 434 files under `internal/`); the pass at ~22:30–22:37 found no change; at 22:52:16 the mirror script was absent. Three clients on three machines listed the store independently within the same minutes and saw the same picture; this client read it six times over 22:54:21–22:56:31 with identical results | 191 files: all 22 `docs/`, all 23 `internal/features-inbox/`, all 15 `internal/parity/`, 11 top-level `internal/*`, 115 of 216 `internal/qa/bugs/`, 4 `internal/qa/*`, 1 `internal/mobile/*` | Loss |
| 5 | 09-17 **00:37:11 → 00:45:35** | Mirror pass at 00:37:11 pushed a whole store (`dff30aa7`); a write into `docs/` at 00:45:35 failed with "no such directory"; six reads over 00:45:41–00:47:44 identical | 215 files: all 22 `docs/`, 21 `internal/features-inbox/`, all 15 `internal/parity/`, 148 of 218 `internal/qa/bugs/`, 4 `internal/qa/*`, 4 top-level `internal/*`, 1 `internal/mobile/*` | Loss |

| 6 | 09-17 **03:23:40 → 03:27:29** | Mirror pass at 03:23:40 pushed a whole store (`7d283951`, 24 docs). A write into `docs/` failed at 03:26:4x. Reads seconds apart then caught the deletion **in progress**: 03:26:58 `internal/mirror-docs.sh` present, `features-inbox/` 3 files, `parity/` 15; 03:27:16 the script gone and `features-inbox/` 0; 03:27:29 `parity/` 1; stable from 03:28:57 (`parity/` 0, 133 of 222 bug files) | 174 files: 23 `docs/`, 31 `internal/features-inbox/`, 15 `internal/parity/`, 89 hex-named `internal/qa/bugs/`, 11 top-level `internal/*`, 4 `internal/qa/*`, 1 `internal/mobile/*` | Loss — the counts fell monotonically over about a minute and did not return |

| 7 | 09-17 **06:41:17 → 06:56:17** | Mirror pass at 06:41:17 pushed a whole store (`e8289af5`: 25 docs, 323 files in the watched scope). The QA client's probe at 06:56:17 found `docs/` absent, `internal/mirror-docs.sh` absent, 13 of 243 bug files left, `notes.md` present and writable; its second reader at 06:56:59 found 2 files in scope, root `internal media notes.md`. Restore from the mirror began 07:01:28 (`docs/` first, then `internal/`) and, through the mount at 10–13 files a minute, ran until about 07:25. **The mesh client's second reader read the store at 07:07:26, mid-restore**: `docs/`, `features-inbox/`, `parity/`, `notes.md` present, 86 files in scope, 237 bug files absent — the restore had not reached them yet. Both clients therefore agree on the loss (06:56–06:57) and the 07:07 view is the repair in progress, not a second event | at least 321 of 323 files in scope: all 25 `docs/`, `internal/features-inbox/`, `internal/parity/`, `internal/mirror-docs.sh`, 230+ of 243 `internal/qa/bugs/` | Loss — first caught by a watcher (the QA client's probe and reader, 06:56) rather than by someone tripping over it; the mesh reader's 07:07 alarm was true (files absent) but was the restore, and readers now say RESTORING while a restore is marked |

The sixth episode adds one observation the others could not: the removal is not instantaneous. Over roughly 30–60 seconds, files disappeared from one listing to the next in the same order of directories each time, and none came back.

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

## Could the client have done it? — checked, and no

The first question a service engineer will ask, so we asked it of ourselves first. The same day we found a bug in our own kernel of exactly the worrying shape (our `qal-j09`): a read of a page that fails is treated as an empty page, and the next write replaces the real page with the empty one by temp-file-and-rename. If any of the losses above were that, this report would be blaming the service for our bug. Three checks, all against the record rather than the pattern:

1. **Truncation versus absence.** Our bug shrinks a file to a smaller *valid* file; it cannot remove a file, and it cannot remove a directory. Every loss above was recorded as *absence* — `[ -e path ]` false, `ls docs` → "no such file or directory" for the directory itself. Across all 108 mirror snapshots of the day (one every pass, 09-16 09:48 → 09-17 04:50), **no file present in two consecutive snapshots shrank by more than half**; the 22 shrinks that did occur are ordinary edits (the root `notes.md` pruned by its author, 1–5%; two documents trimmed by theirs), none inside an episode's window, none to near-empty. A client truncation would have appeared exactly there.
2. **Reach.** The write path of our bug touches only files the kernel itself owns through one module (`.arbos/notes.md` and an agent's own checklist page). This store has no `.arbos/` at its root and is not served by a kernel; the files lost (`docs/*.md`, `internal/features-inbox/*`, `internal/parity/*`, `internal/mirror-docs.sh`, the hex-named drafts) are written by agents' plain file writes and by `cp` from other machines, never through that module. The one file in the store with the shape our bug would rewrite — the root `notes.md` — is the one file that has never been lost.
3. **Our own tooling.** Nothing we run deletes inside the store: the mirror script removes only its own temp files and index; the QA loop's cycle removes only its VM-side staging and rollouts older than 30 days; the publisher removes only its results clone. The restores copy in; nothing copies out with `--delete`.

Also checked and set aside: the four files that vanished between consecutive snapshots *outside* the episode windows are two deliberate moves by their authors (`internal/kernel-self-update-design.md`, twice — the second was our own re-deletion of a duplicate), one file of ours that reappeared unchanged on the next pass, and one report folder moved by the desktop loop. None coincides with an episode.

So: nothing in the record fits a client write. The losses are absences of files and of whole directories, at times when no client of ours wrote to those paths, seen identically from three machines. That is what we are asking about.

## A third phenomenon: the same store answers differently to different clients at the same moment (09-17, from ~05:35 UTC, live as this is written)

Distinct from the deletions (§episodes) and from the partial listings (§partial views), and recorded while it was happening rather than reconstructed.

- **Client A** (the benchmark loop's VM, its own mount of this store id): from about **05:35 UTC** the mount lists empty and refuses writes. Its whole cycle's output is staged locally under `/tmp/swe/store-pending/` with a script to apply it when the mount returns.
- **Client B** (the coordinator's machine): writes to the root `notes.md` succeeded throughout that window — the file's modification time read from Client C is 05:56:56 UTC, 51,333 bytes.
- **Client C** (the QA loop's VM, `fuse.agent-store` mount, this report's author): read three times at **05:57:33, 05:57:38, 05:57:43 UTC** — `docs/` present with 25 documents, `internal/mirror-docs.sh` present, 234 files in `internal/qa/bugs/`, `notes.md` as above; at **05:57:48** a write, read-back and delete under `internal/qa/` all succeeded. The mirror passes at **05:23:42** and **05:41:09** UTC — inside Client A's blackout — each pushed a whole store (24 documents, 651 K → 654 K).

So at one moment the service told one client the store was empty and unwritable, and told two others it was whole and writable, including the very files the first could not list. This is not a deletion (nothing was lost; the mirror kept pushing the full tree) and not a settling listing (Client A's view held for over twenty minutes). Where a service engineer might look: per-connection or per-session state — a mount whose session with the service failed or expired and now answers from nothing, while other sessions on the same store id are served normally.

A limitation of ours this exposes: the mirror and its alarm run on Client C. They can only refuse or alarm on what **that** client sees; a fault that leaves Client C's view intact is invisible to them, which is why nothing fired between 05:35 and 05:57. The remedy on our side is a second reader on another machine, or each writing client checking that its own mount lists a known file before trusting a write — the benchmark loop's stage-locally-and-apply-later pattern is the right one and we are adopting it.

## Partial views — a second phenomenon, kept separate

On 09-16 at ~14:20 one client observed *unstable* counts seconds apart (1092, 1122, 1092 files under `internal/`), every sampled file present, and a clean pass minutes later. That is a listing that returns a subset for a while and then completes. It is why episodes #2 and #3 are marked uncertain: they were single reads and could have been partial views. Episodes #1, #4 and #5 were not: stable for minutes, across clients. If the service distinguishes "listing incomplete" from "object deleted", that distinction would tell us which of the two we saw in #2 and #3.

## What recovery cost

- An off-store mirror (a git branch, one commit per pass) now runs every 15 minutes; a pass walks `docs/`, `internal/` (minus bulk) and `media/desktop-feedback/` and takes 50 s to 4 min on this mount. Its safety gate refuses to push when the view is damaged; it has refused correctly in #3, #4 and #5.
- Each restore (`git archive` from the last good commit, then `cp` into the mount) took 7–8 minutes for ~200 files and left the store correct. Anything written into a lost path between the snapshot and the loss is gone: windows of 15 min (#4) and 8 min (#5). In #2 and #3, restores made before we understood partial views may have overwritten edits made after the snapshot; authors were told the windows.
- Two agents held all writes for ~15 minutes during #4; every agent that writes documents here has been told to re-read before concluding a file is lost and to restore only from the mirror. Roughly one agent's full attention for a day went to detection, restore and record-keeping.

## What we cannot see

The server-side journal for this store id: which objects were deleted, evicted, unlinked or hidden, by what actor or process, at 09-16 07:43–09:01, ~12:20, 14:20–14:23, 22:37–22:52, 09-17 00:37–00:45 and 03:23–03:27 UTC. Our client sees only the result. Two observations that may narrow the search on that side: directory mtimes read through the mount are constant (`2026-09-12 22:04:36`, the store's creation) regardless of content changes; and `chmod` on a file in the mount is refused (`Operation not permitted`).

## Appendix — where the primary records are

- QA worker's episode record (all five, with the exact missing lists and restore steps): `internal/store-docs-loss-2026-09-16-qa-record.md` in this store
- Recovery worker's account of episode #1: `internal/store-docs-loss-2026-09-16.md`
- Mirror design, boundary and procedure: `internal/store-docs-mirror.md`
- Per-pass loss list: `internal/qa/store-mirror-losses.jsonl` (QA loop); mirror branch `store-docs` in the `unarbos/arbos` repository — commits `bdb3578f` (11:00), `8764ff48` (14:20), `0321076e` (22:15), `dff30aa7` (00:37), `7d283951` (03:23) are the last-good snapshots before episodes #2–#6
