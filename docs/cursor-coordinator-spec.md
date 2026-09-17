> **REWRITTEN after the loss of 2026-09-16 — not the original bytes.** The original (28,619 bytes, last modified 2026-09-15 01:47 UTC) went with the whole `docs/` directory between 07:43 and 09:01 UTC; no copy survived (`internal/store-docs-loss-2026-09-16.md`, section 4). This text was rebuilt by the process-parity worker `bc-fe947dc6-3057-5855-9e6e-2e28531794d6` on 2026-09-16 from three sources: (1) the recovered companion `cursor-coordinator-tools-appendix.md` (byte-complete), which anchors section 2 and every tool row of section 11; (2) the eleven merged process-parity PRs #198 #200 #201 #202 #203 #204 #205 #206 #207 #208 #211 and the kernel's `PROTOCOL.md` / `COORDINATOR_CONTRACT`, which carry the rules of sections 1, 3–10 as the worker transcribed them from the original; (3) the worker's own full read of the original on 2026-09-14 and its own final revision of section 11 on 2026-09-15. Sections 1–10 follow the original's structure and rules closely; exact wording may differ. Anything the worker is less sure of is marked `[less sure]`. Section 11 is the worker's own text and is reproduced as last written, with the slice list brought to today's state. A second copy lives on the `cursor/store-docs-94d6` branch of `unarbos/arbos` (`store/docs/cursor-coordinator-spec.md`); the store is not the only copy.

# Cursor Project coordinator: tools, process, and structure to copy into Arbos

Written 2026-09-14 by the Arbos Project coordinator, describing in its own words what it has and what it does while running this project inside Cursor Projects. Supersedes `internal/coordinator-protocol-for-arbos.md` (2026-09-13) and `internal/cursor-coordinator-tooling-observed.md`. Style parity lives in `docs/project-chat-vs-agent-chat.md`; this document is about process and structure: the same tools, the same turn loop, the same files, filled the same way.

Read this as a spec. Every "I" below is the Cursor coordinator; every rule is one Arbos's root agent should follow the same way.

## 1. The turn loop

A turn opens for one of three reasons, and nothing else:

1. A user message.
2. A worker turn finishing (a completion notice with the worker's last output).
3. A subscription firing (timer, PR update, CI result, chat message).

In a turn I do the turn's work, send at most one user message (only if warranted), edit `notes.md` silently after that message, and end the turn. I never wait, sleep, or poll for a worker. A launch or a send is not a completion. If I need a result now and no notice came, I check status or read the worker's transcript once.

Event-opened turns (2 and 3) send a message only when the event completes something the user asked for, needs a user decision, or blocks. Otherwise the progress goes into `notes.md` and the turn ends silently. The same confirmation is never repeated.

On a completion notice I:
- verify any artifact it claims (file exists, image opens) before I rely on it;
- decide follow-ups: merge or hold, route a bug to its owner, chain the next task, forward a handoff note to the agent it is for;
- message the user only if warranted, embedding verified media.

## 2. Tool roster

The exact tools I have, grouped. The right column says what Arbos needs so the root agent can do the same thing. Parameter by parameter: `cursor-coordinator-tools-appendix.md`.

### Agents

| Tool | What it does | Arbos needs |
|---|---|---|
| `CreateAgent(prompt, name, model?, machine?)` | Creates a worker: an independent top-level agent with its own VM, clone, and branch. Returns its id at once; the worker runs asynchronously. `machine` picks placement: new cloud VM (with base branch), same VM (shared checkout, told to use a git worktree), a specific self-hosted machine, or a pool. `model` is optional and inherits mine. | `spawn` with name, brief, model, place (local, worktree, remote machine via hub), base branch. Returns id immediately. |
| `CreateAgent(subagent_type=explore \| computerUse \| videoReview)` | A typed short-lived helper that runs inline on my machine and returns its result before my turn continues. A tool, not a peer. | Inline helper kinds: explore (read-only codebase search), computer use (drive a UI), video review. |
| `SendToAgent(agent_id, title, message, delivery=steer\|queue, rename?)` | Messages a worker. `steer` injects into its running turn (falls back to queue when idle); `queue` delivers as its next turn. `title` is the short label of that turn; `rename` changes the worker's durable name only when its task changed. | `steer` and `queue` to a child with a per-turn title; rename on assignment change. |
| `GetAgentStatus(agent_ids?)` | Non-blocking: lifecycle (running/idle/errored/archived), turn in flight or not, last turn status, PR URL. | Child status query, no waiting. |
| `ReadAgentTranscript(agent_id, mode=tail\|full, max_turns)` | Bounded transcript read; full text also spilled to a file. | Read a child's transcript tail or full to a file. |
| `StopAgent(agent_id)` | Aborts the worker's current turn; the worker stays available. | Stop a child's turn without killing the child. |

Completion notices arrive as system notifications when a worker's turn ends, carrying the worker's last output. They are best effort; I do not depend on them arriving.

### Files and shell (my own quick work)

`Read`, `Write`, `StrReplace` (exact string replacement), `Delete`, `Glob`, `Grep` (ripgrep), `Shell` (with a tmux-backed session for anything long-lived, and a background mode with an output file I can poll with `AwaitShell`), `EditNotebook`. I use these for one quick call, verification, and `notes.md` edits, not for the delegated work itself.

### User channel

| Tool | What it does |
|---|---|
| `SendMessage(message)` | The only thing the user sees. Ordinary assistant text is hidden thinking. |
| `UpdateCurrentStep(text)` | A six-word status of my current phase, shown on the timeline ("Copying stills to artifacts"). Called alongside other tool calls when the phase changes. |
| `TodoWrite` | A structured checklist for my own multi-step work; shown to the user. |
| `SwitchMode` | Change interaction mode (agent / plan / ask / debug) when the task type changes. |
| Goals (`CreateGoal`, `UpdateGoal`) | Long-lived goal objects; used only when the user asks. |

### Repository

| Tool | What it does |
|---|---|
| `ManagePullRequest(create_pr \| update_pr \| post_comment \| resolve_comment \| get_ci_status \| set_pr_status)` | Create draft PRs by default, mark ready, comment (top-level, reply, or on a file line), resolve threads, read CI, open/close. PR bodies can embed artifact paths that are uploaded automatically. |
| `EditPullRequestLabels` | Add or remove labels, only when asked. |

### Subscriptions (wake-ups instead of polling)

`subscribe_github_pr`, `subscribe_github_ci`, `subscribe_slack_thread`, `subscribe_slack_channel`, `subscribe_slack_new_channels`, `subscribe_timer`, `list_subscriptions`, `unsubscribe`. A firing opens a turn for me. I keep subscriptions on project PRs to notice CI failures and merges. Workers use a timer to wake themselves for hourly loops (the merge steward, the QA loop).

### Web and dynamic tools

`WebSearch`, `WebFetch`; `GetDynamicTools` / `CallDynamicTool` for MCP servers (Figma, Cursor cloud diagnostics, subscriptions); `FetchMcpResource`.

### Screen

`RecordScreen(START \| SAVE \| DISCARD)` for GUI demos; screenshots go under an artifacts folder the chat can render (`/opt/cursor/artifacts/screenshots/...`).

### Environment

Cursor cloud diagnostics: run info, list of cloud agents, environment info and builds, self-hosted worker list (how I find the user's own machine as a placement target), message queue snapshot.

## 3. The delegation algorithm

1. Read the request. Is it answerable with no tool call or one quick call from evidence already in context? Then answer it myself. Otherwise delegate the whole request, even if the first calls look quick.
2. One worker per independent workstream. Unrelated streams launch in parallel in the same turn.
3. Resume an existing worker only for a direct follow-up to its assignment, or when the new work depends on its checkout, running processes, or context that would be costly to transfer. Otherwise a fresh worker.
4. Placement: cloud for independent work. The user's own machine only when the work depends on their running branch, uncommitted changes, running processes, or hardware. Never a cloud fix that must be hand-copied back.
5. Kickoff at once, from the user's words. No research first, no waiting on files. Short: task, constraints, exact output destinations, what to report. Existing content is passed as a path, never restated.
6. Worker name: a short imperative label of about five words ("Run SWE-bench through Arbos harness"). Renamed only when the assignment changes.
7. After dispatch: finish other independent coordination, end the turn.
8. Scaling: one topic, manage workers directly. Several substantial parallel topics or one coordination-heavy area: one coordinator child per area that returns one result; its interim completions stay internal.
9. Hold risky, destructive, or scope-changing actions for the user; ask once, plainly, with a recommendation, then proceed on the answer.

## 4. The project store ("Context")

One folder per project, visible to every agent of the project.

```
notes.md                  user-visible status page (section 5)
archived.md               where finished or stale notes items move; linked from notes.md
docs/                     deliverables the user asked for or will open; each linked from chat or notes
docs/project-context.md   stable goals, constraints, dated decisions; every agent reads it first
internal/                 agent-consumed material: audits, inboxes between workers, handoff notes
internal/<area>-inbox/    one folder per receiving agent (features-inbox, qa/inbox)
media/<topic>/            screenshots, recordings, data; verified before embedding
```

Rules: update existing documents rather than duplicating; short kebab-case names; folders only for several related files; a move invalidates handed-out paths, so references are updated and affected workers told. Deliverables never go in `internal/`; `internal/` paths are never linked in user-facing text unless asked.

Beyond the project store there are a user store (cross-project preferences and workflows) and a team store (established conventions only).

## 5. The `notes.md` algorithm

The page the right panel renders as "Project". Written only by the coordinator.

- Line 1: a link to `docs/project-context.md`.
- `<tldr>`: only with several sub-projects and six or more items. At most four bullets, the most recently updated workstreams first, each a fresh one-line readout with its canonical link.
- Sections `##` by durable topic (workstreams, concepts), `###` subgroups if a section needs them. Never `#` or `####`. Status-based sections only when the work is a pile of unrelated fast tasks.
- Items are `- [ ]` / `- [x]`. Shape: `[short link label](target) — status readout`. The label carries identity (PR, worker, document); the text says where it stands and what is next, one plain phrase. Rewritten fresh from current state on every touch, never appended history.
- Nest under a parent checkbox only for a real workstream with its own status, at least two groups, and at least two children. A status-less title is a header, not a checkbox. Singletons stay flat.
- Completed items: checked, last in their section, capped at the three newest. Older ones move to `archived.md` (move, never delete).
- Links: PRs and direct workers get short descriptive labels, never bare numbers or full titles. A worker's code change shows its PR if one exists, else the worker link, never both. A cloud worker with a demo can get `[Try Live](id#desktop)`.
- Update: after every real state change, silently, after the user message, before the turn ends. In-place edits preferred. A full rewrite goes through a validated temp file swapped in atomically. Never deleted. Not re-read to update it when its content is already in context.
- Restructure periodically: refit sections and nesting as workstreams start, merge, and finish; decay stale items into `archived.md`.

## 6. Kickoff brief

```
Read first: <paths in the project store the worker needs>
Task: <one paragraph in the user's words>
Do: <numbered concrete steps>
Rules: <repo and base branch, no merging, secrets via vault by id and never printed, redaction, spend cap>
Output: <exact paths under docs/ or internal/ or media/>; verify each exists
Report: <what to say back, short>
```

Plus placement (machine, base branch) and a name. Kickoffs and follow-ups are instructions plus paths, not content.

## 7. Artifacts and media handoff

- I assign the exact destination under the project store's `media/<topic>/`. The worker writes there, verifies each file, and returns the path.
- Before I show it, I verify the file exists and, for images, open it. Then I embed: images as Markdown, videos with a video tag. A path that only exists on the worker's disk, in its own store, or in `/tmp` is not a finished handoff.
- Documents: created only when content is too long for chat, is a durable artifact, or a reusable deliverable. Headline in chat, link for detail.
- A plan the user should see is one `docs/` file; after each update I verify it exists and link it from `notes.md` and the next message.

## 8. Memory (user store)

`preferences.md` is a short index of lasting preferences, linking `workflows/` (playbooks), `principles/` (decision rules with applicability and stopping boundary), and `scripts/` (reusable automation). I save a preference only when the user states it, corrects me, or repeats the behaviour under the same conditions; I record where it applies; I never generalise from one request. Current instructions override memory: conflicting guidance is revised, not stacked.

## 9. Communication

- Lead with the result or decision. Short chunks, one idea each. Define jargon once. Simple direct wording.
- Progress updates and demos as work lands, not only a final report.
- Summarise worker reports; never paste them.
- Questions asked directly, once, with a recommendation. Never ask permission for reversible work that follows from the request.
- Link every PR, worker, document, and artifact with a short label.
- Never state a worker is still working without checking.

## 10. Safety habits

- Verify evidence before a state-changing action.
- Secrets read from the vault by item id, never printed; captured output redacted.
- Spend caps checked mid-run; stop at the cap and report.
- Destructive actions, merges, and scope changes held for the user.

## 11. Gap analysis for Arbos

Grounded in `unarbos/arbos` `main` at `0eadd1d` (2026-09-14) by the process-parity worker, row for row against `cursor-coordinator-tools-appendix.md`. Where: **core** = `crates/arbos-core`, **kernel** = `crates/arbos-kernel`, **engine** = `crates/arbos-engine` (contract in `prompt.rs`, long form in `core/protocol.rs`), **desktop** = `desktop/`. "PR" is the one that closes the row; a closed row names the PR that did. A row that stays different names the reason. Status as of 2026-09-16: every PR named below is merged into `main`.

### Agents

| Cursor tool · parameter | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `CreateAgent prompt` | `spawn task/read_first/do/rules/output/report` (rendered kickoff) or raw `brief` | has | kernel `tools.rs`, core `store::Kickoff` | #103 |
| `CreateAgent name?` (derived from prompt if omitted) | `spawn name` → hyphenated id and row; from the brief's first words if omitted | has | kernel `hooks.rs` | #103 |
| `CreateAgent model?` (inherits; invalid slug fails) | `spawn model` (accepted, off-schema); inherits; a bad id fails at the first call, not at spawn | partial | kernel `tools.rs` | open, filed (validate against the provider's list at spawn) |
| `CreateAgent subagent_type = explore \| computerUse \| videoReview` | `spawn kind=explore \| computer-use \| video-review`, built in, `inline` → waits and returns | has | core `agent_def.rs` | #200 |
| `CreateAgent machine = new_cloud_vm{base_branch}` | `isolate=worktree base=<branch>` (own checkout on this machine; Arbos has no cloud VMs of its own) | has (Arbos shape) | kernel `worktree.rs` | #198 |
| `CreateAgent machine = same_vm` | default: the worker edits the checkout in place | has | — | #195 |
| `CreateAgent machine = self_hosted_worker{worker_id} \| self_hosted_pool{pool, labels}` | `spawn host=<machine>` (hub roster or `machines.toml`); no pool/labels | partial | kernel `remote.rs`, core `hub.rs` | open, filed (`host=pool:<tag>` picks a free machine with that tag) |
| Returns id at once; async; completion as a notification with last output | `spawn` returns the id; `[done]` inbox message with last words | has | kernel `plan.rs::notify_parent_done` | #98 |
| `SendToAgent agent_id, message` | `say to text` | has | kernel `hooks.rs` | — |
| `SendToAgent title` | `say title` → the worker's live line + `meta.toml` | has | kernel | #198 |
| `SendToAgent delivery = steer \| queue` | `say mode=steer \| request` | has | kernel | #8 |
| `SendToAgent rename?` | `say rename` | has | kernel | #198 |
| `GetAgentStatus agent_ids?` → lifecycle, turn in flight, last turn status, PR URL | `agents [ids]`: running / idle / paused / archived, live step, last turn verdict and words, PR | has | kernel `tools.rs` | #207 |
| `ReadAgentTranscript agent_id, mode=tail\|full, max_turns` → capped inline, full to a file | `transcript agent [mode] [max_turns]`: tail rendered inline, full to `results/transcript-<id>.txt` | has | kernel `tools.rs` | #207 |
| `StopAgent agent_id` (idle = no-op) | `say mode=stop` (idle = a note) | has (name differs) | kernel `hooks.rs` | — |

### Files and search

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `Read path, offset?, limit?` (text, images, PDFs) | `read path offset limit`; images as pixels; PDF as text (`engine/pdf.rs`) | has | engine `tools/fs.rs` | — |
| `Write path, contents` | `write path contents` | has | engine | — |
| `StrReplace path, old_string, new_string, replace_all?` | `edit old_string/new_string` (and anchors), `replace_all:true` | has | engine `tools/fs.rs` | #207 |
| `Delete path` | `delete path` (one file, through the write guard) | has | engine `tools/fs.rs` | #207 |
| `Glob glob_pattern, target_directory?` (sorted by mtime) | `find pattern [path] [sort:mtime]` | has | engine | #207 |
| `Grep pattern, path?, glob?, type?, -i, -A/-B/-C, multiline, output_mode, head_limit, offset` | `grep pattern path glob ignore_case context mode:content\|files\|count limit`; `type`, `multiline`, `offset` not taken | has (partial on three flags) | engine `tools/fs.rs`, kernel `grep.rs` | #207 |
| `EditNotebook` | none | missing (reason: no notebook work in Arbos's scope; a worker edits `.ipynb` as JSON) | — | not planned |

### Shell

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `Shell command, description?, working_directory?, block_until_ms?` (0 = background; output file with pid/cwd/exit) | `bash command cwd wait_ms background timeout_ms`; a job's `out.log` and `[kernel]` line | has | engine `tools/bash.rs`, `jobs.rs` | — |
| Named tmux session for long-lived work | `terminal action:open` (a PTY the user sees) and `background:true` jobs | has (Arbos shape) | kernel `pty.rs` | — |
| `AwaitShell shell_id?, block_until_ms?, pattern?` | `await id pattern wait_ms` | has | engine | — |
| Shell state persists across calls | each `bash` is a fresh login shell (`secret use` persists) | partial (reason: a persistent shell hides state from the transcript; `terminal` is the persistent one) | — | not planned |

### User channel

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `SendMessage message` (the only thing the user sees) | the reply is the message; `say to=user` for a notice; answer shape | has | engine `prompt.rs` | #195 |
| `UpdateCurrentStep current_step` (timeline step) | `status step` → `status.toml` + `status` frame; the desktop draws it as the live line | has | core `status.rs`, desktop | `status_e2e`, #216 |
| `TodoWrite todos[{id, content, status}], merge` | `todo set/add/check/update/remove/show` over `agents/<id>/todo.md`; `changed` frame; desktop card | has | kernel `tools.rs`, desktop | #201, #209 |
| `SwitchMode target_mode_id` | `mode: auto \| ask \| plan` set by the user | partial (reason: Cursor's coordinator cannot switch either; the user sets it) | core `agent.rs` | not planned |
| `CreateGoal / UpdateGoal` | `subscribe kind=goal` | has | core `subscription.rs` | `goals_e2e` |
| `GenerateImage` | none | missing (reason: no image model behind the provider interface yet) | — | filed, not planned |

### Repository

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `ManagePullRequest create_pr title body branch_name base_branch draft` | `pr create title body [branch base] [draft]` (draft by default; template folded in; recorded and followed) | has | kernel `pr_tool.rs` | #211 |
| `… update_pr title body base_branch draft:false` | `pr update pr [title body base] [draft:false → ready]` | has | | #211 |
| `… post_comment body in_reply_to path line start_line side` | `pr comment pr body [in_reply_to \| path line start_line side]` | has | | #211 |
| `… resolve_comment comment_id` | `pr resolve pr comment_id` (GraphQL review thread) | has | | #211 |
| `… get_ci_status` | `pr ci pr` (`gh pr checks`, verdict from the exit code); `github_ci` subscription | has | | #211, #104 |
| `… set_pr_status open \| closed` | `pr status pr open\|closed` | has | | #211 |
| Artifact paths in the body uploaded and rewritten to URLs | local links pushed to the `arbos-artifacts` branch, rewritten to raw URLs | has | | #211 |
| PR templates honoured | the repository's template folded into the body | has | | #211 |
| `EditPullRequestLabels pr_url add_labels remove_labels` | `pr labels pr add[] remove[]` | has | | #211 |
| `SetActiveBranch path, branchName` | the desktop shows the checkout's branch; a worktree worker's done names its branch | has (Arbos shape) | desktop panel head | — |

### Subscriptions

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `subscribe_github_pr` | `subscribe kind=github_pr repo pr` (+ auto-follow of `gh pr create` and `pr create`) | has | core `subscription.rs` | #104 |
| `subscribe_github_ci` | `kind=github_ci repo pr \| branch` | has | | #104 |
| `subscribe_slack_channel` | `kind=chat channel [match]` on a door's channel (Discord or Slack) | has | kernel `chatdoor.rs`, `subs.rs` | #202 |
| `subscribe_slack_thread` | `kind=chat channel thread=<ts>` | has | | #202 |
| `subscribe_slack_new_channels` | none (a door names its channels) | missing (reason: needs a workspace-level list call; low value) | — | filed, not planned |
| `subscribe_timer` | `kind=timer every \| after \| at`, `continuity` | has | | #104 |
| `list_subscriptions`, `unsubscribe` | `subscribe list`, `remove` (+ `pause`, `resume`) | has | | #104 |

### Web and dynamic tools

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `WebSearch search_term` | `search query max_results` (numbered sources) | has | engine `tools/web.rs` | — |
| `WebFetch url` (no auth, no private hosts) | `fetch url` (refuses the metadata service) | has | engine | — |
| `GetDynamicTools namespace? toolName? pattern?` | MCP servers from `mcp.toml` appear as `mcp__<name>` tools with their schemas in the tool list | has (Arbos shape) | kernel `mcp.rs` | — |
| `CallDynamicTool namespace toolName arguments` | calling the `mcp__<name>` tool | has | kernel `mcp.rs` | — |
| `FetchMcpResource server uri downloadPath?` | none | missing | kernel `mcp.rs` | open, filed (`mcp resource`) |

### Screen and environment

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| `RecordScreen START \| SAVE \| DISCARD, save_as_filename?` | `record op:start \| stop \| discard [max_secs]`; the last frame beside the video | has | kernel `record.rs` | #207 |
| Screenshots under an artifacts folder the chat renders | `screenshot` → `agents/<id>/images/`, `media/<topic>/` in the store; the chat renders images by path | has | kernel `screenshot.rs` | #16 |
| `run-info`, `environment-info` | `runtime/kernel.json`, `Environment:` prompt line | has (Arbos shape) | kernel | — |
| `list-cloud-agents`, `batch-fetch-details` | `agents` (status) and `transcript` (transcripts to a file); hub roster of machines | has (Arbos shape) | kernel `tools.rs`, core `hub.rs` | #207 |
| `get-message-queue` (pending follow-ups) | `<<inbox>>` in the prompt: waiting messages, from and title | has | engine `prompt.rs` | #206 |
| `list-self-hosted-workers` | `Machines:` roster line; `.arbos/machines/` | has | core `hub.rs` | #93 |
| environment builds / snapshots / setup actions | none | missing (reason: no cloud VM images in Arbos; a place is a folder) | — | not planned |

### Skills

| Cursor | Arbos | Status | Where | PR |
|---|---|---|---|---|
| Skill file list; read the matching one first | `.arbos/skills/<name>/SKILL.md`, `/name`, `/mode <skill>`; roster in the prompt | has | core `skills.rs` | #42 |

### Context every turn

| Cursor context | Arbos | Status | Where | PR |
|---|---|---|---|---|
| User info: OS, shell, workspace path, git flag, date, terminals folder | `Project:`, `Cwd:`, `Environment:`, `Now:` (date, weekday, OS, arch, shell) | has | engine `prompt.rs` | #206 |
| Git status snapshot at start | `Git:` branch, short head, dirty count | has | engine `prompt.rs` | #206 |
| Always-applied workspace rules and user rules | `Rules`: `.cursor/rules/*.mdc` with `alwaysApply: true`, `.arbos/rules/`, `~/.config/arbos/rules/` | has | engine `prompt.rs` | #206 |
| The Project prompt (role, loop, notes rules, store, media, memory, communication) | `COORDINATOR_CONTRACT` + `PROTOCOL.md` | has | engine `prompt.rs`, core `protocol.rs` | #103–#211 |
| The current agent's store path | `Store:` line | has | engine | #206 |
| Attached files and images, saved with paths to pass on | `user` frame `attachments` → paths on the `user` line; images as pixels | has | engine `project.rs` | — |
| Notifications: worker turn ends with last output, subscription firings, queued-message hints | `[done]` and `subscription:N` inbox messages open turns; `<<inbox>>` names what waits | has | kernel, engine | #98, #104, #206 |
| Running summary after compaction | `[context checkpoint]` summary | has | engine `compact.rs` | — |

### Turn loop, delegation, store, notes, kickoff, media, memory, communication, safety (sections 1, 3–10)

| Rule | Arbos | Status | Where | PR |
|---|---|---|---|---|
| Turn opens on user / worker done / subscription only; never poll | inbox files + `[done]` + subscriptions; contract forbids polling; one bounded look via `agents`/`transcript` | has | kernel `plan.rs`, `subs.rs` | #91, #104, #207 |
| Event turn: message only when warranted, else notes | directive | has | engine `prompt.rs` | #103 |
| Verify claimed artifacts before embedding | kernel checks the local paths a top-level reply links; a missing one wakes the agent once | has | kernel `plan.rs::verify_reply_links` | #204 |
| Delegation 1–7 | directive + spawn-first + placement and reuse rules | has | engine `prompt.rs` | #103, #196, #198 |
| Delegation 8: coordinator child per area | `spawn kind=coordinator` / `role=coordinator` | has | core `agent_def.rs`, `project.rs` | #200 |
| Delegation 9: hold risky actions, ask once | directive; `mode: ask`, `approve` | has | engine, kernel | #37, #106 |
| Store layout | bootstrap seeds it; root-only writes enforced | has | core `store.rs` | #103, #107 |
| `internal/<area>-inbox/`; never link `internal/` in user text; update rather than duplicate | directive | has | engine `prompt.rs` | #198 |
| User store and team store beyond the project store | `~/.config/arbos/{preferences.md,workflows/,principles/,scripts/}`; no team store | has (team store: not planned) | engine `tools/memory.rs` | #205 |
| notes.md line 1, sections by topic, item shape, one per workstream, spoken label; context link never an item; a standing check links its subscription file | `plan` tool + directive + `check` lint | has | core `notes.rs`, `store.rs` | #107, #185, #211 |
| `<tldr>` threshold, ≤4, freshest first | kept by the tool once the page is big; a hand-written one is kept | has | core `notes.rs` | #203 |
| Completed: three kept, older **move** to `archived.md` | overflow moved under its section, newest last | has | core `notes.rs` | #203 |
| Worker's PR if one exists, else the worker, never both | retire re-points at the PR from `prs.jsonl` | has | kernel `plan.rs` | #203 |
| Atomic temp-file swap, never deleted | tmp+rename; `notes.md` root-only | has | core `notes.rs`, `store.rs` | #107 |
| Restructure periodically, decay stale items | finished items decay into the archive; section refits stay root's | partial | core `notes.rs` | #203 |
| Kickoff brief six fields + placement + name | `spawn` template, `base`, `host`, `name` | has | core `store::Kickoff` | #103, #198 |
| Media handoff | directive + kernel link check | has | engine, kernel | #198, #204 |
| Documents rule | directive | has | engine `prompt.rs` | #198 |
| Memory rules (state/correct/repeat; applies; revise not stack) | `remember scope=user kind=… applies=…`; same name replaces | has | engine `tools/memory.rs` | #205 |
| Communication | answer shape + directive | has | engine `prompt.rs` | #195 |
| Safety: secrets by id never printed, redaction | `secret` tool, `[REDACTED:NAME]` | has | kernel `secret_tool.rs` | #30 |
| Safety: spend caps checked mid-run | `[spend] cap_usd`; counted per turn, announced at 80 % and the cap, workers/subscriptions/spawn refused past it | has | core `spend.rs`, kernel | #208 |

### What Cursor does not have (kept in Arbos)

Worker-to-worker `say` with hops; `mode: ask` + `approve` with a disk mirror; hooks; per-agent permission modes; rewind; nested `.arbos` git; kernel-run shell subscriptions with no model turn; voice and chat doors as input; a persistent visible `terminal`.

### Slices (one PR each, stacked, all merged into `main` by 2026-09-16)

1. #198 — `say title/rename`, `spawn base`, placement/store rules in the directive.
2. #200 — built-in helper kinds `explore`, `computer-use`, `video-review`; `spawn role=coordinator`.
3. #201 — `todo`, the thread's own checklist.
4. #202 — `subscribe kind=chat` (channel, thread, match).
5. #203 — page algorithm in `plan`: overflow **moves** to `archived.md`; `<tldr>` kept by the tool; PR link on retire; ordered edits.
6. #204 — kernel verifies the files a top-level reply links; a missing one wakes the agent once.
7. #205 — user store: `~/.config/arbos/{preferences.md,workflows/,principles/,scripts/}`; `remember kind=`.
8. #211 — `pr` tool (`create/update/comment/resolve/ci/status/labels/template`) with artifact upload to `arbos-artifacts` and PR templates.
9. #208 — spend cap (`project.toml [spend] cap_usd`), counted per turn, announced at 80 % and the cap, enforced.
10. #206 — context inventory: `Store:`, `Now:`, `Git:`, `Rules`, `<<inbox>>`.
11. #207 — tool parameters: `agents`, `transcript`, `delete`, `edit replace_all`, `find` by mtime, `grep ignore_case/context/mode/limit`, `record op:discard`.

Left open, filed: `mcp resource`, `spawn model` validated at spawn, `host=pool:<tag>`, `grep type/multiline/offset`. Not planned, with a reason above: `EditNotebook`, `SwitchMode`, `GenerateImage`, `subscribe_slack_new_channels`, environment builds/snapshots, persistent shell state, a team store.

Desktop asks filed to `internal/features-inbox/2026-09-14-process-parity.md` for the layout worker: worker line shows the turn `title` (done in the symmetry cycles `[less sure]` of the PR number); `todo.md` card (#209); `status` as the timeline step (#216).

### Acceptance run (2026-09-15, headless, the whole stack)

A fresh place, `role = coordinator`, one four-part goal (principles; primary-source research; a script written and run; a 30-minute standing check). Root wrote `docs/project-context.md` itself (Goal, Principles, Constraints, dated Decisions, Resources), spawned two named workers with six-field briefs, kept `notes.md` with `##` sections by topic and `[label](target) — readout` items, the research landed as `docs/markdown-task-lists-and-setext-headings.md`, both workers saved a still under `media/<topic>/`, the standing check is `agents/root/subscriptions/0002-….toml`, finished rows were checked and re-pointed at the deliverables, both workers archived, `arbos-kernel check` clean. Evidence: `media/process-parity/acceptance-2026-09-15/`. Two drifts from this Project's page, folded into the directive (#211): the context link had become an item above the first section; the standing check's item pointed at `notes.md` instead of its subscription file.
