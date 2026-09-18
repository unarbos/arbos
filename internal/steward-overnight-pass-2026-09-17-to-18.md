---
cursor:
  subagentId: "bc-71eb0fc3-658e-5b64-8b2b-9854416c9baf"
---

# Merge steward — overnight pass, 2026-09-17 19:45 → 2026-09-18 02:10 UTC

Continues from `mac-update-pass-2026-09-17.md` (which ended at `2c8d879`). Every merge is a merge commit on `main`; `rust` fast-forwarded after each push; the `v0.2.0` draft release was not touched and nothing was published by the steward.

## Where things stand

| | |
|---|---|
| `main` = `rust` | `9bb49c61` |
| TestFlight (phone) | **1765**, run for `9bb49c61`, printed `uploaded 0.2.0 (1765) 9bb49c61` |
| Dev channel (Mac) | update-bar owns the number; last read by the steward: 1662 at `8d0688d1`, since moved on |
| Live hub | rebuilt at `11a01d84` (#538) per coordinator; **predates #545** (`dd7814fc`), which the phone needs for sleeping machines |
| Open PRs → `main` | none |

Every TestFlight number above and below was read from the run's own `uploaded 0.2.0 (N) <sha>` line, not computed.

## Merged (65 merges, in order)

Grouped by what Jacob gets. Heads are the merge commits.

**Phone builds** (each `ios/` merge produced a TestFlight upload; numbers in order): 1595 (#480), 1657 (#502, call text in project chat), 1699 (#433, composer names the visible project), 1716 (#529, no-match empty state), 1725 (#533, banner names the project), 1731 (#535, headings/bullets as typography), 1735 (#537, Settings on the app palette), 1748 (#543, hub refusal words; `last_activity_ms` as `now`/`4m`/`2h`/`3d`), **1765** (#547, leaving an unstarted call no longer kills the app; workers sheet shows the kernel's children).

**Voice / GPT-Live:** #492, #490, #495, #500, #501 — a call binds to the tab's folder, GPT-Live sees the project and on-screen chat, voice rows stay in the attached project's chat.

**Data-loss and identity family (the day's theme):** #499 store identity by inode, kernel stops rather than write where its store is not; #522 lock files removed only when they are our own (device+inode); #525 a token beside the inode so sshfs/FUSE re-numbering is not read as a move, and the job spawn checks the store; #530 the desktop never bootstraps where no store is (the ghost's real maker); #521 the checkpoint-tree wait bounded at 20 s with a late tree dropped, not kept; #527 checkpoint refs of unreachable turns dropped so `.git` stops growing; #503/#526 the mechanism line stamped with its task and checked on read.

**Swallowed writes said (audit):** #509 (`close_turn_folder` no longer hangs boot on a read-only `turns/`), #513, #518 (git missing said once; `git_missing` in `kernel.json`), #536 (on the hello frame), #520 (unwritable folder refused with the situation).

**Kernel/jobs:** #491, #493, #519 (`ps -ww`), #510 (ETXTBSY retry, only that error), #532 (zombie believed only when seen twice), #548 (`job_stop` frame through the kernel's own group kill), #534 (small-context model named once), #541 (prompt: "no change needed" takes evidence), #506 (shell edits recorded as edits), #546 (**Jev routes mechanical steps; on by default for an OpenRouter key; `jev = false` is the way back**).

**Hub:** #538 (`last_activity_ms` per project), #545 (offline machines stay on the roster; retargeted from #538's branch to `main` by the steward to avoid the #372 auto-close shape).

**Desktop:** #486, #489, #494, #496, #497, #498, #511, #512, #469 (feedback over ssh for a tab that never attached), #531, #542 (native fullscreen Space), #544 (Project panel matches Terminal; `+` menu; divider drags), #540, #505.

**Dev channel:** #504 (reclaims a deferred green build when the newer commit goes red; `--first-parent` added at the steward's hold), #517 (a run does not wait on its own triggering commit — the "sibling run" is the same sha's CI on `rust`, structural to the steward's fast-forward).

**Test pins for the day's flake roster:** #508, #513, #514, #516, #523, #528, and #542's follow-up `c130abeb` — every test that went red on CI today (`stop_keeps_follow_up`, `archived_children`, `archive_children`, `feedback_bundle`, `audit_kernel_2` nudge, `done_wake_fold`, `repeat_near_dup`, `restart_states`) now waits on a fact or holds its worker behind root's turn.

**Rig / harness:** #515, #524 (+ follow-up), #539, #540, #505.

## Holds and judgement calls

- **#504 held at `ceef3af0`** for one flag: the walk used `rev-list` without `--first-parent`, so PR-branch commits with green `pull_request` runs would have been chosen (checked against the live API: #499's branch head read `success`). Author added the flag and a merge-shaped test; lifted at `aa73095e`.
- **#445 closed as landed**: merging it into `main` yields a tree identical to `main` (its content came through #476, whose merge message names it). Verified by the tree, not the PR state.
- **#507 closed as superseded** by #508 (same file, same fix).
- **#469, #433, #523**: `main` merged into the branch by the steward so CI ran on a current base; all three green and merged.
- **#542**: the desktop change landed on its rerun's green (first run red on `restart_states_e2e`, a kernel test a desktop-only diff cannot reach); the author's later test-only commit landed separately.
- **#546** read as a default-on engine behaviour change: Jev tool steps go through the same `batch::run` as LLM tool calls (read-only / store / coverage checks unchanged); `secret` results redacted from the 32k card; errors fall through; cost counted in spend.

## Post-merge pushes (the exit-check class)

Commits pushed to a branch after its PR merged, with no PR of their own:

- `cursor/rig-clipped-aa39` — **two** commits after #505 (`e90661a8` 23:32, `34717a10` 23:44). Flagged on #505 twice; still no PR. Layout worker (aa39).
- `cursor/mobile-cycle-55-journey-recording-a4fa` (#524) and `cursor/mobile-cycle-60-oldest-rows-a4fa` (#543) — both resolved: #524's follow-up merged; #543's crash fix reopened as #547 and merged.

Stale Part A/B entries in `steward-exit-check.sh` output unchanged from earlier reports (`agent-unlisted`, `archive-children-default`, `refuse-a-stranger-kernel`, `window-layout-tabs-right-panel`; Part B list unchanged).

## For the next pass

- Live hub wants a rebuild from `dd7814fc` or later (#545) before the phone shows sleeping machines.
- `rig-clipped-aa39`'s two orphan commits need a PR from the layout worker.
- When the update-bar writes the next Mac number, tell Jacob that build turns Jev on (#546); `jev = false` in `config.toml` reverts to the one-model loop.
