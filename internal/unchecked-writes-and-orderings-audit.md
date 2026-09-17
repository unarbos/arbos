---
cursor:
  subagentId: "bc-dcc57cf8-d8e5-575b-8c61-7a42eda2b027"
---

# Unchecked writes and orderings: the audit, 2026-09-17

Every `let _ = write / rename / remove / append_event / save` in the kernel, engine, core and hub (122 production sites at `main` `7017eb75`, test modules excluded), each answered with two questions:

1. **What does the next reader believe if this write did not land?**
2. **What does it read if the write landed somewhere other than where it looks first?** (QA's sharpening from qal-j19: a fallback location without a reader that knows about it *is* this bug.)

Ranked: **destructive** (a reader acts on a record that is not there), **misleading** (a reader or a person is told something false), **harmless** (nothing believes anything on the strength of it). Destructive ones are fixed in [#444](https://github.com/unarbos/arbos/pull/444) or already on an open PR; the rest are a list with owners. Instances answered earlier today: `blocked.rs`, `jobs::detached`, `jobs::notified` (#392); `HeldRecord::save`/`load` (#441, qal-j19); `inflight::start`, the undo mark's line, `notes` unread (#392); `jobs::spawn` meta, the `killed` marker's two writers, the push registry, `apply_patch` (#434).

## Destructive — fixed

| Site | Reader | Belief if unwritten | Fix |
|---|---|---|---|
| `tools/git.rs` `snapshot_turn_tree`: the undo mark (`runtime/checkpoint`) | `undo` | Uses the **previous turn's** mark: `reset --hard` to an older HEAD. | Mark removed on write failure; `undo` with no mark refuses. #444 (and #392 stamps the mark with its turn line, refusing a stale one). |
| `core/files.rs` `exclude_locally`: `.arbos/` into `.git/info/exclude` | The person's next `git add -A` | Stages the agent's whole record — transcripts, checkpoints — into their repository. | Returns the error; `init_arbos_repo` writes a failed notice on root's transcript once per start with what to do. #444 |
| `sched.rs` panic path: notice + `turn_complete` | The next boot's wake replay; the window | The wake reads as unfinished and is **replayed** — tools run again (qal-j02's shape); the window shows a turn that stopped for no reason. | `note_turn_error` marks the folder; if the transcript refuses the words they go to the windows live (as #408 did for `Err`). #444 |
| `engine/inflight.rs` `start` (main) | The continued turn after a crash | Believes the tool call never started and **issues it afresh** (qal-j02). | Checked on #392: an unwritable record refuses the call. **Land #392.** |
| `subs.rs` `SUB_MARKER` on a subscription's job | A kernel that finds the job at boot | Cannot place the job; the run's result reaches nobody — finished and unnoticed. | Unwritten → the job is ended and the run reported as not started. #444 |
| `jobs::spawn` meta.json after the process ran | `list`, `kill`, the leash's cap | A command running in a folder nothing can list, kill or cap. | Fixed in #434. |

## Misleading — fixed where cheap, else listed

| Site | Reader | Belief if unwritten | Status / owner |
|---|---|---|---|
| `serve.rs` `Frame::Pause` / `SetModel` / `SetMode` `a.save` | The client; the next start | The window shows the setting set; the next start reverts it silently. | Error frame to the client: "changed for this run only…". #444 |
| `serve.rs` `Approval` event | The transcript's reader | A tool that ran with no one's say-so on the record. | Logged `approval_unrecorded`; the frame still reaches the window. #444 |
| `jobs.rs:415` `killed` marker before the signal | `jobs`, a person | A killed job with "no exit recorded" (qa-024). | Listed. features (me). Low: the write precedes a kill that then works. |
| `jobs.rs:470–533` reap markers at boot | `jobs` | Reaped leftovers with no reason. | Listed. features. |
| `subs.rs:1007`, `serve.rs:287` `settled` markers | The next boot's cut-run sweep | A run seen to its end is reported again as "cut by a restart". | Listed. features. Duplicate delivery, not loss. |
| `remote.rs:459/1240` `file.save` (remote links) | The next boot | Forgets a remote child (a kernel left running there, qa-038's shape) or keeps a forgotten one. | Listed. features. |
| `remote.rs:871` stand-in's address | The next boot | The child unreachable after a restart. | Listed. features. |
| `acp_worker.rs:381` session id | The next ACP turn | A fresh session, context lost. | Listed. features. |
| `plan.rs` turn-folder `meta.toml` renames (`271`, `351`, `423`) | `close_turn_folder`, the feedback bundle | A turn folder never closed, or opened without its start. | Listed. features. Misreport only. |
| `mechanism.rs:43` reset | The `changes` view | Last task's mechanism line shown for this one. | Listed. features. |
| `repro.rs` records | The repro gate (`ARBOS_REPRO_REQUIRED`) | Fewer reproductions counted; an edit refused. | Listed. env-gated, default off. |
| `blocked.rs:57/62` write side | `blocked` list readers | A file's blocked state stale. | Read side fixed in #392; write side listed. |
| Transcript notices (`batch.rs:610`, `compact.rs:533`, `turn.rs:604`, `hooks.rs:542/605/1172`, `hub_link.rs:273`, `serve.rs` several, `migrate.rs:176`) | A person | Not told a thing the kernel meant to say (a nudge, a hub refusal, a compaction, a stop's kill refusal). | Listed. Omission, never a false statement. features. |
| `plan.rs:42` cut events at boot | The window | A turn cut by a restart with no line saying so. | Listed. |
| `protocol.rs` PROTOCOL.md | The model | A stale protocol file (#392 noted). | Listed. |
| `host.rs:554` `remember_place` | The desktop's recent places | A place missing from the list. | Harmless-to-misleading. |

## Harmless

Temp-file removes after a rename (`inbox.rs:175`, `kernel/files.rs:120/201`, `remote.rs:1653`, `git.rs:422/423`), scratch cleanups (`screenshot.rs`, `record.rs`, `pr_tool.rs`, `browser.rs`), `create_dir_all` before a checked write (`inflight`, `repro`, `mechanism`, `git.rs:324/685`, `grep.rs:32`, `protocol.rs:26`), the lock file's remove on drop (`lock.rs:47` — the flock is the truth, not the file), the roster's stale-entry sweep (`hub.rs:805`), log rotation (`klog.rs:39/74`), the provider trace (`provider.rs:306`), legacy `kernel.json` (`serve.rs:1905`), the legacy `focus`/`checkpoint` moves (`core/files.rs:220`, retried next start), the root agent's `cwd` backfill (`files.rs:245`), the default focus (`files.rs:428`), the worktree's exclude (`worktree.rs:362`; the checkpoint has its own excludes since #405), `jobs::seen` offsets (`382`: output shown twice at worst), `bash.rs:241` `keep`, the file-hook stdin writes, the attach socket writes, `git::snapshot` (unused).

## Question two — writes that land elsewhere, readers that look first

Every place a reader searches more than one location, checked for "does it know about the other copy":

| Reader | Locations | Verdict |
|---|---|---|
| `HeldRecord::load` | `runtime/place-held.json`, temp | **Was first-match; now newest-by-time** (#441 `38b2e145`, qal-j19 third shape). |
| `resolve_history_agent` (#436) | live `agents/<id>`, `archive/agents/<id>`, then by name in each | Live before archived is the right order: an id is in one place at a time (the archive move is a rename). A **name** held by both a live and an archived worker resolves to the live one — correct, the live one is the one that can be spoken to; `history_end.id` says which. |
| Checkpoint sidecar (`checkpoints.d/<line>.json`) vs `checkpoints.jsonl` line | `restore_files` waits for the sidecar, then reads the line | The sidecar is newer by construction and is preferred when present with the same `head`; a stale sidecar from an earlier turn at the same line is guarded by the `head` check. OK. |
| Leash pointer (`runtime/leash/<pid>`) vs the job's own path | The wrapper: `$D`, else the pointer | The pointer is read only when `$D` is gone, and only if it names an existing folder. OK. |
| `kernel_json_read` (runtime vs legacy) | New path first, legacy second | Both written at start (`serve.rs:1905` legacy is best-effort). A legacy copy from an **older kernel still running** would be read only if the new path is absent — which it is not while the new kernel serves. OK; the legacy write can be retired once the desktop reads only the new path. |
| Roster files (`.arbos/machines/*.toml`) | One per machine, stale ones swept after new ones written | OK. |
| The `place-held` record vs the lock file's pid | Record keyed on the holder's pid; a different pid resets it | OK. |

## Orderings — destroy before replace, checked this pass

Beyond the ones fixed today (#405 record-first, #419 restore, #422 roll/fork, #434 spawn/marker/registry/patch, `swap-first` re-exec): `inbox::claim` (rename into the turn folder; a failure leaves the message in the inbox), `inbox::deliver` (temp+rename, temp removed on failure), `notes::save` / `status::write` / `waiting` / `subscription::save` / `write_roster` / `HeldRecord::save` (temp+rename), `roll_transcript` (rename whole, then the opener; checkpoints move with it), `archive_finished_inner` (pointers repointed, then one rename, refused if the destination exists), `remove_if_clean` (worktree removed, branch deleted only when it has no commits), remote spawn (stand-in removed only on failure), `apply_patch` (planned whole, written in order, partial application named). No new destroy-before-replace found.

## Rule, restated for the next pass

A write the kernel makes is one of three things, and the code must say which: **a record** (checked, and its reader refuses or says so when it is absent), **a best-effort note** (`let _`, and nothing downstream may believe anything on its strength), or **a fallback** (and every reader of the primary knows about it). The pattern is in the hands, not the file: qal-j19 was written at 11:40 and audited at 12:00.
