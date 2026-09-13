# Cursor vs Arbos: agents, plans, crons, inboxes

Comparison only. No code was changed. Arbos audited on `unarbos/arbos` branch `cursor/release-integration-52cd` at `e3d89b6` (2026-09-13). Cursor side from first-hand observation of Cursor's coordinator tooling and its public documentation.

## Summary

1. Cursor has **no plan engine**. Its coordinator has agents, messages, subscriptions, and documents. Nothing else.
2. Arbos has all four of those too, plus a fifth thing Cursor lacks: a **plan node** that is at once a goal, a timer, a shell job, a callback, and a question. That one type carries most of Arbos's complexity.
3. Scheduling: Cursor has one scheduler, **subscriptions** (timer, GitHub PR, GitHub CI, Slack). Arbos has two: plan nodes (`every`/`after`/`wake`/`condition`) and a separate GitHub poller, fed through five wake paths in one `select!` loop.
4. Messaging: Cursor has `SendToAgent` with `steer` or `queue`. Arbos Phase 2 already has the better shape, **inbox files**, but user steer still lives in memory and the GitHub door writes every message twice.
5. Plans and goals: Cursor keeps them in **documents** (`notes.md`, `docs/project-context.md`) plus a worker checklist tool. Arbos keeps them in `plan.jsonl` with six statuses, attempts, sibling gating, and a parent-settling loop.
6. Agents: same model. `spawn` = `CreateAgent`, `say mode=steer` = `SendToAgent steer`, `spawn wait=true` = an inline subagent, `agents-defs/` = custom subagents, `machines.toml` = self-hosted workers (push over SSH instead of outbound connect).
7. Arbos has things Cursor's coordinator does not: file-first state, nested git and rewind, worker-to-worker `say`, hooks, permission modes, kernel-run shell jobs with no model turn, voice, Telegram. Keep them.
8. Recommendation: **adopt Cursor's vocabulary and object set**. Goals are a file. Progress is a checklist file. Every timed or event-driven wake is a subscription. Every message is an inbox file. Delete the plan node.
9. This changes Phase 4 of the file-system design (plan files) and simplifies Phase 6 (ask). Phases 1, 2, 3, 5, 7 stay as written; GOALS.md moves earlier.
10. Six decisions for Jacob are at the end. The big one: delete `Node`/`When`/`Do`/`Attempt`/`Clock` and replace them with `plan.md` + `subscriptions/`.

## Terms

- **Coordinator**: the one agent the user talks to in a Cursor Project. It delegates; it does not edit code.
- **Worker**: an agent the coordinator creates to do one workstream. Arbos calls this a child or sub-agent.
- **Turn**: one run of the model loop, from a wake to "done". Same word in both systems.
- **Wake**: the moment an idle agent starts a turn. In Cursor a wake is always "a message or event arrived". In Arbos it is a struct with six kinds.
- **Subscription**: a standing request to be woken when something happens (a clock tick, a PR change, a Slack message).
- **Inbox**: where messages wait for an agent. In Arbos, a folder of files.
- **Plan node**: Arbos's unit of plan. One goal with a `when` (trigger) and a `do` (executor).
- **Cron**: a job that runs on a schedule.
- **Steer**: words injected into a turn that is already running, read at the next tool call.
- **Queue**: words delivered as the next turn, after the current one ends.
- **Agent Store**: Cursor's shared folder synced to every agent in a Project (`notes.md`, `docs/`, `internal/`, `media/`). Arbos's equivalent is `.arbos/`.

## 1. Arbos today

All paths are under `crates/` unless noted.

### Agent

- One folder `.arbos/agents/<id>/` with `agent.md` (`key: value` lines: name, parent, paused, model, allowlist, readonly, cwd, remote, mode, kind). `arbos-core/src/agent.rs:109-148`.
- Parent/child is the `parent:` line. Caps: 8 children, depth 3 (`arbos-kernel/src/sched.rs:11-12`), overridable from `config.toml` (`hooks.rs:89-105`).
- Custom kinds: `.arbos/agents-defs/<name>.md`, and it also reads `.cursor/agents/*.md` (`arbos-core/src/agent_def.rs:1-34`).
- Remote child: `agent.md` has `remote: <machine>:<path>`; the kernel rsyncs the project, starts a kernel over SSH, tunnels, and mirrors the transcript (`arbos-kernel/src/remote.rs:1-12`; machines in `~/.config/arbos/machines.toml`, `arbos-core/src/machines.rs:1-38`).

### Turn

- Started by the serve loop's `start` closure: drains `wake = false` notes into the transcript, marks running, hands a `Wake` to the scheduler (`serve.rs:293-326`).
- A turn caused by an inbox file gets `turns/tNNNN/cause.md` (the claimed message) and a small `meta.toml` (`arbos-core/src/inbox.rs:221-237`, `plan.rs:448-455`). A turn caused by a plan node does **not**: its record is `TurnMeta` in memory (`plan.rs:32-37`, `plan.rs:219-226`).
- On end: `finish_turn` closes the node from the transcript, or closes the turn folder (`plan.rs:570-643`); then one git commit of `.arbos/` (`serve.rs:344-345`, `snapshot.rs:1-8`).
- Crash recovery still reads the transcript shape (`needs_serve`, `arbos-core/src/files.rs:504-526`) and reclaims `active` nodes (`plan.rs:75-121`).

### Plan node

- `Node { id, parent, seq, goal, check, when, do, status, outcome, origin, hops, attempt, attachments }` (`arbos-core/src/node.rs:160-196`).
- `when`: `after_ms`, `every_ms`, `next_due_ms`, `wake`, `condition` (`node.rs:93-114`). `do`: `Agent | Shell{cmd, report} | Notify{text} | Ask` (`node.rs:117-135`).
- Six statuses with a legal-transition graph (`node.rs:434-463`); earlier one-shot siblings gate later ones (`node.rs:468-475`); readiness predicate (`node.rs:478-494`); `fireable` sorts into mechanical, condition, and agent wakes (`node.rs:516-539`).
- Storage: `plan.jsonl` and `attempts.jsonl`, "last line per id wins", folded at boot (`node.rs:345-374`). `plan.md` is a generated render.
- The `plan` tool: `add` / `update` / `show` with `when × do` per node (`arbos-kernel/src/tools.rs:246-303`). `NewNode::build` validates ~125 lines, including sniffing the goal text for "every ", "hourly", "in 30m" and refusing the node (`hooks.rs:415-540`).
- Kernel-owned behaviour: a parent goal closes when all children close (`settle_parents`, `hooks.rs:640-675`); a `wake: true` node fires a "Callback" prompt when its siblings finish (`plan.rs:526-529`); a recurring node is re-armed at claim (`plan.rs:283-285`).
- The model is told to put every timed request in a plan node and to trust `<<plan>>` over memory (`arbos-engine/src/prompt.rs:9-10`).

### Inbox message (Phase 2, landed)

- One file `agents/<id>/inbox/<utc>-<from>-<seq>.md` with TOML front matter: `from`, `kind` (`message | request | brief | answer | approval | wake`), `wake`, `reply_to`, `hops`, `attachments`, `sent`, then the body (`inbox.rs:1-49`).
- Written whole-or-absent by tmp + rename (`inbox.rs:133-162`). Claimed by rename into `turns/tNNNN/cause.md` (`inbox.rs:221-237`).
- Producers: user prompt (`serve.rs:627-631`), `say` (`hooks.rs:1350-1385`), spawn brief (`hooks.rs:1216-1218`), Telegram (`doors.rs:39`), GitHub door (`github.rs:328-330`). All still go through `hooks.inbox(agent, Node)`, which translates a `Node` into a file (`hooks.rs:755-776`): the message API still speaks in nodes.
- Consumed: `wake = true` files are claimed by `plan::scan` before any plan node (`plan.rs:175-209`); `wake = false` files are drained at the next turn start (`take_notes`, `hooks.rs:803-828`).

### Wake

- `Wake { agent, kind, text, attachments, steer, node, hops }`, kinds `User | Say | Plan | Serve | Job | Compact` (`arbos-core/src/wake.rs:8-32`).
- Five wake paths meet in one loop (`serve.rs:329-499`): `wake_rx` (Serve, Compact, Job), `kick_rx` → `plan::scan` (inbox claims and plan nodes, every write and every 5 s), `done_rx` (turn ended → finish, commit, requeue steers), the 200 ms `tail` tick (job sweeps → `Job` wake, `serve.rs:436-472`), and the 5 s `tick` (kick).
- The old `wake` marker file still exists with no writer (`files.rs:66`, `files.rs:506`).

### Steer

- User steer: `Frame::User { steer: true }` goes into the live turn's in-memory `TurnControl` queue (`serve.rs:608-611`, `arbos-engine/src/control.rs:19-24`, `:89-91`), consumed at the next tool boundary (`turn.rs:358-370`). Leftovers at turn end are turned into inbox files (`requeue_steers`, `hooks.rs:305-319`). Not yet a file while pending.
- Agent steer: `say mode=steer` uses the same queue; if the target is idle it becomes a waking inbox file (`hooks.rs:1305-1316`, `:1383-1389`).

### Say

- Modes `note | request | steer` (`hooks.rs:34-41`; schema `tools.rs:206-224`). `to` resolves by id, name, or unique substring (`hooks.rs:1224-1252`).
- `note`: inbox file, `wake = false`. `request`: inbox file, `wake = true`, with a `hops` reply budget (default 3, `node.rs:38`) that falls back to a note at 0 (`hooks.rs:1364-1375`).
- `say to=user`: appends to `.arbos/user.md` by full rewrite, and a `Say` event on the top ancestor's transcript (`hooks.rs:1415-1433`).
- Dedupe of `(to, text)` per turn is in memory (`hooks.rs:1399-1409`).

### Spawn brief

- `spawn(brief, kind, model, readonly, cwd, isolate, host, wait, wait_secs)` (`tools.rs:57-101`). `isolate=worktree` cuts a git worktree; `host=<machine>` spawns on another computer; `wait=true` blocks until the child's first report (`tools.rs:175-194`).
- The child is created with a narrowed allowlist and the brief as a `kind = "brief"` inbox file (`hooks.rs:1100-1220`). The first prompt wraps it in a fixed template that tells the child to decompose with `plan add` and report with `say` (`plan.rs:410-421`).
- Report back is agent-driven: the child must remember to `say`. The kernel writes to the parent only when the parent is blocked in `wait` (`hooks.rs:251-273`).

### Subscription (GitHub door, #32)

- `subscribe add repo= pr= note=` / `list` / `remove` (`github.rs:334-349`). Stored in one place-level file `.arbos/subscriptions.json` (`github.rs:42-69`).
- A poller runs `gh pr view` every 60 s, diffs against the last snapshot (reviews, comments, checks, head, state), and on change delivers a `[github]` message (`github.rs:228-316`).
- Delivery writes twice: a `Say` event straight onto the transcript **and** a waking inbox file (`github.rs:320-332`); the claim then appends a second `Say` event for `from = "agent:github"` (`plan.rs:425-442`). Every GitHub message shows twice.

### Hook (#45)

- Executables in `.arbos/hooks/<event>/` or entries in `.arbos/hooks.toml`; events `before-tool | after-tool | after-turn`; Cursor's `PreToolUse`/`PostToolUse`/`Stop` names are accepted (`arbos-engine/src/tools/file_hooks.rs:1-66`). Exit 2 blocks, exit 3 asks the user.

### Mode (#37)

- `mode: auto | ask | plan` in `agent.md` (`agent.rs:54-107`). Children inherit the stricter of parent and own (`agent.rs:298-302`). Changed by `Frame::SetMode`, recorded as a transcript notice (`serve.rs:776-794`).

### Ask and approve

- `ask` blocks the turn on an in-memory one-shot channel (`tools.rs:362-399`, `hooks.rs:1452-1487`). The answer arrives as `Frame::Answer` (`serve.rs:706-726`). A kernel restart orphans the question.
- `approve` is the same shape for dangerous commands (`hooks.rs:1490-1508`).

### Rewind (#80/#82/#89) and git (Phase 1)

- `.arbos/` is a nested git repo; `runtime/` holds lock, `kernel.json`, focus (`arbos-core/src/place.rs:31-74`). One commit per turn and per mechanical node run (`snapshot.rs:1-8`, `plan.rs:164-168`).
- `arbos-kernel rewind --to LINE | --back N | --commit SHA [--files]` cuts the transcript and can restore the working tree from a per-turn checkpoint (`arbos-kernel/src/rewind.rs:1-22`, `arbos-engine/src/tools/git.rs:44-52`).

### Memory and goals

- `remember` writes `.arbos/memory.md` (place) or a user-scope file; shown under Memory in the prompt (`prompt.rs:21`, `:105-124`). `AGENTS.md`/`CLAUDE.md` at the project root is read too (`prompt.rs:232`).
- No `GOALS.md` exists on this branch (grep of `crates/` finds none). Design Phase 5a.

## 2. Cursor's primitives

From the observed-tooling note unless marked.

- **Coordinator + workers.** One coordinator per Project, delegates, never edits code. Workers are top-level agents with their own VM and branch by default.
- **`CreateAgent(prompt, name, model?, machine?, subagent_type?)`.** `machine = new_cloud_vm | same_vm | self_hosted_worker{id} | self_hosted_pool{pool, labels}`. With `subagent_type` it runs inline and blocks; no message channel.
- **`SendToAgent(agent_id, title, message, delivery = steer | queue)`.** Steer injects into the running turn, falls back to queue when idle. Queue is the next turn.
- **`GetAgentStatus`, `ReadAgentTranscript(tail|full)`, `StopAgent`.** Status = running/idle/errored/archived, turn in flight, last turn status, PR URL.
- **Completion notification.** A system message when a worker's turn ends. Best-effort. The coordinator is told never to poll.
- **No worker-to-worker messaging.** Workers report to the coordinator; it relays.
- **Subscriptions are the only scheduler.** `subscribe_timer`, `subscribe_github_pr`, `subscribe_github_ci`, `subscribe_slack_*`, `list_subscriptions`, `unsubscribe`. An event opens a turn with a system notification. Max 180 days (research doc).
- **No inbox object.** Pending user follow-ups are visible via `get-message-queue`; that is the whole queue.
- **Plan surface is documents.** `notes.md` (coordinator-owned checkbox status, updated every turn), `docs/project-context.md` (stable goals, constraints, decisions; workers read it first). Plans are files in `docs/`. No node, cron, or plan graph.
- **Goals are a user-invoked tool.** `CreateGoal`/`UpdateGoal`, used only when the user asks.
- **Worker checklist.** A worker has a todo-list tool for its own session (this worker's `TodoWrite`); it schedules nothing.
- **Agent Store** is the shared context, synced to every agent. Workers write results there.
- **Self-hosted workers** connect outbound; cloud agents claim them by id or pool.
- **Behaviour rules**: delegate anything non-trivial; one worker per workstream; steer instead of restart; hold risky actions for the user and ask once; update `notes.md` after every state change.

## 3. Comparison table

| Concept | Cursor | Arbos today | Same? | Recommendation |
| --- | --- | --- | --- | --- |
| Coordinator role | One coordinator, never edits code | `root` agent, `mode: auto` may edit | Different | **Add** `role = "coordinator"` (read, spawn, say, plan, ask only) default for new projects |
| Worker | Top-level agent, own VM/branch | Child folder, `parent:` line, worktree or `host=` | Same | Keep |
| Create worker | `CreateAgent(prompt, machine, subagent_type)` | `spawn(brief, kind, isolate, host, wait)` | Same | **Rename** `isolate`+`host` → one `machine` arg (`here \| worktree \| <name>`); keep `spawn` |
| Inline blocking subagent | `subagent_type` runs inline | `spawn wait=true` | Same | Keep |
| Custom agent kinds | `.cursor/agents/*.md` | `.arbos/agents-defs/`, reads `.cursor/agents/` too | Same | Keep |
| Steer a worker | `SendToAgent delivery=steer` | `say mode=steer`; user `Frame::User steer` | Same | Keep; make the pending steer a `kind = "steer"` inbox file, not `TurnControl` memory |
| Queue a message | `SendToAgent delivery=queue` | `say mode=request` (+ `hops`) | Same | **Rename** `request` → `queue` |
| Silent note | none | `say mode=note` (`wake = false`) | Arbos extra | **Remove** from the tool; keep `wake = false` only as the kernel's hops-exhausted fallback |
| Worker → coordinator result | Completion notification, system-written | Child must `say`; kernel writes only for `wait` | Different | **Add** kernel-written `kind = "done"` inbox file to the parent at every child turn end (last words + pointer) |
| Worker ↔ worker | Not allowed | `say to=<any id>` with `hops` | Arbos extra | Keep; roster + hops already bound it |
| Status read | `GetAgentStatus` | `agent.md`, `Frame::Tree`, `turns/` (partial) | Same idea | Keep; finish `status.toml` (Phase 3) |
| Transcript read | `ReadAgentTranscript tail\|full` | `read`/`grep` on `transcript.jsonl` (prompt says never do it) | Same idea | Keep file read; drop the "never read" rule for the coordinator |
| Stop | `StopAgent` | `Frame::Stop` → `stop_work` (turn, jobs, children) | Same | Keep |
| Timer / cron | `subscribe_timer` | Plan node `every`/`at`/`after` | Different | **Replace** with `subscribe kind=timer` |
| PR / CI events | `subscribe_github_pr`, `subscribe_github_ci` | `subscribe add repo pr` (checks inside the diff) | Same | Keep; split `pr` and `ci` kinds; fix the double `Say` write |
| Slack | `subscribe_slack_*` | Telegram door (`doors.rs`) | Different source, same shape | **Add** as `subscribe kind=slack` later; Telegram becomes `kind=telegram` |
| Poll-a-predicate | none (PR/CI pollers do it internally) | Plan node `condition` + `every` | Arbos extra | **Simplify** to `subscribe kind=shell` (poll a command; fire on exit 0 or on output change) |
| Kernel-run job, no model | none | `do: shell` + `notify "{output}"` | Arbos extra | Keep as a subscription option `deliver_to = user` (cheap readings) |
| Callback when siblings finish | none; completion notices | Plan node `wake: true` | Arbos extra | **Remove**; the `done` message above replaces it |
| Deferred one-shot | `subscribe_timer` once | `when.after` | Same idea | Fold into `kind=timer, once = true` |
| Goal hierarchy, gating, statuses | none | `Node` tree, `seq` gating, 6 statuses, `settle_parents`, `attempts.jsonl` | Different | **Remove** the engine |
| Worker checklist | TodoWrite (session list, no scheduling) | `plan add/update/show` with `when × do` | Different | **Simplify** `plan` to a checklist that writes `plan.md` (`set`, `check`, `show`); no `when`, no `do` |
| Project goals | `docs/project-context.md` | none (`AGENTS.md`, `memory.md` nearest) | Missing | **Add** `.arbos/GOALS.md`, root-owned (Phase 5a), injected every turn |
| Status list | `notes.md`, coordinator-owned | `plan.md` generated render | Different | Root's `plan.md` becomes the agent-written checklist the UI shows = `notes.md` |
| Goals tool | `CreateGoal`/`UpdateGoal`, user-invoked only | none | Missing | Not needed: `edit` on `GOALS.md` by root; a `goal` op is optional |
| Inbox | No object; `get-message-queue` shows pending | `inbox/*.md` files | Arbos better | Keep |
| Wake | "a message or event arrived" | `Wake` struct, 6 kinds, 5 paths | Different | **Simplify**: wake = an inbox file with `wake = true`; delete `WakeKind`, `Serve`, `Job`, `Compact` wakes |
| Turn record | Status has "turn in flight", "last turn status" | `turns/tNNNN/` for inbox turns; memory for node turns | Partial | Finish Phase 3; every turn gets a folder |
| Ask the user | Coordinator asks in chat; answer is the next follow-up | `ask` blocks on a one-shot channel | Different | **Simplify** to park: end the turn with the question; answer is an inbox `kind = "answer"` |
| Approve a risky action | "Hold risky actions, ask once" (a rule) | `mode: ask`, `approve` channel, hooks exit 3 | Arbos stronger | Keep; block-with-disk-mirror (Phase 6) |
| Permission modes | Agent/Plan/Ask modes in the IDE | `auto \| ask \| plan` per agent, inherited | Same | Keep |
| Hooks | `.cursor/hooks` (IDE), not in coordinator tooling | `hooks/` dirs + `hooks.toml`, Cursor names accepted | Arbos extra | Keep |
| Shared context | Agent Store synced to every agent | `.arbos/` nested git; rsync + mirror for remote | Same idea | Keep; sync = push/pull of the `.arbos` repo |
| Memory / preferences | `preferences.md` in user store | `remember` → `memory.md` (place or user scope) | Same | Keep |
| Rewind | IDE checkpoints; none in coordinator | `rewind --to/--back/--commit [--files]` | Arbos extra | Keep |
| Self-hosted machines | Worker connects outbound; claimed by id/pool | `machines.toml`; kernel pushes over SSH | Different direction | Keep registry; **Change** to outbound hub registration (Phase 7) |
| Voice, Telegram | none in coordinator | `doors.rs` | Arbos extra | Keep |
| User notices | Coordinator writes in chat + `notes.md` | `user.md` full rewrite + `Say` on root | Different | **Replace** `user.md` with `user/inbox/*.md` (Phase 3) |

Where Arbos is more complex than it needs to be:

- One type does five jobs. `Node` is goal, cron, shell job, callback, and question. That forces `is_inbox` (six conditions), `origin` prefixes, `hops` on nodes, the status graph, `settle_parents`, `attempts.jsonl`, `reclaim`, and ~125 lines of `NewNode::build`. Cursor needs none of it.
- Two schedulers. `plan::scan` and the GitHub poller both wake agents; the GitHub poller then goes through the plan path anyway (`Node::inbox`).
- Five wake paths and a six-kind `Wake` for what Cursor treats as one event: "something arrived".
- The message API still speaks in nodes. `hooks.inbox(agent, Node)` converts a `Node` into a file for every producer.
- Two ways to say something to a peer without waking it (`note`, exhausted `hops`) and two to wake it (`request`, `steer`-when-idle).

Where Arbos has something Cursor's coordinator lacks, and should keep it: file-first state you can `cat`; nested git and rewind; `say` between any two agents; hooks; per-agent permission modes with inheritance; kernel-run shell on a schedule with no model turn; voice and Telegram doors; worktree isolation on the same machine.

## 4. Keep / Change / Remove

**Keep**

- `agents/<id>/` folders, `agent.md` (→ `agent.toml` later), parent chain, caps, `agents-defs/`.
- `inbox/*.md` files, claim-by-rename, `turns/tNNNN/cause.md`.
- `spawn` (with `wait`, `kind`, worktree, other machines), `say mode=steer`, `stop_work`.
- Hooks, modes, `approve`, `remember`/`memory.md`, `AGENTS.md`.
- Nested `.arbos` git, one commit per turn, `rewind`, `arbos-kernel check`, fixtures.
- GitHub PR subscription and its poller.
- Kernel-run shell with `{output}` delivery to the user.

**Change**

- `say`: modes become `queue | steer` (Cursor's words). `note` leaves the tool surface.
- `spawn`: `isolate` + `host` → `machine`. Add the design's `context`, `look_in`, `report_as` pointer fields (Phase 5d).
- `plan` tool: from `when × do` nodes to a checklist (`set` the list, `check` an item, `show`). It writes `agents/<id>/plan.md` directly. No scheduling.
- `subscribe`: from GitHub-only to `kind = timer | github_pr | github_ci | shell | telegram | slack`, stored one file per subscription under `agents/<id>/subscriptions/NNNN-slug.toml`. Its firing writes one inbox file. `plan.md`'s "standing" section is rendered from this folder.
- Steer: a pending steer is an inbox file with `kind = "steer"`; the turn loop reads the folder at each tool boundary.
- `ask`: park, not block. The turn ends with the question on the transcript and a `waiting/ask-*.toml`; the answer is an inbox file and starts a new turn.
- Child completion: the kernel writes `kind = "done"` to the parent's inbox when a child's turn ends (last assistant text, or a pointer to the child's result file).
- `user.md` → `user/inbox/*.md`.
- Root: `role = "coordinator"` in `project.toml` narrows root's allowlist to read/grep/find/spawn/say/ask/plan/subscribe/browser. Default on for new places.
- Remote workers: keep `machines.toml`; direction flips to outbound registration through the hub (Phase 7).

**Remove**

- `Node`, `When`, `Do`, `Attempt`, `NodeStatus` graph, `can_transition`, `gated_by_sibling`, `fireable`, `is_inbox`, `settle_parents`, `NewNode::build` and its goal-text sniffing, `plan.jsonl`, `attempts.jsonl`, `compact_nodes`, `reclaim`'s node repair.
- `Clock`, `TurnMeta`, `Clock.turns`, `Clock.mech`, `claim`, `abandon`, `finish_turn`'s node half, `wake_for`, `wake_prompt`'s Callback/Due/Condition variants.
- `WakeKind` and the `Serve`, `Job`, `Compact` wakes (job done = kernel inbox file; compact = `turns/tNNNN/compact` marker; serve = reclaim from turn folders).
- `hooks.inbox(agent, Node)`; producers call `inbox::deliver(Message)` directly.
- `say mode=note`; `hooks.sent` dedupe (the file is the receipt).
- The `wake` marker file and `needs_serve`'s transcript scan.
- `subscriptions.json` (single place-level file) → per-agent folder.
- The double `Say` write in the GitHub door (`github.rs:321-327`).
- `TurnControl.steer` queue and `requeue_steers`.
- `Frame::PlanOp` → `Frame::InboxOp` (cancel/run a file) and `Frame::SubscriptionOp`.

## 5. Target model, in Cursor's vocabulary

- **Coordinator** = `root`. It reads, plans, spawns, steers, asks. It does not edit code (`role = "coordinator"`).
- **Goals** live in `.arbos/GOALS.md` (= `docs/project-context.md`). Root owns it. Every agent gets it in its prompt every turn.
- **Status** lives in `agents/root/plan.md` (= `notes.md`): a checkbox list root rewrites after every state change, with links to child ids. The right-hand panel shows it.
- **Plans** are documents in `.arbos/shared/` (= `docs/`). A worker's own checklist is `agents/<id>/plan.md` (= TodoWrite).
- **Scheduling** is subscriptions only: `agents/<id>/subscriptions/*.toml` with `kind`, `prompt`, `every`/`at`/`once`, source fields (`repo`, `pr`, `cmd`), `deliver_to = agent | user`, `expires`. A plan node with `every` becomes a `timer` subscription whose prompt is the old goal text. There is no other clock.
- **Messaging** is inbox files. `queue` = a file with `wake = true`. `steer` = a file with `kind = "steer"`, read at the next tool boundary. The user's prompt, a peer's words, a subscription firing, a child's completion, and an answer are all the same file shape.
- **Agents** are created with `spawn` (= `CreateAgent`), steered with `say mode=steer` (= `SendToAgent steer`), queued with `say mode=queue` (= `SendToAgent queue`), stopped with Stop (= `StopAgent`), read with `status.toml` and `transcript` (= `GetAgentStatus`, `ReadAgentTranscript`). Completion is a kernel-written `done` message (= completion notification).
- **Shared context** is `.arbos/` as a git repo (= Agent Store). Remote workers push and pull it.
- **Machines** are `machines.toml` entries that register outbound with the hub (= self-hosted workers); `spawn machine=<name>` claims one.
- **Risky actions** are held by `mode: ask` and hooks exit 3 (= hold-risky-actions rule), on disk.

Concrete renames: `say mode=request` → `mode=queue`; `spawn isolate/host` → `machine`; `plan.jsonl` → `plan.md` (agent-written); `subscriptions.json` → `agents/<id>/subscriptions/`; `user.md` → `user/inbox/`; `Frame::PlanOp` → `Frame::InboxOp` + `Frame::SubscriptionOp`; `WakeKind::Plan` prompt text → the subscription's `prompt`.

Concrete deletions: everything in the Remove list. Roughly `node.rs` (900 lines), most of `plan.rs` (~700 of 977), `NewNode` in `hooks.rs` (~210 lines), `wake.rs`, `control.rs` steer queue.

## 6. Migration on top of the design's phases

| Phase | Was | Now |
| --- | --- | --- |
| 1 runtime split + nested git | done | **Stays** |
| 2 inbox files | done for messages; steer in memory | **Stays**; finish: steer file, producers call `inbox::deliver` directly, GitHub double-write fixed |
| 3 turn folders + `status.toml` | as written | **Stays**; plus the kernel-written `done` message to the parent; `user.md` → `user/inbox/` |
| 4 plan files (`plan/*.md` with `when × do`) | one Markdown file per node | **Changes**: (a) `plan` tool → checklist writing `plan.md`; (b) `subscriptions/*.toml` + one poller loop (`timer`, `github_pr`, `github_ci`, `shell`, `telegram`); (c) one-time migrator: `plan.jsonl` recurring/`after` nodes → subscriptions, open one-shot nodes → checklist lines, done nodes → dropped (git has them); (d) delete the Remove list |
| 5 long-running context | GOALS.md, checkpoints, journal, segments, grep scope, brief pointers | **Stays**; move 5a (`GOALS.md`) to right after 3, since it is Cursor's plan surface |
| 6 waiting files | ask parks or blocks (open question 8) | **Stays, simplified**: ask parks (Cursor shape); approve blocks with a disk mirror |
| 7 attach over files + hub | as written | **Stays**; add outbound worker registration = self-hosted workers |

Order: 2-finish → 3 → 5a → 4 → 6 → 5b-d → 7. Phase 4 is the only invasive step; it touches `node.rs`, `plan.rs`, `hooks.rs`, `tools.rs`, `prompt.rs:9-10`, `github.rs`, the desktop plan panel, and the `cron-fires-and-reports` fixture.

## 7. Decisions for Jacob

1. **Delete the plan engine?** Replace `Node`/`When`/`Do` with `plan.md` checklist + `subscriptions/`. Recommend **yes**. It matches Cursor exactly and removes ~1 800 lines and two in-memory state holders.
2. **Root is a coordinator that does not edit code?** Recommend **yes**, default on for new places, off for existing ones (`project.toml [root] role`). Cost: one spawn for a one-line fix. Gain: root's context lasts for years.
3. **Keep kernel-run shell jobs with no model turn?** Cursor has none. Recommend **keep**, as `subscribe kind=shell` with `deliver_to = user`. It is the cheapest cron there is and already works.
4. **Ask parks (ends the turn) or blocks?** Recommend **park**. It is Cursor's shape, restart-safe, and a file. Approve stays blocking with a disk mirror.
5. **Keep worker-to-worker `say`?** Cursor forbids it. Recommend **keep**, with `hops` (default 3) and the roster. It saves the coordinator a relay turn and is one file write.
6. **Kernel writes the child's completion to the parent?** Today the child must remember to `say`. Recommend **yes**: a `kind = "done"` inbox file at every child turn end, carrying the last reply or a pointer. It is Cursor's completion notification, on disk.
