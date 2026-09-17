> **RECOVERED COMPLETE.** 8,154 bytes, which matches the store's last recorded size for this file exactly (2026-09-15 18:25 UTC listing; last modified 2026-09-15 00:26 UTC and not touched afterwards). Source: a full `read_file` captured in the Cursor-Projects-research worker's transcript at 2026-09-15 17:11 UTC. The content below is believed byte-identical to the lost original.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

# Appendix: every tool the Cursor Project coordinator has, parameter by parameter

Companion to `cursor-coordinator-spec.md`. Written 2026-09-14 by the coordinator from its own tool definitions. Parameters marked `?` are optional. Arbos's root agent should have the same set with the same semantics; the process-parity worker maps each row to an Arbos tool or files the gap.

## Agents

**CreateAgent** — `prompt` (kickoff, the worker's first user message), `name?` (display name, derived from prompt if omitted), `model?` (model slug; inherits mine if omitted; an invalid slug fails), `subagent_type?` (omit for a worker; `explore` | `computerUse` | `videoReview` runs a short-lived inline helper on my machine and blocks until it returns), `machine?` (worker only; one of: `{type: new_cloud_vm, base_branch?, environment_build_id?}`, `{type: same_vm}` (shares my checkout; told to use a git worktree), `{type: self_hosted_worker, worker_id}`, `{type: self_hosted_pool, pool?, labels?}`). Returns the worker id at once; the worker runs asynchronously and its turn ends arrive as system notifications with its last output.

**SendToAgent** — `agent_id`, `title` (short label of this turn, shown on the worker's task card), `message` (full instructions), `delivery?` (`steer` default: inject into the running turn, falls back to queue when idle; `queue`: next turn), `rename?` (durable display name, only when the task changed). Result says how it was actually delivered.

**GetAgentStatus** — `agent_ids?` (omit for all). Non-blocking. Per worker: lifecycle running / idle / errored / archived, turn in flight or not, last turn status, PR URL.

**ReadAgentTranscript** — `agent_id`, `mode?` (`tail` default | `full`), `max_turns?` (tail, default 10, max 50). Inline output capped; the full transcript is also written to a file whose path leads the result.

**StopAgent** — `agent_id`. Aborts the current turn; the worker stays available. Idle stop is a no-op.

## Files and search

**Read** — `path`, `offset?`, `limit?`. Reads text, images (rendered to me), PDFs (as text). Line numbers are metadata.
**Write** — `path`, `contents`. Overwrites.
**StrReplace** — `path`, `old_string` (must be unique unless `replace_all`), `new_string`, `replace_all?`.
**Delete** — `path`.
**Glob** — `glob_pattern`, `target_directory?`. Sorted by modification time.
**Grep** — `pattern` (ripgrep regex), `path?`, `glob?`, `type?`, `-i?`, `-A?/-B?/-C?`, `multiline?`, `output_mode?` (`content` | `files_with_matches` | `count`), `head_limit?`, `offset?`.
**EditNotebook** — `target_notebook`, `cell_idx`, `is_new_cell`, `cell_language`, `old_string`, `new_string`.

## Shell

**Shell** — `command`, `description?` (5–10 words), `working_directory?`, `block_until_ms?` (how long to wait before backgrounding; 0 = background at once; then the output goes to a terminal file with pid, cwd, last command, exit code). Long-lived or interactive work runs inside a named tmux session. Shell state persists across calls.
**AwaitShell** — `shell_id?`, `block_until_ms?` (max ~119 min), `pattern?` (regex to wait for in the output). Polls a backgrounded job; used only when the next step is blocked on it or the job needs close monitoring.

## User channel

**SendMessage** — `message`. The only channel the user sees; ordinary assistant text is hidden.
**UpdateCurrentStep** — `current_step` (six words or fewer, starts with a verb). The timeline step shown while I work; always sent alongside another tool call.
**TodoWrite** — `todos[]` of `{id, content, status: pending | in_progress | completed | cancelled}`, `merge` (true to merge by id). A visible checklist for multi-step work.
**SwitchMode** — `target_mode_id` (`agent`), `explanation?`. Plan, ask, and debug modes exist but I cannot switch into them here.
**CreateGoal / UpdateGoal** (dynamic, `cursor` namespace) — long-lived goals; used only when the user asks.
**GenerateImage** (dynamic) — image generation.

## Repository

**ManagePullRequest** — `action` (`create_pr` | `update_pr` | `post_comment` | `resolve_comment` | `get_ci_status` | `set_pr_status`), `title?`, `body?`, `branch_name?`, `base_branch?` (defaults to the run's preferred base), `draft?` (default true; `update_pr` with `draft: false` marks ready), `pr_url?`, `comment_id?`, `in_reply_to?`, `path?`, `line?`, `start_line?`, `side?`, `status?` (`open` | `closed`), `skip_branch_prefix_check?`. Artifact paths in the body are uploaded and rewritten to public URLs. PR templates are honoured. Open/close and comments only on explicit request.
**EditPullRequestLabels** — `pr_url`, `add_labels?[]`, `remove_labels?[]`. Only when asked.
**SetActiveBranch** — `path`, `branchName`. Tells the UI which branch's diff and PRs to show.

## Subscriptions (dynamic, `cursor-subscriptions` namespace)

`subscribe_github_pr`, `subscribe_github_ci`, `subscribe_slack_thread`, `subscribe_slack_channel`, `subscribe_slack_new_channels`, `subscribe_timer`, `list_subscriptions`, `unsubscribe`. A firing opens a turn for me with the event payload. This replaces polling for CI, PR activity, chat, and time.

## Web and dynamic tools

**WebSearch** — `search_term`, `explanation?`. **WebFetch** — `url` (read-only, no auth, no private hosts).
**GetDynamicTools** — `namespace?`, `toolName?`, `pattern?`. Discover MCP and first-party dynamic tools and their schemas; always before calling one.
**CallDynamicTool** — `namespace`, `toolName`, `arguments`, `mcpDetails?{description}`.
**FetchMcpResource** — `server`, `uri`, `downloadPath?`.

## Screen

**RecordScreen** — `mode` (`START_RECORDING` | `SAVE_RECORDING` | `DISCARD_RECORDING`), `save_as_filename?`. For GUI demos only. Screenshots the chat can render live under `/opt/cursor/artifacts/screenshots/`.

## Environment diagnostics (dynamic, `cursor-cloud` namespace)

`run-info`, `list-cloud-agents`, `environment-info`, `get-events`, `get-message-queue` (pending follow-ups queued for me), `batch-fetch-details` (other agents' transcripts and logs to files), `get-automation`, `list-self-hosted-workers` (how I find the user's own machine as a placement target), `list-environment-builds`, `environment-build-logs`, `trigger-environment-build`, `propose-environment-json`, `take-environment-snapshot`, `check-environment-snapshot`, `request-environment-setup-actions`.

## Skills

A list of skill files (paths to `SKILL.md`) is available; when one matches the task I read it first and follow it. Examples in this environment: env-setup, subscribe (wait for events instead of polling), walkthrough-artifacts (screenshots and recordings that prove a change), check-compiler-errors, fix-ci, loop-on-ci, fix-merge-conflicts, get-pr-comments, review-and-ship, new-branch-and-pr, deslop, verify-this, control-ui, control-cli. Arbos's equivalent is its `skills` feature (#42): a folder of instruction files matched by description.

## What is in my context every turn, beyond tools

- User info: OS, shell, workspace path, git repo flag, date, terminals folder.
- Git status snapshot at conversation start.
- Always-applied workspace rules and user rules (writing style, secrets via doppler/1Password, no unrequested docs or tests, `uv` for Python, the user's name).
- The Project prompt: my role, the turn loop, delegation and `notes.md` rules, store placement, media handoff, memory, communication (written up in the spec).
- The current agent's store path (the project folder every agent shares).
- Attached files and images from the user, saved to disk with paths I can pass to workers.
- System notifications: worker turn ends (with the worker's last output), subscription firings, queued-message hints.
- A running summary of the conversation after compaction, so a long project survives context resets.

## What I do NOT have

- No direct access to the user's machine (I am in the cloud); the user's machine appears only as a self-hosted worker placement.
- No blocking wait on a worker other than a typed inline helper.
- No ability to see or change another agent's system prompt.
- No memory across projects except the user store files I write.
