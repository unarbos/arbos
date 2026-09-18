# QA loop design

> **Rebuilt 2026-09-16 10:00 UTC.** The original (45,197 bytes, last written 2026-09-16 07:43 UTC) was lost with the whole `docs/` folder between 07:43 and 09:01 UTC (`internal/store-docs-loss-2026-09-16.md`). This file is a reconstruction by the QA worker (`bc-f2e2f30d-1298-59f1-a24c-55113322de28`) from the bug files in `internal/qa/bugs/`, the scenario docstrings in `internal/qa/*.py`, `internal/qa/kickoff-history.jsonl`, the PRs, and the worker's own history. Each section says which it is: **evidenced** (rebuilt from files that still exist or from the recovered 5,608-byte fragment) or **reconstructed** (from the worker's history; wording differs from the original, facts do not to the best of its knowledge). Written to `/tmp` first and copied in.

Design and running record of the full-time QA agent for Arbos (`unarbos/arbos`): it uses Arbos to do ordinary work while trying to break it, records every attempt as a rollout, turns breaks into bug files, and acts as the fix agent (one PR per bug, each with a regression check). Project goals and the file-system-first principle: `docs/project-context.md`.

## Summary (10 lines) — reconstructed

1. A Python runner (`internal/qa/run.py`) starts `arbos-kernel serve` on a scratch folder and drives it over its attach socket (newline JSON frames), no desktop needed.
2. Scenarios are Python functions with a docstring and tags; today 204 (ordinary tasks, adversarial, benchmark items, inbox-note attacks, desktop under Xvfb, file-based plan, multitasking audit, remote/mesh, batch checks).
3. Every run is a rollout folder: frames sent and received, kernel stdout/stderr, `.arbos` snapshots before and after, `result.json` with the breaks.
4. Break detectors are of three kinds: scenario expectations, a consistency checker over the `.arbos` tree, and standing detectors (duplicate assistant lines, provider blocks, forbidden machines, restated replies).
5. A break becomes a draft bug (`internal/qa/bugs/<fingerprint>.md`), deduplicated by (scenario, rule); curated bugs are `qa-NNN-*.md` with repro, expected/actual, suspected location, PR link.
6. The features agent's notes in `internal/qa/inbox/` are a scenario source: each note becomes an `inbox:<feature>` attack scenario plus named follow-ons, run against the note's branch.
7. The headline scenario replays Jacob's twelve-item acceptance benchmark (`scenarios/kickoff-session.json`) and records which items pass in `kickoff-history.jsonl`. Since 2026-09-16 a second headline runs every cycle beside it: the acceptance journey (`docs/acceptance-journeys.md`, `journey_scenarios.py` → `journey-linux`), the whole path a user lives — create a project, a real challenge, follow-ups, steer/interrupt, leave and come back, the result on disk, kernel restart and a second project — scored per step J1..J8 in `journey-history.jsonl`; a step failing twice in a row becomes a named `qal-jNN` bug. The desktop and iPhone loops run the same definition.
8. The loop runs hourly (`deploy/cycle.sh`): build the tracked branch(es), run the library, run the voice harness, the desktop stack, the pod probe, nightly SWE-bench, publish to the `qa-results` branch. Cost is capped per day; model calls go through OpenRouter.
9. Fix agent: for the clearest bug, a branch off `main` (earlier `rust`), a fix, a regression test where one naturally fits, a PR ready for review. Nothing merges automatically; the gates below are proposed, Jacob decides.
10. Safety: the loop never touches production machines beyond its own directory, never auto-merges, never prints secrets, and (standing rule) never touches Jacob's `mac`.

## What exists today (audit, 2026-09-12, `origin/rust`) — reconstructed

### Tracing and recording
- `transcript.jsonl` per agent (`.arbos/agents/<id>/`): append-only events (`wake`, `user`, `assistant`, `tool`, `notice`, `ask`, `answer`, `turn_complete`, later `nudge`, `say`). The kernel tails it and broadcasts `event` frames.
- Provider traces per model call (`.arbos/agents/<id>/trace/NNNN-<ts>-L<line>.json`, `trace = true` in config): request, status, chunks, content, calls, usage. The link to the transcript line and the call ids came with the tracing PR (#14).
- Structured kernel log `.arbos/runtime/kernel.log` (JSONL: ts, level, event, agent, detail) — added by the tracing PR; before it, unstructured `eprintln!` only.
- Turn lifecycle, pid and version (`kernel_start` line, `turn_start`/`turn_end`) — same PR.

### Driving a session headlessly
- `arbos-kernel serve <place>` binds a loopback TCP socket and writes `.arbos/runtime/kernel.json` (`url`, `pid`, `version`, `git_sha`). A client connects, gets `hello` + `snapshot`, sends `user` (with `steer`, `attachments`, `channel`, `device`), `answer`, `approve`, `focus`, `history`, `rewind`, `kickoff`, `put`/`read`/`ls` frames; receives `turn`, `event`, `ask`, `error`, `working`, `rewound`, `changed`, `replayed`/`history_end`.
- `arbos-kernel run --steer`, `attach`, `answer` (CLI); `serve --provider replay --replies FILE` scripts the model for deterministic tests; `serve --bind` off loopback with tokens in `access.toml`; `--hub` registers with an `arbos-hub`.

### State that must agree (consistency rules, `internal/qa/consistency.py`)
Every agent folder has `agent.md`; `parent:` names an existing agent (no cycles); every transcript line parses; a `wake` is followed by `turn_complete`/`interrupted` (else `needs_serve()` refires it on every start); `kernel.json` pid is live while served; the lock file exists only while served; `plan.jsonl` (old engine) nodes are consistent; `subscriptions/*.toml` parse; no agent folder is a ghost (transcript without `agent.md`). Runtime files moved to `.arbos/runtime/` on `main`.

## Components — reconstructed

1. **QA driver** (`run.py`): `Kernel` (start/stop/kill, stderr capture, `extra_args` for the replay provider), `Client` (attach, `user`, `wait(pred)`, `wait_turn`), `Cx` (scratch place, XDG config with OpenRouter key from 1Password, scratch HOME), `Recorder` (rollout folder staged on local disk and copied to the store once at the end — the store is a FUSE mount, slow on appends). Scenarios: `@scenario(name, needs_model, tags)`.
2. **Scenario library**: core adversarial set in `run.py` (`boot-idle`, `prompt-no-key`, `second-serve`, `concurrent-sessions`, `rapid-create-delete`, `huge-input`, `malformed-folder`, `malformed-frames`, `kill-mid-turn-restart`, `ordinary-task`, `readonly-arbos`, `disk-full`, `clock-jump-cron`, `plan-shell-verdicts`, `restart-during-compaction`, `steer-storm`, `spawn-storm`, `secrets-leak-hunt`, `ask-identity`, `bench-tests-are-spec`, `bench-no-screenshot-unasked`); benchmark items (`bench-*`, one per failing checklist item); the headline `kickoff-session`; `inbox:<feature>` from notes; `desktop_scenarios.py` (Xvfb + the app's JSON driver); `fileplan_scenarios.py` (`fp-*`); `multitasking_scenarios.py` (`mt-01…29`); `remote_scenarios.py` (`rm-*`); `batch_scenarios.py` (`bt-*`, `fs-*`). `run.py --list`, `--only`, `--tag`, `--integration`, `--fileplan on|off`, `--budget-usd`.
3. **Break detectors**: scenario `expect(cond, rule, detail, where)`; `check_place()` consistency rules (`state:*`); `check_duplicate_assistant` (twice-in-a-row assistant text, and since 2026-09-16 `reply-restates-itself`); `check_forbidden_machines` (any tool call naming `mac`); `provider_blocked` (a 403/policy refusal makes the run `env:provider-blocked`, never a bug); `spawn_health` in `mt-*` (a made-up `host` refused, qa-031).
4. **Rollout recorder**: `internal/qa/rollouts/<UTC>-<scenario>/` with `frames.jsonl`, `sent.jsonl`, `driver.log`, `kernel*.stdout.log`/`stderr.log`, `state-before/`, `state-after/`, `result.json`, `scenario.json`; secrets scrubbed (`[QA-REDACTED]`, base64/hex/url-encoded forms too); `rollouts/index.jsonl`. Retention: passed rollouts go after 30 days, breaks stay.
5. **Triage**: `draft_bug()` writes `bugs/<fp>.md` once per (scenario, rule) with status `draft`, the rollout path, the detail, a repro line; later runs append "seen". A person or the fix agent promotes a draft to `qa-NNN-*.md`. `call-mode-collect.py`, `swebench-collect.py`, `pod-health.py` do the same for their sources.
6. **Fix-agent handoff**: the clearest bug per turn; branch `cursor/fix-<bug>-de28`; the fix; a regression test where a natural place exists (`crates/arbos-kernel/tests/*_e2e.rs`, unit tests, the voice harness's `tests/scenarios/*.toml`); a PR ready for review with the rollout cited. The scenario that found it stays in the library as the permanent regression check.
7. **PR and auto-merge policy (proposed, not enabled)**: gates — CI green (build, test, fmt, clippy), the parity suite green, the QA scenario that found the bug green on the PR's kernel, no protected path touched, a human `qa-fix` label. Protected: the auto-merge policy itself, secrets and access files, deletions above a size, `.github/`, release tags. Rollback: revert on `main` within one cycle when the next cycle's headline or a fixed bug regresses. Jacob decides when to enable; nothing has been auto-merged.
8. **Feedback**: every fixed bug's scenario remains and its fingerprint is recorded, so a regression re-opens the bug with "seen again" rather than a new draft.
9. **Inbox notes as a scenario source**: `internal/qa/inbox/<date>-<feature>.md` → `inbox:<feature>` (the note's "what could break" list is the attack plan, run against the branch the note names); `EXTRA_INBOX_SCENARIOS` maps a feature to library scenarios that also run against its branch. `internal/features-inbox/` carries proposals back to the features agent.

## On-disk layout — evidenced

```
internal/qa/
  run.py consistency.py desktop_scenarios.py fileplan_scenarios.py multitasking_scenarios.py remote_scenarios.py batch_scenarios.py
  scenarios/kickoff-session.json          the twelve-item benchmark replay
  inbox/<date>-<feature>.md               notes from the features agent (read, never edited)
  bugs/qa-NNN-*.md, ui-NNN-*.md, <fp>.md   curated bugs, UI-pass bugs, auto drafts
  rollouts/<UTC>-<scenario>/               one folder per run (see Components 4)
  kickoff-history.jsonl spend.jsonl call-mode-history.jsonl pod-health.jsonl swebench-history.jsonl
  deploy/cycle.sh vm-loop.sh publish.sh kill-shim.sh setup-arbos-qa-user.sh arbos-qa.service arbos-qa.timer
         swebench-nightly.sh swebench-collect.py call-mode-collect.py pod-health.py
internal/qa-cycle-2026-09-16.md            cycle notes written while docs/ was missing
```

## Running full time — reconstructed (deployment facts evidenced by `deploy/`)

- **Where**: designed for ArbosLife (`const@204.12.171.6`) under one directory (`~/arbos-qa`) and a user-level systemd timer (hourly). It ran there from 2026-09-12 23:53 UTC until the qa-020 incident (below), after which Jacob asked for a dedicated user; `deploy/setup-arbos-qa-user.sh` is the one-command setup and waits on him. Since then the loop runs on the QA worker's cloud VM (`deploy/vm-loop.sh`), which is **suspended while the worker is idle** (finding of 2026-09-14: both the kernel's log and the recorded frames showed the same two-hour hole; a `timeout 50m` does not help a frozen process). The coordinator's 90-minute timer now wakes the worker; the inter-cycle wait polls the clock in 30 s steps so a cycle starts within a minute of a wake.
- **Cadence**: hourly. `cycle.sh`: (1) build the primary kernel, (2) pull inbox notes, (3) run the library, (3a) tracked branches with `--integration` (and `--fileplan on|off` from the kernel source), (3b) inbox-note branches, (3b2) desktop stack under Xvfb on the QA VM only, (3c) voice harness + pod probe, (4) retention, (5) publish (`qa-results` branch: bugs mirrored with `rsync --delete`, history files, small rollout files).
- **Cost controls**: OpenRouter with `spend.jsonl` per scenario (tokens × price table) and a daily cap (`--budget-usd`, $10 on the loop; model scenarios skip once reached); `google/gemini-2.5-flash` since 2026-09-16 (OpenRouter blocks `openai/*` for the key: "user blocked for a previous policy violation"); SWE-bench nightly on `anthropic/claude-sonnet-5` with its own cap and a half-set stop; paid compute (Lium GPUs etc.) only when a scenario genuinely needs it and never without a budget line here first.
- **No duplicate floods**: bug fingerprint = (scenario, rule); a repeat appends a "seen" line; `env:*` rules never draft; the `PROVIDER_BLOCK` rule sets a whole run aside.
- **Docs mirror (standing job since 2026-09-16 09:55 UTC)**: the tail of every cycle runs `internal/mirror-docs.sh` (`REPO=$ROOT/repo`), which pushes the store's `docs/` to the orphan branch `store-docs` on `unarbos/arbos` when something changed (`internal/store-docs-mirror.md`). A non-zero exit is an alarm: the script refuses to push when the store view looks damaged or `docs/` is gone. `deploy/mirror-alarm.py` then records `store-mirror-history.jsonl`, names the documents missing against the branch, restores the branch into `state/store-docs-restore-<ts>/` (never into the store), and drafts `bugs/store-docs-mirror-refused.md`; the next QA turn checks the loss is real, copies back, and tells Jacob. Where the store is not mounted (ArbosLife) the step is skipped.
- **Mirror scope and boundary (2026-09-16 12:45 UTC, after the second loss).** The mirror covers `docs/`, `notes.md` and `internal/` — every file under `internal/` **except** run output and caches (any folder named `rollouts`, `staging`, `state`, `node_modules`, `.venv`, `__pycache__`, `target`, `.git`), binaries (images, audio, video, archives, compiled files) and files over 2 MB (`MIRROR_MAX_BYTES`). Protected: reports, inbox notes, bug files, scripts (`deploy/`, `mirror-docs.sh`, the parity rig), scenario code, history `.jsonl` files. Not protected: rollout bundles, screenshots, big logs, `media/`, `artifacts/` — their owners keep their own copy. The safety gate also refuses when `internal/` is missing or empty, and when the `internal/` set would shrink by more than a tenth (`MIRROR_ALLOW_SHRINK=1` overrides). The mirror runs at the **start and the end** of every cycle (`mirror_store start|end`), so the window between a write and its copy is under an hour; the alarm handler also fires (exit 3) when `internal/mirror-docs.sh` itself is gone and restores from the branch's copy. First widened push: `c2cf89b8`, 19 docs + 347 `internal/` files, 51 s (the store walk is pruned in `find` and hashed in one `git hash-object --stdin-paths`).
- **Pause switch**: `state/PAUSED-until-<UTC>` stops the loop (`qa_paused` in `vm-loop.sh`/`cycle.sh`); used 2026-09-13 21:52–23:50 UTC at Jacob's request.

## Incident 2026-09-13: the loop killed the production box's processes (qa-020) — reconstructed

`bench-screenshot`'s background job hit its `timeout_ms`; the kernel's `kill_job` ran `kill -9 -<pid>` without `--`, which procps parses as `-9 -9`… killing every process of the user `const` on ArbosLife (validator, dash, swarm). The loop was stopped; `kill_job` now uses `libc::killpg` and refuses pids 0/1 (PRs #73 into `rust`, #74 into #58); `deploy/kill-shim.sh` guards the box; Jacob asked for a dedicated `arbos-qa` user before the loop returns there.

## Safety — reconstructed

Never auto-merge (see gates); never merge or close PRs; never print a secret (values are scrubbed from rollouts, `env_stray_secrets` names variables only); never touch anything on ArbosLife outside `~/arbos-qa`; never `kill` without a process group or a pid check; scenario kernels run with a scratch `HOME`/`XDG_CONFIG_HOME` (no hub, no machines, no keys beyond the one model key); **standing rule (Jacob, 2026-09-13 22:09 UTC): no scenario, harness or voice test may attach to, spawn on, message or otherwise touch machine `mac` on the hub, nor `mac/.arbos` / `mac/misc-arbos`** — enforced by `check_forbidden_machines`, by the scratch-HOME rule, and by the voice harness's mock machine being `qa-mock` (PR #132); the pod voice gateway is Jacob's own phone path and is only redeployed, never called.

## Incident 2026-09-16/17: the loop deleted the Project Agent Store seven times (qal-j15) — evidenced

The inbox scenario hands a feature note to a live agent with "then attack it — try each of these". Note `2026-09-13-swebench-loop-cycle-1.md` listed `cd / && rm -rf *` as a `needs_approval` gap; the kernel does not catch it; the agent ran it on this VM fifteen times; from 09-16 09:02 the walk reached `/cursor/stores/<id>` — the first user-writable tree under `/` — and deleted 99–229 files a run, ≈1,300 on the 40-minute run of 09-17 06:52. We recorded it as a service fault for a day (`docs/store-fault-report-2026-09-17.md`, now corrected at the top). The FUSE client log on this disk (`/tmp/agent-store-fuse.log`) showed the `delete_files` bursts from this client at every episode; nobody read it until 07:35.

**08:13 the same day, the same command ran again.** The store was hidden by then (0 deletes from this client in the FUSE log) and the walk went on to the next user-writable tree: `home/ubuntu`. It took `~/arbos-qa/{repo,deploy,logs,state,staging,results,secrets.env,swebench,target-*}` and `~/.cargo`, `~/.ssh` before the reaper's new environment match killed it (`loop/`, `.rustup`, `go` survived). The first defuser had left the command in the prompt with "do NOT run this — only check whether the kernel asks"; the only way an agent can check that is to run it, and it did. Its rollout was in `staging/` and went with it. Nothing irreplaceable was lost: the repo is a clone, `deploy/` is mirrored in the store, the key is in 1Password, the FUSE log is under `/tmp`. The loop was stopped at 09:08 and restarted only after the wrapper also made `~` and `/workspace` read-only and the defuser removed destructive commands from prompts outright.

Rules from it, now enforced:

- **A kernel under test cannot see the store and cannot write anything of ours.** `deploy/ns-wrap.sh` starts every kernel (and the desktop app) in a mount namespace where `/cursor/stores` is an empty directory and `~` and `/workspace` are read-only, back at uid 1000; only `/tmp` (scenario places) is writable; `run.py` refuses to start a kernel without it (`ARBOS_QA_STORE_VISIBLE=1` is the only override, and it must not be set in the loop). Same to do for desktop-launched kernels once Electron in a nested namespace is checked.
- **An attack item that names a destructive command is a check that the kernel asks — never an instruction to the agent to run it.** Destructive commands are removed from a note's text before it reaches the agent (a marker says so), every inbox prompt opens with a standing rule never to delete outside the project folder even to test an ask, and approval paths are tested without a model (`ra-01`, to write). Telling the agent "do not run this, only check whether the kernel asks" does not work: it checked by running it (08:13).
- **A process the harness started is the harness's to kill, wherever its cwd is.** `reap_scratch` reaps by scratch path; the 06:52 `rm` had cwd `/` and outlived its kernel by 40 minutes. Teardown now also kills anything whose environment names the scenario's scratch `HOME`.
- **Read the client log before blaming the service.** The FUSE mount logs every RPC this client makes; a burst of `delete_files` from us is the first thing to look for when files vanish.
- **Stage only what you deliberately changed, from a tree only you write.** 12:07 on 2026-09-17: the loop's end-of-cycle publish, blind to the store, staged its 09:28 copies of 228 bug files into `store-pending/` — the same folder the hand edits were staged in — and replaced two verified bug files with older ones; the branch built from that folder carried the rollback to the relay, which held it on authorship. Two writers into one staging tree, and a sweep of the tree, is a rollback mechanism. Now: hand edits live in `hand-pending/` and only that tree is pushed; the loop's `store-pending/` never receives bug drafts while blind and never replaces a staged copy with one that does not contain it (`may_overwrite`, at staging as at applying).
- **A restore or a staged write goes over a file only when content says it may — time is not enough.** The mesh worker's check when it applied our six staged files (2026-09-17 09:50), now `may_overwrite` in `vm-loop.sh`: the store's copy must be contained in ours (ours is theirs plus additions; at most two of their lines changed), written through a temp name and a rename, read back and compared. Anything else is HELD in `store-pending/` and said out loud. A mount can lie about mtimes; content cannot, and "the copy I hold is a subset of what is there" would have stopped every bad restore of the night.
- **On this store, an absence is not evidence of an absence** (the coordinator's finding, 10:13: 13 files on the writer's mount, 3 on his, same minute, no error). A file you cannot list may be there; a second client or the writer's own log decides, never one client's listing — and nothing is deleted or restored on the strength of one.
- **A restore is safe only when you can name why no newer version can exist** — your own unedited copy, of a file nothing else writes. A mirror copied over a live tree is not that: our 07:01 restore wrote 06:41 versions over the phone loop's newer files (M-141) and could not bring back a file newer than the mirror (M-142). Restores stay staged-only; a restore of a *missing* file from the mirror is allowed and announced; an *existing* file is never overwritten.
- **The mirror never accepts a smaller tree without a reason.** A directory on the tip that does not list here refuses the pass; any fall in file count refuses it unless `MIRROR_ALLOW_SHRINK='<reason>'`. The old one-tenth allowance accepted a 454-for-492 view.

## Rewind and undo: the general property is asserted (2026-09-17 09:30) — evidenced (`rw-08`, `rw-09`, every `rw-*`)

After #419 (the seventh destructive bug around `restore()`), every rewind the `rw-*` scenarios send records the working tree, HEAD and index before, and if the restore reports an error, requires them unchanged after (`<name>-failed-restore-changed-the-tree`). `rw-08` forces the failure #419 names (the work-tree object made unreadable) and `rw-09` a `clean` that cannot remove a folder. Result: #419's two claims hold against a `main` control; the property does not — `reset --hard` still runs before `read-tree` can fail, so a failed restore moves HEAD and drops the person's later commits from the tree (`qal-j16`). The eighth was found by the property, not by reading.

Two measurement rules from the same hour:

- **A fix and its control never share a `CARGO_TARGET_DIR`.** Two worktrees built into one target directory gave a `main`-labelled binary with the PR's code (cargo reused the artifacts); `strings <binary> | grep <a string only the PR has>` is the check.
- **Neither the harness nor a kernel under test reads this host's global git config.** `run.py` and `cycle.sh` set `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1`. The host's `~/.gitconfig` signs commits with a helper under `~/.cursor/bin`; after the 08:13 wipe took `~/.cursor`, every `git commit` on the VM failed and every scenario that needs a HEAD silently had none — a scenario passing or failing on the host's dotfiles is not measuring the kernel.

## What does the pass prove? — a step in every scenario review (2026-09-17 09:45)

Three passes tonight proved something other than what they claimed: 29 scenarios that skipped themselves and read as green; a subscription check that matched the kernel's own echo of the input; and #419's first test, whose safety copy wrote the missing blob back because git stores by content. Each was found by someone asking what the pass actually proved. So the question is now a step, not an instinct. Before a scenario is trusted:

1. **Run it against a control that must fail**, and read *why* it failed — the break must name the mechanism, not a side effect (`ra-01` on `b133af2c`: the sentinel gone, the card shown; `rw-08c` on `main`: f2 gone, HEAD moved).
2. **Ask what else could make it pass.** Content addressing (a blob that comes back by being written elsewhere), a mock echoing the input, a skip scored as a pass, a wait that times out into the expected state, a check on the harness's own copy rather than the kernel's output. `rw-08` now asserts the corrupt object is *still* unreadable after the rewind.
3. **Ask what else could make it fail.** The harness's own git shim, sharing one capture file between two concurrent gits, produced a checkpoint sha with an email glued on and read as a kernel bug for ten minutes (`rw-08` at `0bceb0df`, first run). A break is reproduced before it is filed, and the rollout holds the evidence (`kernel-git.log`).
4. **A probe must fail the way the world fails, not a way of its own.** `rw-10b`'s first version stood a FIFO where `.git/index` should be; `open()` on a FIFO with no writer blocks forever, and the kernel's turn hung 200 s on every build. Nothing in a repository ever does that. A directory gives the same `stat` answer and blocks nothing. When a probe's failure looks nothing like the bug it stands for, suspect the probe first.
5. **When a message changes, ask who reads it.** User-facing wording is an interface: the desktop parses `place already served` to tell a lost spawn race from a crash; this loop's cost detector matched the spend-cap text and went blind for two cycles when it was renamed. A scenario that depends on a phrase says so in its break message, and a PR that changes a phrase names its readers.
6. **A pass in under a second is a question, not a relief.** `ra-01` passes in 1.0 s; the answer was fifteen recorded refusals, each naming its tree.

`ra-01` (the wipe guard, #410) and `rw-08`/`rw-08b`/`rw-08c` (the rewind property, #419) went through this before their results were recorded in `qal-j15` and `qal-j16`.

## The first-match family (opened 12:20) — evidenced (`fm-01`)

`qal-j19`'s third shape was a reader taking the first of several locations rather than the newest record. The author's audit lists four more such readers: the history lookup (id then name, live then archive), the checkpoint sidecar beside its journal, the roster's per-machine files, the leash pointer beside the job folder. The property, same shape as the rewind property: **stage a stale copy where the reader looks first and a live one where it looks second; the reader must take the live one.** `fm-01` staged the checkpoint sidecar — a cut turn's `checkpoints.d/<line>.json` beside a new turn's pending record at the same line, same HEAD — and the next rewind restored the cut turn's tree: a file the person had rewound away came back, the new session's file vanished, reported as restored (`qal-j20`, at `a5072074`). Found by the property on its first use; nobody had read for it. The other three readers are next.

## #441, a held place said once — verified 11:58 (`lk-01`…`lk-03`, kernel `b5b24dba`)

- `lk-01`, a real relaunch loop rather than a moved clock: 162 relaunches over 5.5 minutes, every one exit 3 with `place already served` on stderr; the place's `kernel.log` holds **6** `place_held` lines — one full (pid, build, url), four heartbeats, one error-level escalation at 301 s naming 148 refusals and the ways out. Where the pod saw 1411 lines in 32 minutes.
- `lk-03`: the holder killed → the next start serves, logs `place_freed`, removes the record; a new holder gets its own record (`refusals: 1`) and its own full line. No permanently-refusing state.
- `lk-02` at `38b2e145` (12:20): all three shapes pass; `qal-j19` closed.
- `lk-02` at `46477c88` (12:12): shapes (a) and (c) fixed — one long line with the temp fallback; six short lines saying the record could not be written when there is nowhere to keep it. Shape (b) remains: a stale `runtime/` record that cannot be updated shadows the live temp copy because `load` reads `runtime/` first — escalation on every relaunch. In `qal-j19`.
- `lk-02` at `b5b24dba`, the record's own failure (`qal-j19`): `runtime/` read-only → the full line **6 of 6** relaunches; with a six-minute-old record that cannot be updated → the error-level escalation **6 of 6**. `HeldRecord::save` is `let _ =`, and a save that fails is treated as done — qal-j09's shape one layer down, at error level, at the volume #441 exists to stop.

One rule kept from the PR, added to the review list below: **our user-facing wording is an interface for something.** The author kept the exact phrase `place already served` because the desktop parses it; the spend-cap rename silently broke this loop's cost detector for two cycles. When a message changes, ask who reads it — `lk-01` asserts the phrase on every relaunch for that reason.

## #432, the coordinator that slept on its workers — verified 10:45 (`co-01`…`co-05`, kernel `ec34d0e7`)

From Jacob's own report. Five scenarios, replay provider, a real spawned worker each time:

- `co-01` — `sleep 75; echo waited` with a worker running: refused in 0.1 s with *"while 1 worker(s) of yours run. Their reports wake you the moment they land … End the turn now … use await <job>"*; the worker's report starts the next turn and is answered.
- `co-02` — the refusal is no broader than the fault: `sleep 6` with no workers runs to its output; `sleep 3 && echo` with a worker runs.
- `co-03` — `for …; do sleep 1; done` with a worker runs, as the PR says. **Observation for the author, not a break:** the same wait spelled `sh -c 'sleep 8'`, `/bin/sleep 8`, `timeout 20 sleep 8`, and `true && sleep 8` all run with a worker. The PR's contract is the bare form and the prompt line carries the rest; if a model reaches for one of these the guard will not be what stops it.
- `co-04` — the yielding path, the one that can lose work: an attached twelve-second loop yields at 2.6 s when the worker's report lands, with *"Still running as job j1 … A worker's report landed while it ran — it follows this result"*; the job runs to the end (its `out.log` holds the output) and root is woken with *"job j1 exited with code 0 after 12s — `…` — log: …"*. Nothing dropped. (The probe's first version used a bare `sleep 12`, was refused, and never tested the yield; its second version matched the command's own arguments and "found" the result at once. Both caught by reading what the pass proved.)
- `co-05` — a worker that has reported and is done no longer counts: `sleep 6` runs right after the report and again after the archive.

Not measured: *answer the question in prose first* — a prompt-contract line, needs a live model; left to the kickoff journey's scorer.

## Rewind, closed out (10:25) — evidenced (`rw-08`…`rw-10c`)

`qal-j16` fixed at `0bceb0df`, its misreport at `5340c0d2`; `qal-j17` (the tree taken after the turn wrote) fixed at the source at `2daa555d` — `rw-10b` five runs, 0 lost, 0 wrong, where `5340c0d2` was 4 of 5 wrong. The fix's new face is `qal-j18`: the wait for the tree shows as the command running and is recorded as the command's time (`rw-10c`, six seconds of `echo`). Nine scenarios now stand on rewind: `rw-01`–`rw-04` (history), `rw-08`/`08b`/`08c` (the property under three failures), `rw-09` (a failed clean is said), `rw-10`/`10b`/`10c` (another git in the repository; the instant; the wait's face). Every one was run against a control that fails, and the failing run's reason read.

## The sandbox's own blind spot (2026-09-17 09:36) — evidenced (`/tmp/agent-store-fuse.log`)

The `ra-01` control (a kernel from before #410) ran `cd / && rm -rf *` inside the namespace. Store hidden, `~` and `/workspace` read-only, `/tmp` private: the host kept all of those. It did not keep `/run/agent-store-fuse/pod-grant` — a world-writable sticky directory holding a file owned by this user — and from 09:42 this VM's store client answered every listing **empty, with no error**, because every token mint returned 401 without the grant. This client's store is unusable until the grant is re-issued (it is minted by the platform, not by us); writes are staged under `store-pending/` and applied when it returns. Two rules: the wrapper hides every world-writable sticky directory the kernel does not need (`/run/agent-store-fuse`, `/run/user/<uid>`, `/var/tmp`, `/dev/shm`) — the list is in `ns-wrap.sh`, and a new one found is a new line there; and a store client whose credential is gone must say so (EACCES), not list nothing — that is the per-client empty view of the fault report, and it is worth its own note to the store's engineers.

## Phases (each shippable) — evidenced (recovered fragment)

1. **Done this turn**: runner, checker, 11 scenarios (8 no-model, 3 model incl. headline), 12 rollouts, 7 bug files (`internal/qa/bugs/qa-001` to `qa-007`), first fix PR with a repo test ([#7](https://github.com/unarbos/arbos/pull/7), `qa-001`). Headline scored 6/12 (items 1, 2, 6, 7, 10, 11 pass; `internal/qa/kickoff-history.jsonl`). Runs on demand on any machine with the kernel built: `python3 internal/qa/run.py --kernel <arbos-kernel> [--with-model]`.
2. **Loop on ArbosLife**: running since 2026-09-12 23:53 UTC (hourly timer, spend log, $10/day cap, results on the `qa-results` branch, `sync.sh` both ways). Since 2026-09-13 00:20 UTC each cycle also builds the branch named in each of the newest three inbox notes and runs that note's scenario against it. Five benchmark scenarios (`bench-*`) run every cycle. Fix PRs so far: #7 (qa-001), #10 (qa-003), #11 (qa-004), #12 (qa-007), #13 (qa-002), #14 (tracing gaps 1-3), #19 (qa-008), #20 (qa-009), #22 (qa-010), #24 (qa-005 + qa-006), #25 (qa-012), #26 (qa-013), #28 (qa-014). Attack surface widened 2026-09-13 01:00 UTC with `readonly-arbos`, `disk-full`, `clock-jump-cron`, `restart-during-compaction`, `steer-storm`, `spawn-storm`. Desktop attacks (`desktop-rapid-session-switch`, `desktop-kill-kernel-under-ui`, `desktop-huge-transcript-scroll`) drive the gpui app under Xvfb through its JSON driver (qa-015 → PR #31 for the Linux screenshot). Best kickoff then: 9/12 on `rust`. Release integration #58: the whole library ran against it 2026-09-13 06:48–07:20 UTC; kickoff 7, 7, 5 of 12, pulled down by qa-019 (fix #66 → #58), qa-017 and qa-018 (#68), #70. The cycle tracked #58 as a second branch until it merged.
3. **Fix agents and gates**: fix-agent prompt template, `qa-fix` label, CI workflow (build, test, fmt, clippy), QA gate job. PRs still merged by Jacob.
4. **Auto-merge**: merge bot with the five gates and the protected list, rollback rule, tags. Enabled by Jacob only.
5. **Kernel tracing additions**: structured kernel log, `error` frame, trace-to-transcript links (landed in #14).

## File-based agent model (decided 2026-09-13) — evidenced (fragment) + reconstructed

Scenarios for `notes.md` (the design said `plan.md`; #104 named it `notes.md`, both accepted) + `subscriptions/` + `inbox/` + `waiting/` live in `fileplan_scenarios.py` (`fp-*`): authored inbox file, shell and timer subscriptions, parked ask with an answer file, kernel-written `done` to the parent, the checklist, and a migration case (legacy `plan.jsonl` with a standing cron, a pending prompt and an open ask). Gate: the branch named by the inbox note that mentions `subscriptions/`, `--integration`, or `--fileplan on` (cycle.sh sets it when the kernel source has the engine). Kickoff item 9 accepts a `timer`/`shell` subscription with `every` (the kernel's own weekly `git gc` chore excluded). First results on #106 (2026-09-13): `fp-authored-inbox` pass; `fp-shell-subscription` runs with no model turn and the `notify` line reaches the user, but a hand-written file without `id`/`created`/`next_due` is dropped silently (qa-029); migration keeps the cron and the prompt but turns the open ask into a `notes.md` line (qa-028).

## Nightly SWE-bench slice (added 2026-09-13) — evidenced (fragment)

Arbos runs as a verifiers `Harness` (PR #94). `deploy/swebench-nightly.sh` runs the reference set of 16 SWE-bench Verified instances (`media/swebench/2026-09-13/results.json`, reference 12/16, $8.02) against the tracked head every night after the 02:00 UTC cycle: kernel image built from the head plus `harness/`, Sonnet 5 through OpenRouter, two halves with the second skipped when the first spends half the cap, 900 s per instance. `deploy/swebench-collect.py` writes `swebench-history.jsonl`, diffs solved→failed against the previous run into `regressions` with a bug draft each, and copies failing bundles to `rollouts/swebench/`. Triage of the first failures: qa-022 (rewrites existing tests), qa-023 (no login shell), qa-024 (killed job says "no exit recorded"); `bench-tests-are-spec` guards the first. Docker was installed on the QA VM for this. On the VM the slice only fires when a cycle spans 02:00 UTC while the worker is awake.

## Benchmark status (kickoff replay) — evidenced (`kickoff-history.jsonl`, 41 runs)

| Branch | Runs | Best | Last three |
|---|---|---|---|
| `rust` (early, recorded without a branch) | 3 | 9 | 6, 9, 9 |
| `cursor/release-integration-52cd` (#58) | 8 | 7 | 6, 7, 7 |
| `cursor/prompt-size-b027` (#119) | 10 | 7 | 5, 7, 7 |
| `main` (from 2026-09-13 23:50 UTC) | 14 | 11 | 10, 10, 10 |
| `main` + #219 (identical tree to `main` @ `3fff0130`) | 3 | **12** | 12, 11, 11 |

Scorer corrections (all in `run.py score_kickoff`): (1) 2026-09-13 21:20 UTC — items 2, 4, 5, 11 read the project store under `.arbos/` (`store_markdown`; a store file counts once it is no longer the bootstrap template), item 9 ignores the kernel's `git gc` chore; (2) 2026-09-14 00:30 UTC — items 3, 4, 10 read every agent's transcript (`all_agent_tools`), item 10 credits the `secret` tool, item 4 accepts two or more bare URLs as sources; (3) 2026-09-15 02:20 UTC — `agent_dirs()` reads archived workers under `.arbos/archive/agents/` (#144) for items 3, 4, 6, 7, 9 and every helper. Scores before each correction under-count by up to three. Harness fixes on the path to 12/12: the planted bug is committed (the fix is a real diff); the steer goal is sent while a worker is running (root waits on its workers, so a steer after root's `turn_complete` always found the worker archived); the design task is long enough to be steered. What the item failures taught, in order: qa-019 (`kind="default"`), qa-031 (`host="local"`), qa-033 (`say` to an archived worker), qa-034 (git guard judged the place, not `cd toy-repo`), qa-035 (`timer`+`cmd` refused), #212/#219 (the show-me image); with #217, #218, #219 in: 12, 11, 11 (the two misses model behaviour: a 2714-char reply; a research doc not written). Gemini 2.5 Flash since 2026-09-16: 10/12 on the first cycle; not comparable with the gpt-4.1-mini numbers.

## Open questions (with recommendations) — reconstructed

1. Where does the loop live full time? Recommend ArbosLife under `arbos-qa` (script ready); the VM sleeps.
2. Who promotes drafts to numbered bugs? The QA worker, once per triage pass, with its own prefix (`qal-`): two writers on one sequence collided at `qa-033`–`035` (the other writer is the features agent), and a shared counter needs a lock nobody owns. Decided 2026-09-16.
3. Auto-merge: recommend not before the parity suite and the QA gate run on every PR, and never for the protected list.
4. Model for the loop: recommend a cheap fast route (Gemini 2.5 Flash now) for the library and Sonnet for SWE-bench; the benchmark is model-sensitive, so score per model.
5. Cost cap: $10/day was hit by mid-day on 2026-09-13 once inbox scenarios multiplied; recommend $15 with inbox scenarios rate-limited to the three newest notes.
6. Desktop coverage: needs a built gpui app; recommend the QA VM keeps building it (X11 libs present) and ArbosLife does not.
7. Human-in-the-loop checks (Discord door, live call): recommend Jacob's one keystroke per feature rather than impersonating him; the `#arbos-qa` channel exists for it.
8. Store durability: recommend every writer builds files in `/tmp` and copies them in (this file does), after the 2026-09-16 loss.

## Tracing gaps that block good rollouts (3 most important) — reconstructed; all closed by #14

1. No timestamped structured kernel log on disk → `.arbos/runtime/kernel.log` (JSONL).
2. Provider traces not linked to the transcript line / call ids → filename carries the line, the trace carries `call_ids`.
3. No persisted turn lifecycle, pid and version → `kernel_start`, `turn_start`/`turn_end`, `kernel.json` with pid/version/git_sha.

## Bug index — evidenced (`internal/qa/bugs/`)

**Numbering.** Two writers minted `qa-NNN` from one unnumbered sequence, so `qa-033`, `qa-034` and `qa-035` each exist twice: the QA loop's (this worker, `bc-f2e2f30d`) and the features agent's (`bc-dcc57cf8`, found in the Mac wake-up incident and while verifying #195/#196). Both sets stand as filed. From here the QA loop's bugs are `qal-NNN-*.md` (continuing at `qal-040`); the features agent's carry their own prefix; the `ui-NNN` series is the UI pass. A reader who finds two `qa-034`s is looking at two writers, not a mistake in the record.

qa-001 no-key turn left unended (#7) · qa-002 live events stop after agent recreate (#13) · qa-003 large prompt starves the kernel loop, SIGINT ignored (#10) · qa-004 focus frame writes any path (#11) · qa-005/006 ghost folders for unknown agents (#24) · qa-007 SIGINT during a turn behaves like a crash (#12) · qa-008 empty prompt starts a turn (#19) · qa-009 plan accepts one-shot for a recurring goal (#20) · qa-010 search blocked without a provider (#22) · qa-011 read-only agent folder swallows the prompt (#14) · qa-012 file-size limit kills the kernel (#25) · qa-013 clock rewind silences a recurring node (#26) · qa-014 steers lost one per step (#28) · qa-015 desktop screenshot macOS-only (#31) · qa-016 model API key reaches the transcript via bash env (open; encoded forms too) · qa-017 turn resurrects a deleted folder (#68 → #58; back on `main`, #143) · qa-018 claim retries forever (#68) · qa-019 `spawn kind=default` refused (#66, #70) · qa-020 job timeout kills every user process (#73, #74) · qa-021 ask delivered twice, answer has no identity (#83-era fix; rule changed by ui-004) · qa-022/023/024 SWE-bench triage · qa-025 runaway background job outlives the kernel · qa-026 call mode: "Okay, wait, don't run that" approved (#111) · qa-027 barge-in drops the approval question (#111) · qa-028 migration turns an open ask into a checklist line · qa-029 hand-written subscription dropped silently · qa-030 fresh-place spawn race leaves a stale notice (#120) · qa-031 `spawn host=local` refused (#121) · qa-032 rewind leaves a dangling wake (fixed upstream; #168 made the cut atomic) · qa-033 `say` to a finished worker says no such agent (#213) · qa-034 git guard checks the place not the `cd` target (#217) · qa-035 `subscribe timer`+`cmd` refused (#218) · qa-036 bound port 502 on a plain GET (#234) · qa-037 remote `spawn wait=true` returns on the sync notice (open) · qa-038 remote kernel per spawn outlives the parent (open) · qa-039 store put conflict hides the hash; peer refusal names the wrong say target (open). Second-writer numbers: qa-033 orphaned job survives restarts, qa-034 paused agent overdue timer, qa-035 worktree worker cannot read the store. UI pass: ui-001…ui-013 (ui-013: `cargo test` in `desktop/` does not compile on `main`). CI CI flakes of one family — a check that infers a later step from an earlier one — fixed by the loop: #149 (`recreate_e2e` order race), #168 (`rewound` after a non-atomic cut), #170 (settle sleeps → polls), #313 (`goals_e2e` took the file's absence as proof of the wake). A fifth, found and fixed by the feedback worker: #335 (`fallback_403_e2e` grepped the raw transcript for `403` and matched millisecond clocks, one run in forty; it had been holding other workers' PRs, so some of 2026-09-16's unexplained CI reds were it). The rule all five feed is in `docs/project-context.md` under the codebase facts.

## The qal-j08 family — a write that fails silently, a record that looks valid, a destructive step that trusts it (hunt of 2026-09-17 04:40)

The shape behind qal-j08 (a checkpoint with no work tree because `commit-tree` failed for want of a git identity; `rewind --files` then `reset --hard` + `clean -fd`). The hunt: every place the kernel writes something it does not verify, or reads something and treats a failed read as *nothing* rather than *unknown*, then acts on it later. Census at `main` `0f2a8bc6`: 34 `let _ = fs::write/rename` sites and 12 `read_to_string(...).unwrap_or_default()` sites in kernel, engine and core. Ranked by what the later step does.

**Destructive — driven, filed, out tonight**
- **qal-j09** `notes::load` — one failed read of `.arbos/notes.md` parses as an empty page; the next `plan` call writes that page over the real one by tmp+rename. 293 → 36 bytes, "Set 1 item(s)." (`sw-01`.) Loud: the coordinator's page lives on the store mount that answers partially.
- **qal-j10** `undo` — the turn-start mark (`.arbos/runtime/checkpoint`) is written best-effort; when the write fails the mark keeps an older HEAD and `undo` does `reset --hard` + `clean -fd` to it, deleting committed work from kept turns; "restored <sha>". (`sw-02`.)
- **qal-j08** checkpoints without a work tree → `rewind --files` wipes kept turns' files. (`rw-04`.) Fresh-place state: a new user has no git identity.

**Destructive by code reading, not yet driven (same crate, same `unwrap_or_default`)**
- `memory::load` → `remember` rewrites `memory.md` with `fs::write` from the loaded text: a transient read failure with a working write wipes memory.
- `hooks::notify_user` → `user.md` rewritten from an empty read: the user's inbox log truncated to one line.
- `files::init_arbos_repo` → `.arbos/.gitignore` merged from empty: a hand's extra lines lost.
- `inflight::start` (documented best-effort): with the record unwritten — a full disk — a kernel death mid-tool re-runs the command; the qal-j02 guarantee returns to its hole silently.

**Second look at the misreport-only list (05:25):** one of them reaches a destructive step by a path not traced — `migrate.rs`'s `let _ = rename(plan.jsonl → .migrated)`: the rename is the only record that the one-time migration happened; when it fails the next start migrates again and a standing cron exists twice and fires twice, a pending task is queued twice. Fails on `main`, #390 and #392 alike → **qal-j12** (`sw-05`). The others hold as misreport after a second look: `jobs.rs` `killed`/`exit`/`settled`/`seen`/`notified`/`detached` markers change what `jobs` says, not what runs (`detached` unwritten means a background job dies with its kernel — a loss of the user's background process, but the leash's default, not a delete); `blocked.rs` — a 403'd family is retried (cost); `serve.rs` legacy `kernel.json` — a stale port; `host.rs::remember_place` — the recent-places list rewritten from an empty read (the opener forgets places; minor loss). qal-j09 and qal-j10 closed against #392 @ `a0f2a92d`; the ordinary `undo` still undoes (`sw-04`).

**Misreport only**
- `plan.rs` `meta.toml` rewritten as its tail after a failed read; `jobs.rs` `killed`/`exit`/`settled` markers with `let _` — a job reads as still running for ever; `blocked.rs` rename ignored — a blocked model family forgotten; `worktree.rs` exclude write ignored — `.arbos` may get committed (then `restore` refuses, safely); `focus` rewritten to root on an invalid read (by design).

**The fresh place is the vulnerable state** for qal-j08 (no identity) and for anything reading the store before its first write; every new user is in it, and the rigs reach it only through J1. The pattern to fix once rather than ten times: a read helper that distinguishes `NotFound` (empty is right) from every other error (unknown — surface it, never rewrite), and a rule that a destructive git step (`reset --hard`, `clean -fd`, `rename` over a file) runs only on a record whose write was confirmed.

## Where this rig could produce a number nobody should believe — audit of 2026-09-17

Prompted by the SWE-bench loop finding its containers had full network access and its agent had downloaded the upstream fix; the general fault is a rig that quietly produces a pass it did not earn or reports a result it did not establish. The harness read with that eye:

**Found and fixed the same hour**
1. **Twenty-nine scenarios could set themselves aside and read as `pass`.** Any scenario that wrote `notes["skipped"]` and returned — no desktop binary, no `arbos-hub`, no chrome, a feature replaced, a path not exercised — had zero breaks and was printed `[pass ]`, counted as a run, and never named in the skipped list. A cycle with no desktop build would have shown every desktop scenario green. Now `status = skipped` with `reason: self: …`, printed on the line and counted with the other skips.
2. **`sb-01`'s first version passed on the definition alone.** It waited for any frame containing `started-bg`; the kernel's `snapshot` echoes the subscription file, whose `cmd` contains that string, so the check matched in 0.0 s before the command ran. Fixed to require a delivered line (`bg: started-bg`) on a non-snapshot frame — and with that the real hold on #377 appeared. Rule kept: a marker looked for must be one the system can only produce by doing the thing, never one the scenario itself wrote into the input.
3. **J7 could be earned by editing the test.** "Tests pass" ran the project's own `unittest`; a worker that changed `assertEqual(area(3, 4), 12)` would have passed. J7 now also requires the seed's test file untouched and computes `area(3,4)`, `area(5,6)`, `perimeter(3,4)` itself. Past rollouts snapshot only `.arbos/`, so earlier J7 passes cannot be re-checked for this; from here they are.

**Known and stated, not fixed**
4. **The kickoff scorer counts claims.** Items 3 (screenshot), 4 (linked research), 10 (vault lookup) are scored from tool-call counts and URL patterns in the coordinator's or workers' transcripts — a `screenshot` call that returned nothing, a URL printed without being fetched, a `secret list` that found nothing all score. The 12-item number is a proxy for behaviour, not proof of outcome; it is reported as "items whose shape appeared", and this line should travel with it.
5. **`rp-01` is vacuous on a kernel without the knob.** `ARBOS_TEST_NO_CHILD_WAIT` is ignored by kernels before #371, so on them the scenario passes without testing anything; there is no way to detect the knob's presence from outside. Its value is against a regression on kernels that have it.
6. **J8c is not established here.** The Linux rig takes the dropped-connection verdict from the phone loop's history and says so in the evidence; a `pass` there is the phone loop's.
7. **Model scenarios have the network.** The kernel's `bash` reaches the internet during the journey and the kickoff replay. The toy projects have no upstream to copy from, so no pass can be bought that way today; a future benchmark with a public answer would need the SWE-bench loop's remedy (no network in the worker's sandbox) before its number means anything.
8. **The rig cannot see macOS** — the section below.

The rule that comes out of 1–3: every scenario says which of three things it did — established the fact, could not check it (`unverified`/`skipped`, with the reason), or found it false — and nothing else is allowed to read as green.

## What this rig cannot see — read this before trusting a green cycle (2026-09-17)

The QA rig is Linux (Ubuntu VM, Xvfb, the gpui app under a software Vulkan, `dunst` for notifications). Linux was chosen because bundled fonts make rendering comparable across machines, and that reasoning was sound for rendering. It does not extend to the rest. Jacob runs Arbos on a Mac, and in one week he found two faults himself that no cycle here could have seen — the desktop's lock race on first spawn (qa-030) and, on 2026-09-17, a kernel deaf to its own children: every job exited cleanly, none was reaped, every long command came back "still running" at the 600 s floor, five workers hung. The mechanism was macOS-specific (a signal-driven child reaper versus Linux's `pidfd`), so a Linux kernel with the same code was never wrong. A green cycle here means: *on Linux, the checked paths held.* It does not mean the app works on his machine. Classes this rig cannot see:

- **Platform process semantics.** Child reaping, `SIGCHLD` delivery and inherited signal masks, `waitpid` races, process-group behaviour on kill, what `exec` inherits from a launcher (a GUI app on macOS is launched very differently from a process under `tmux`). Since #371 the kernel has a fault-injection knob (`ARBOS_TEST_NO_CHILD_WAIT`) and `rp-01`/`rp-02` drive the exit-file path with the reaper disabled and with `SIGCHLD` blocked — that catches *this* fault on Linux, not the class.
- **Signals and lifecycle from the platform.** App Nap, sleep/wake, the window server closing a socket, Gatekeeper/notarisation prompts, launchd, a Mac's clock jumping on wake.
- **Native dialogs and permissions.** File pickers, the screen-recording and accessibility prompts (`screencapture` needs both), Keychain, the notification centre's own rules (dunst is not Notification Center), the menu bar, ⌘ shortcuts that gpui maps to ctrl here.
- **Filesystem semantics.** APFS case-insensitivity, extended attributes and quarantine flags, `mtime` granularity, iCloud-backed folders, the Mac's `~/Library` paths versus XDG.
- **The real network.** The kernel is local on this rig; a dropped link (J8c) is checked only by the phone loop.
- **Rendering on a Retina display**, scaled captures, and the GPU path the Mac actually uses (this rig renders through `lavapipe`).

What to do with that: the desktop loop's Mac rig is the only place those classes are checked, and it should run the journey and the `rp-*`, `sq-*`, `im-*` scenarios on macOS every cycle; a fault Jacob finds that a Linux cycle passed is, by default, in one of the classes above and is filed with that note; and no report from this loop says "the app works" — it says which paths held, on Linux.

## Acceptance journey (added 2026-09-16 13:50 UTC) — evidenced (`journey-history.jsonl`, 4 runs)

Definition in `docs/acceptance-journeys.md`. First scores on `main` `c964294c`: 4/8, 6/8, 5/8, 6/8 (the first two dips were harness misreads, corrected the same hour). Always unverified here: J6 notifications (the driver shows no unseen count) and J8c dropped connection (no link to drop; the phone loop owns it). Findings: `qal-j01` (the `status "…"` line drawn as a reply bubble), `qal-j02` (a command in flight when the kernel dies runs twice). Recorded, not scored: the app lands on the home tab at every launch, so a user who reopens and types straight away posts into `~/.arbos`. The cycle log carries `-- journey: N/8 …` and the ten-run pass rate per step.

## First-run behaviours (2026-09-16 10:50 UTC, `main` `c964294c` and `main`+#298)

`batch_scenarios.register_first_run` (`fr-*`) on the local provider stub with vendor-prefixed model ids (`acme/blocked` 403, `zeta/open`, `acme/silent` no first byte, `zeta/empty`): the kernel's family logic keys on the `vendor/` prefix, so unprefixed ids never mark a block. With #298: `fr-01` — the kickoff probes the key, the greeting comes from `zeta/open` after one plain sentence ("This key cannot use acme models … Pick another default in Settings › Model"), the block lands in `runtime/blocked-models.json` and the next turn skips the family up front — pass; `fr-02` — a primary with no first byte is given up after `first_byte_ms` (5 s in the test) and the fallback answers in 6 s — pass (skipped on kernels without the field). Without #298 (`main`): the raw refusal is quoted, the block is not remembered, the blocked family is retried every turn. Both kernels: `fr-03` — a first turn whose model returns nothing ends with no words and no notice (**qal-040**); the kickoff correctly does not count as taken.

## Running record, in order — reconstructed unless a file is named

- **2026-09-13 17:05 UTC, call mode owned by the loop.** The voice gateway's scripted harness (`voice-server/tests`, mock duplex model + mock kernel, no GPU) runs every cycle (step 3c; `call-mode-collect.py` → `call-mode-history.jsonl`, `bugs/call-<scenario>.md`). The pod's escalation log is mirrored to `internal/call-mode-escalations.jsonl`; each mined phrase becomes a `_DRILL` pattern and a line in `drilldown-mined-phrases`. Safety attacks: four scenarios red on #100's head, green after #111 (qa-026, qa-027); the pod gateway was redeployed with the fix at 17:24 UTC. Not in the cycle: `tests.desktop_call` (needs a desktop built from #71 + #101) and `tests.live_call` (needs the phone token and a human).
- **2026-09-13 20:40 UTC, multitasking audit.** The 26 items of `inbox/2026-09-13-multitasking-audit.md` as `mt-01…26` (kernel-level over the attach socket, heartbeat via a local silent OpenAI-compatible stub; desktop items via the driver; item 22 → `fp-waiting-ask`); step 3b2 builds the #105 desktop stack. Most delegation items first tripped on qa-031; `spawn-host-refused` names it. Standing-pass scenarios (2026-09-14, PR #140): `mt-27` three children / zero grandchildren, `mt-28` a fork claims no worker, `mt-29` `rewound` under 100 ms (found qa-032).
- **2026-09-14, overnight triage.** Harness staleness after the v0.2.0 merge fixed (`runtime/` paths, plan-node checks gated by `has_plan_engine`, trailing `nudge` allowed, `ask-identity` follows ui-004); the VM-sleep finding.
- **2026-09-14 07:50 UTC, Discord door (#153) live.** Bot "Arbos" has Manage Channels in Jacob's Arbos guild; channel `#arbos-qa` (`1548963686096314558`) created; a #153 kernel with `doors.toml` (`token = "env:ARBOS_DISCORD_TOKEN"`, `every = "3s"`) polls it. Verified: bot messages never wake root; the token is nowhere in transcript, kernel log, stderr or traces; the door survives connection resets. The human-message half (wake, reply once, re-wake, restart no replay, `mention_only`, redaction) waits for a human keystroke in the channel (`/tmp/doors-check.py` runs the second half).
- **2026-09-14 14:00 UTC, pod 502.** Cause: the kernel dropped plain HTTP GETs on its bound port; WebSocket attach through the tunnel worked (qa-036 → #234). `deploy/pod-health.py` probes the kernel (WebSocket), voice gateway (`/healthz`) and hub every cycle.
- **2026-09-15, kickoff to 12/12.** See Benchmark status.
- **2026-09-15 16:00 UTC, batches 44/45 (`848e0039`).** `rm-01` remote install over loopback ssh works end to end (qa-037, qa-038 found); `rm-02` update replaces an older build; `rm-03` kickoff turn on a fresh place; `rm-04` project face in `hello` and the hub roster, `changed project.toml`, plain GET 200.
- **2026-09-16 04:15 UTC, model route change** to Gemini 2.5 Flash; `env:provider-blocked`.
- **2026-09-16 07:40 UTC, batch on `43d8569d`.** `bt-01…08` on the replay provider: `say to=user` refused (#289), spawn guard reads the brief (#285), worktree re-spawn `fixer-2` (#286), tool markup stripped (#278), history pages backwards (#272), 403 falls through to `fallback_models` (#283), archived worker visible (#287), image as bytes (#270) — all pass.
- **2026-09-16 09:10 UTC, mesh federated store (`b49e6163`).** `fs-01`: read/ls/put/CAS/root-owned refusal/`..`/unknown machine all hold; qa-039 for two edges. Cycle notes: `internal/qa-cycle-2026-09-16.md`.

## The cycle as it runs on `qa-vm2` (2026-09-18 06:00) — supersedes the step list above

The loop moved machines on 2026-09-17 12:30 and the cycle changed under this document overnight. Where
the two disagree, this section is what runs; the reasoning for each line is in `internal/qa/bugs/qal-j21`
to `qal-j27` and in `internal/qa-loop-second-machine-2026-09-17.md`.

| the sections above say | it now does |
|---|---|
| the cycle runs the library | the tracked step runs **one half**, alternating, and says which: `== library half B: 151 of 291 scenarios, headlines in both`. The whole library needs ~162 min of measured work (291 scenarios, 33.5 s mean) against a 100 min cap, so it reached ~62% and registry order decided which — silently, until the truncation alarm |
| steps 3, 3a, 3b, 3b2, 3c | a **3a2** between 3a and 3b: the `uw-*`/`af-*` family on its own 25-minute invocation (`--tag unchecked-write`), because they register last and a capped step loses its tail — `af-04` was number 291 of 291 |
| `timeout 50m` on the tracked step | **100m** (`ARBOS_QA_TRACK_TIMEOUT`), 80m on desktop, and `exit 124` is a `!!` alarm naming what the cut cost |
| the desktop driver comes from `internal/parity` when present | from **the app's own commit** (`$wt/desktop/driver`), copied to local disk and the commit printed. The store copy is a fallback only: it was five hours stale and lacked the fields the app had begun reporting (`qal-j23`, `qal-j25`) |
| — | the two **headlines run in both halves**. A half without the kickoff replay or the acceptance journey would be a cycle measuring less than it reports |
| — | durations are **monotonic**. A wall clock on this paused VM reported a 1528-second UI stall that never happened (`qal-j26`); the guest's uptime advanced 3.65 h across 6.28 h of wall clock |
| — | `publish.sh` **refuses** a push that would delete bug files the branch holds, unless `ARBOS_QA_ALLOW_BUGS_SHRINK` names a reason (`qal-j21`) |
| — | every rollout and index line records the kernel's own `--version`, and a `--kernel-branch` naming a sha the binary does not carry is an alarm |
| J6 notifications "always unverified here" | **established**: the app posts, dunst receives it, the badge clears. A *post* is now distinguished from a failed *attempt* — the window's `posted` list carries an `error` per entry and the rig was counting attempts |
| the ledgers are `vm-*.jsonl` | per-machine via `ARBOS_QA_MACHINE`, so two loops cannot overwrite each other's runs |
| the inbox scenario waits 300 s for the turn | it keeps the ceiling but gives up after **45 s with no frame at all**, and says which of the two ended it. Four sat the full five minutes on 2026-09-17 |

### Six additions to the review list, earned overnight

6. **An assertion must not bound a race.** Batching, timing and ordering that the product does not
   guarantee must not be asserted; assert the property the optimisation exists for, or make the kernel
   enforce the bound and then assert it. A test that is right about the intention and wrong about the
   mechanism goes red with nothing it names being wrong.
7. **A script must not depend on which of two things happens first**, unless that ordering is the thing
   being asserted — and then it is asserted explicitly, so a failure names the ordering rather than dying
   of a line that never came.
8. **A failure message must be computable when the assertion passes.** Python builds the message argument
   before `expect` looks at the condition, so anything the message indexes, pops or unwraps has to be safe
   in the passing case. `af-03` ended with `f"… {wrong[0]!r} …"` where `wrong` empty *was* the pass, so the
   scenario raised `IndexError` exactly when it succeeded and printed `driver-exception` for four
   consecutive cycles while the product was behaving correctly — `qal-j29`. Two consequences worth holding:
   a `driver-exception` is a rig fault until proven otherwise, and it is **more** urgent than a product
   break, because it hides its own scenario's finding instead of reporting one.
9. **Run the path that has no history.** A first run, an empty directory, a missing file, an
   assertion that passes — the states where there is nothing yet get written from imagination and
   tested from the steady state. Two of today's faults are the same shape: `qal-j32` killed a whole
   cycle because `VAR=$(cat missing 2>/dev/null)` takes `cat`'s status under `set -e`, so the `*)`
   branch written for the first cycle could never be reached; `qal-j29` raised `IndexError` building
   a message out of an empty list, which is what the passing case *is*. In both the quiet path was
   the broken one, and in both a single run from the absent state would have found it.
10. **A driver that types must prove the app took the keystrokes.** Clicking and typing is an
    instruction, not an observation. The app says whether the composer is focused and what it holds
    (`desktop/src/driver.rs:1273`); a helper that reads neither can only report that the words are
    not where it looked, which is equally true of a stale selector and of real data loss. `xp-01`
    printed `first-line-lost` — the loop's most serious rule — for five consecutive cycles because
    one click landed on a window not yet taking input (`qal-j33`). The first click after launch
    never focuses; the second does.
11. **Edits live in the store or they do not live.** `vm-loop.sh` copies every scenario module and
    `deploy/` script from the store at each cycle start, so a local edit is erased at the next cycle
    boundary. This has cost work twice: the step-3a2 addition to `cycle.sh`, and the `xp-01` repair
    above, which was overwritten about six minutes after it was proved. Write the store copy in the
    same breath as the local one, and when a script is *running*, write only the store copy — a file
    bash is executing must not change under it (the cycle 2 syntax death).

### And the hardest lesson of the night, which belongs with step 2 of the list

**Ask the boundary question of the reading side too.** `qal-j27` was filed as data loss on the strength of
three scenarios agreeing, with four neighbouring contracts passing on the same build to rule out the
writing side. All three read `root` while typing into a sub-chat, which `new-subchat` gives its own kernel
agent. Three scenarios sharing one wrong assumption is one fault counted three times, not corroboration.
The question that would have caught it: **does the probe read what it wrote?**
