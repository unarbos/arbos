# Arbos: file-system-first state design

Design only. No code was changed. Audit is of `origin/rust` at commit `c066c37`. Written 2026-09-12.

## Summary

1. Arbos already keeps most durable state on disk under `<project>/.arbos/`: agent folders, transcripts, plans, jobs. That part is good and stays.
2. The complexity is in the middle layer: every message, prompt, cron, and callback is squeezed into one `Node` type with two axes (`when` × `do`), and a two-phase claim protocol tracks it partly on disk and partly in memory.
3. Six in-memory maps in the kernel hold live truth that no file shows: which turn a plan node started, who is waiting for a user answer, who is waiting for a bash approval, which messages were already sent this turn, and two UI-poll caches.
4. Proposal: split "messages" from "plans". A message is one file dropped into an agent's `inbox/`. A plan is one file per goal in `plan/`. A turn is one folder in `turns/`. The kernel's job shrinks to: watch inboxes and clocks, start turns, record results.
5. To wake an agent you write a file. To read its status you read a file. To find other agents you list a directory. A generated `PROTOCOL.md` in `.arbos/` tells any reader how. Crons are plan nodes with `every` or `at`; no second mechanism.
6. The main chat grows without limit. Its transcript is append-only in sealed 8 MB segments, with a byte-offset index and a per-turn `journal.md`. Compaction summaries are files in `checkpoints/`. The kernel loads only the latest checkpoint plus the tail; it never reads the whole log for a step.
7. A sub-agent starts from a short brief file with pointers, reads `.arbos/GOALS.md` first (one master goals file per project, written only by the main chat), then pulls what it needs with `grep` over a persistent tgrep index of the whole `.arbos/` history.
8. The kernel keeps in memory only process handles it cannot put on disk: live model streams, child processes, client sockets. Everything else is a cache rebuilt from files.
9. `.arbos/` becomes its own nested git repository. The kernel commits after every turn. Rewind is `git checkout` of that repo plus a kernel restart. Tests author a `.arbos/` folder by hand, run `arbos-kernel serve --until-idle --provider replay`, and assert on files.
10. Seven migration phases, each shippable on its own. Phase 1 (nested git + `runtime/` split) gives git save and rewind before anything else changes.

## Current system audit

Terms used here:

- **Place**: one project folder the kernel serves. Its state lives in `<place>/.arbos/`. (`crates/arbos-core/src/place.rs:5-8`)
- **Agent**: one chat or sub-agent. One folder under `.arbos/agents/<id>/`. (`crates/arbos-core/src/agent.rs:48-63`)
- **Turn**: one run of the model loop, from a wake to `TurnComplete`. (`crates/arbos-engine/src/turn.rs:82-85`)
- **Node**: one plan entry. Also, today, one message. (`crates/arbos-core/src/node.rs:159-196`)
- **Wake**: an in-memory struct that says "run a turn for this agent now". (`crates/arbos-core/src/wake.rs:8-20`)
- **Kernel**: the `arbos-kernel serve` process. One per place, held by a file lock. (`crates/arbos-kernel/src/serve.rs:33-39`, `crates/arbos-core/src/lock.rs:10-42`)

### What is on disk today

Per place, `.arbos/` holds (`crates/arbos-core/src/place.rs:19-53`):

| Path | Written by | Format | Notes |
| --- | --- | --- | --- |
| `agents/<id>/` | kernel | dir | one per agent |
| `archive/` | nobody yet | dir | created, unused |
| `kernel.json` | kernel | JSON | `{url: "tcp://127.0.0.1:port", pid}` (`serve.rs:464-470`) |
| `focus` | kernel, desktop | text | which agent the window shows (`files.rs:129-138`) |
| `user.md` | kernel | Markdown list | notices to the user, append by full rewrite (`hooks.rs:850-857`) |
| `lock` | kernel | text | pid, `flock` held (`lock.rs:18-41`) |
| `hooks/before-tool`, `hooks/after-turn` | user | executables | (`crates/arbos-engine/src/tools/file_hooks.rs:1-10`) |
| `checkpoint` | engine | text | project git `HEAD` sha at turn start, for `undo` (`tools/git.rs:45-62`) |
| `desktop/sessions/*.json` | desktop only | JSON | drafts, ranks, cached transcript (`desktop/src/model/record.rs:1-8`) |
| `sessions.db`, `web.json`, `board.json` | old Go kernel | SQLite, JSON | still probed by the desktop (`desktop/src/kernel.rs:636-671`) |

Per agent, `agents/<id>/` holds (`crates/arbos-core/src/files.rs:16-69`):

| File | Written by | Format | Notes |
| --- | --- | --- | --- |
| `agent.md` | kernel, desktop | `key: value` lines | name, title, parent, paused, model, allowlist, readonly, cwd. Atomic tmp+rename. (`agent.rs:100-122`, `agent.rs:188-200`) |
| `transcript.jsonl` | kernel, other agents | JSONL, append-only | one `Event` per line; line number is the id (`event.rs:3-15`, `files.rs:146-172`) |
| `plan.jsonl` | kernel | JSONL, log | one `Node` per line, **last line per id wins** (`node.rs:341-370`) |
| `attempts.jsonl` | kernel | JSONL, log | one `Attempt` per line, last per id wins (`node.rs:372-378`) |
| `plan.md` | kernel | Markdown | render of the plan; humans read it, nobody parses it (`hooks.rs:286-294`) |
| `jobs/jN/` | engine | folder | `meta.json`, `out.log`, `exit`, `seen`, `detached`, `notified`. Status derived, never stored. (`crates/arbos-engine/src/jobs.rs:1-16`) |
| `pages/`, `images/` | tools | files | screenshots and pages |
| `wake` | nobody writes it | marker | `needs_serve` checks it exists (`files.rs:231-235`) |
| `trace/` | provider, opt-in | JSON | raw provider calls |

Outside the place:

- `~/.config/arbos/config.toml` and `~/.config/arbos/places` (`crates/arbos-engine/src/host.rs:146-165`).
- tgrep search index in `~/.cache/arbos/tgrep/<hash-of-path>/` (`crates/arbos-kernel/src/grep.rs:106-116`). A cache; rebuilt at every kernel start (`grep.rs:24-44`).
- `.arbos/` is in the project's `.gitignore` (`.gitignore:3`). Nothing under it is git-saved today.

### How agents are created

- `bootstrap` makes `.arbos/`, `agents/root/`, `user.md`, `focus` (`files.rs:72-99`).
- The desktop `+` button calls `create_chat`: a new `chat-<ms>` folder copied from root's model and allowlist (`files.rs:104-127`).
- The `spawn` tool makes a child: slug id from the brief, `parent` set in `agent.md`, allowlist narrowed to the parent's, and one inbox node with origin `spawn:<parent>` (`crates/arbos-kernel/src/hooks.rs:670-721`). Caps: 8 children, depth 3 (`crates/arbos-kernel/src/sched.rs:11-12`).
- Parent/child is only the `parent:` line in `agent.md`. Children are found by scanning every `agent.md` (`hooks.rs:96-103`, `hooks.rs:548-565`).

### How agents are woken

There is no single wake path. There are four:

1. **Plan scan.** A `kick` channel message makes `plan::scan` read every agent's `plan.jsonl`, compute what is fireable, claim it, and return `Wake`s (`crates/arbos-kernel/src/plan.rs:112-175`). Kicks come after every plan write and every 5 s (`serve.rs:146`, `serve.rs:219-222`).
2. **Housekeeping wakes** on a separate `wake_tx` channel: `Serve` (continue a turn a dead kernel left open) and `Compact` (`serve.rs:113-121`, `serve.rs:360-364`).
3. **Job wakes.** Every 200 ms the loop lists every agent's jobs, and a finished detached job sends a `Job` wake if the agent is idle (`serve.rs:223-275`).
4. **Steer.** A user message while a turn runs bypasses all of the above and goes into an in-memory queue on `TurnControl` (`serve.rs:314-321`, `crates/arbos-engine/src/control.rs:17-25`, `turn.rs:238-247`).

A user prompt, a `say` request, a spawn brief, a Telegram message, and a cron firing are all **inbox nodes**: a root node with `when.wake = true` (`node.rs:224-230`, `hooks.rs:423-443`, `doors.rs:166-196`). The function `is_inbox` then decides, by six conditions, whether a node is a message or a goal (`node.rs:642-653`).

### How plans are represented and run

- A `Node` has `when` (after, every, next_due, wake, condition) and `do` (agent, shell, notify, ask), plus status, parent, seq, origin, hops, attempt, outcome (`node.rs:93-196`).
- Status graph: pending, active, blocked, done, cancelled, failed (`node.rs:429-459`).
- Sibling order is the only dependency (`node.rs:461-471`).
- `NewNode::build` validates a node from the tool call in ~110 lines. It includes sniffing the goal text for words like "every ", "hourly", "in 30m" and refusing the node (`hooks.rs:154-264`).
- The two-phase run: `scan` picks a node → `claim` re-reads under `plan_lock`, opens an `Attempt`, marks the node active (`plan.rs:180-214`) → stores a `TurnMeta` **in memory** in `Clock.turns` (`plan.rs:39-45`, `plan.rs:163-171`) → when the turn ends, `finish_turn` reads the transcript lines the turn wrote to derive an outcome, unless the model moved the node itself (`plan.rs:309-378`) → `settle_parents` loops until no parent can be closed (`hooks.rs:314-349`).
- Shell and notify nodes run in the kernel with no model (`plan.rs:477-533`). Condition nodes poll a shell predicate (`plan.rs:557-613`).
- At boot, `reclaim` folds `plan.jsonl`, and any node left `active` goes back to pending or done based on `needs_serve`, a scan of the transcript for a wake without a `TurnComplete` (`plan.rs:62-108`, `files.rs:231-254`).
- The prompt the model sees each step includes a fresh render of the plan and the roster of peers (`crates/arbos-engine/src/prompt.rs:59-107`).

### How agents talk to each other

`say` (`hooks.rs:767-832`):

1. Resolve the target by id, name, or unique substring of a name (`hooks.rs:725-753`).
2. Dedupe against an in-memory set of `(to, text)` per sender per turn (`hooks.rs:834-844`).
3. Append a `Say` event **directly into the target's `transcript.jsonl`** (`hooks.rs:790-797`).
4. If `mode: request`, also add an inbox node to the target's `plan.jsonl` with a hop budget (`hooks.rs:805-822`).

So one message is two writes into another agent's folder, and a reply budget (`hops`) rides in memory from `Wake` to `RunCx` (`wake.rs:17-19`, `turn.rs:210`).

Asks and approvals are one-shot channels in memory: `hooks.asks` and `hooks.approves` (`hooks.rs:37-38`, `hooks.rs:887-922`). The `Ask` tool blocks the turn until the channel resolves (`crates/arbos-kernel/src/tools.rs:277-291`). If the kernel dies, the receiver is gone; the answer frame still appends an `Answer` event but wakes nobody (`serve.rs:365-373`).

### How context is trimmed today

- Two stages, both recorded as transcript events: **fold** (old tool bodies render as a one-line cite) and **compaction** (a model writes a summary; a `Compaction { lo, hi, summary }` line goes on the transcript and the projection shows the summary in place of lines `lo..=hi`) (`crates/arbos-engine/src/compact.rs:1-16`, `crates/arbos-core/src/event.rs:65-83`). Nothing is deleted from disk. Good.
- But the summary text lives only inline in one JSONL line. There is no file a person or a sub-agent opens to read "what happened in this chat so far".
- The whole `transcript.jsonl` is parsed into memory at turn start and again after every step, steer, nudge, and compaction (`crates/arbos-engine/src/turn.rs:119`, `:246`, `:329`, `:424`, `:433`, `:498`; `compact.rs:608`). `finish_turn`, `needs_serve`, `last_usage`, `last_touched_path`, and the 200 ms UI tail do the same (`plan.rs:315`, `files.rs:236`, `serve.rs:435`, `tools/mod.rs:93`, `serve.rs:277`). Cost is linear in the life of the chat, per step. A chat that grows for a year makes every step slower.
- The model's view is bounded (`compact_at`, `keep_recent_tokens`, `host.rs:55-59`), so the model context is fine. The kernel's memory and I/O are not.

### Search index

- `PlaceGrep` builds a tgrep trigram index (a trigram is a three-character substring; the index maps each one to the files that contain it) over the place, with `include_hidden: true`, so `.arbos/` transcripts are searchable (`crates/arbos-kernel/src/grep.rs:24-44`). Good: the pull-context idea already half-exists.
- It is rebuilt from scratch at every kernel start and stored under `~/.cache` (`grep.rs:30-43`, `grep.rs:106-116`). tgrep can persist and overlay (`vendor/tgrep/tgrep-core/src/hybrid.rs:1-13`), but the kernel does not use that.
- `PlaceGrep::upsert` exists (`grep.rs:46-50`) and nothing calls it. Lines appended during a session are invisible to `grep` until the next restart.
- tgrep skips files over 64 MB (`vendor/tgrep/tgrep-core/src/walker.rs:43`). A long-lived single `transcript.jsonl` drops out of the index entirely once it crosses that.
- The `grep` tool has no scope: one query covers code and history together.

### Sub-agent kickoff today

- `spawn` takes `brief`, `model`, `readonly`, `cwd` (`tools.rs:57-93`). The child's first prompt is the brief wrapped in a fixed template that names the parent and the child's folder (`plan.rs:222-232`).
- No pointers to context: the parent must paste anything the child should know into the brief text, or the child greps blind. There is no per-project goals file; the closest thing is `AGENTS.md` read from the project root into every prompt (`prompt.rs:154-163`).
- The child reports back with `say`, which appends the full text to the parent's transcript (`hooks.rs:790-797`). A verbose child grows the parent's context.

### What lives only in process memory

| Holder | What | Effect if lost |
| --- | --- | --- |
| `Clock.turns: HashMap<agent, TurnMeta>` (`plan.rs:40-45`) | which plan node and attempt a running turn belongs to, and its first transcript line | `finish_turn` cannot close the node; `reclaim` guesses at boot |
| `Clock.mech: HashSet` (`plan.rs:41-42`) | shell/condition runs in flight | double-run after restart is prevented only by `active` status on disk |
| `Scheduler.in_flight: HashMap<agent, TurnControl>` (`sched.rs:15-17`) | cancel token, steer queue, compact flag per turn | a queued steer is lost |
| `KernelHooks.running: HashSet` (`hooks.rs:43-44`) | agents with a turn running | rebuilt from `in_flight` |
| `KernelHooks.asks`, `approves` (`hooks.rs:37-38`) | who waits for the user | the question is orphaned |
| `KernelHooks.sent` (`hooks.rs:45-47`) | messages sent this turn | none |
| `KernelHooks.frames` (`hooks.rs:36`) | attached client sockets | clients reconnect |
| serve loop `tails`, `announced` (`serve.rs:148-151`) | how many transcript lines each client saw; which jobs were shown | full replay to clients |
| `PtyHub`, `BrowserHub` (`pty.rs:12-17`, `browser.rs:24-30`) | terminal and Chrome child handles | processes orphaned; no file records them |
| `Wake` structs in channels (`serve.rs:58-61`) | pending wakes | reconstructed by `scan` |
| `PlaceGrep` (`grep.rs:16-20`) | search index | rebuilt at start |

### Attach protocol

- Kernel binds `127.0.0.1:0`, writes the port to `kernel.json`, and speaks newline-delimited JSON `Frame`s (`serve.rs:52-56`, `crates/arbos-kernel/src/attach.rs:10-36`). Twenty-two frame variants (`crates/arbos-core/src/wire.rs:7-118`).
- To push events, the loop re-reads **every agent's whole `transcript.jsonl` every 200 ms** and sends lines past the count it remembers (`serve.rs:276-288`).
- The desktop finds a remote kernel over ssh: reads `kernel.json` on the host, starts the kernel with `setsid nohup` if needed, and opens `ssh -N -L` (`desktop/src/kernel.rs:1-7`, `desktop/src/kernel.rs:2128-2131`). It lists remote agents with an `awk` script over `agent.md` files (`desktop/src/kernel.rs:708-755`).
- The desktop also keeps its own per-session JSON under `.arbos/desktop/sessions/` and still merges sessions from the old Go kernel's HTTP gateway (`desktop/src/kernel.rs:636-665`). Three sources for "what agents exist".

### What is complex today, and why

1. **One type carries two jobs.** A `Node` is a message and a goal. That forces `is_inbox` (six conditions), the `origin` string prefixes (`user`, `agent:`, `spawn:`, `kernel`, `node:`), the `hops` field, and the `wake_for` switch that turns a node back into the right kind of prompt (`plan.rs:216-247`). A message should be a file you can `cat`.
2. **Two-phase claim with half the state in memory.** `claim` writes to disk, `TurnMeta` stays in RAM, `finish_turn` joins them. `reclaim` exists to repair the join after a crash. If the turn folder held its own meta, there would be nothing to repair.
3. **Log files with fold rules.** `plan.jsonl` and `attempts.jsonl` are "last line per id wins". A human or a second program cannot read a node's state without applying the rule. `compact_nodes` runs at boot to make the file readable again (`node.rs:354-370`).
4. **Four wake paths, three channels, two timers.** Wakes, kicks, done, frames-in, a 5 s tick, and a 200 ms tail all meet in one `select!` (`serve.rs:185-296`). Each path has its own idle check.
5. **Blocking tools hold memory.** `ask` and `approve` park a turn on a one-shot channel. Nothing on disk says "waiting on the user for question X". A phone client cannot see or answer it after a kernel restart.
6. **The kernel writes into other agents' files.** `say` appends to a peer's transcript. Any agent that can call `say` writes a peer's history. It works because one kernel serialises it, but it does not survive two writers (a remote client, a second process).
7. **Polling for change.** 200 ms full re-reads of every transcript for the UI, 5 s plan scans, job sweeps per agent per tick. Cost grows with agents × transcript size.
8. **Nothing is git-saveable.** `.arbos/` is ignored. `lock`, `kernel.json`, `checkpoint`, tgrep cache, and Chrome profile mix with durable state, so you could not commit the folder as it stands.
9. **No state-driven tests.** `crates/arbos-core/tests/contracts.rs` checks wire round-trips, the status graph, and the lock. Nothing writes a `.arbos/` and runs the kernel on it.
10. **Whole-log reloads.** Every step re-parses the entire transcript. Summaries are buried in JSONL lines. The search index forgets everything at restart and cannot see a file over 64 MB. None of this breaks a one-week chat; all of it breaks a one-year chat.

What is already right and stays: append-only `transcript.jsonl` with one `O_APPEND` write per batch (`files.rs:146-172`); atomic tmp+rename for `agent.md` (`agent.rs:192-200`); jobs as folders with derived status (`jobs.rs:1-16`); `mkdir` as the id allocator (`jobs.rs:289-310`); the place `flock` (`lock.rs`); hooks as executables in a directory; the plan render in the prompt.

## Design goals

1. **Every fact has a file.** If the kernel knows it, a `cat` or `ls` shows it. Memory holds only OS handles and caches.
2. **Write a file to act.** Wake, message, ask, answer, approve, schedule: each is one file dropped in one place.
3. **One kernel per place, one writer per file.** Each file has a named owner. Others append to inboxes or use the lock.
4. **Never half-written.** Every file is either absent, or complete. Readers never see a partial file.
5. **Git-saveable and rewindable.** `.arbos/` commits cleanly. A checkout plus a kernel start resumes.
6. **Human-readable first.** Markdown and TOML for anything a person or a model edits. JSONL only for machine logs.
7. **Discoverable.** A stranger (a person, a non-Arbos agent) can read `PROTOCOL.md` and join.
8. **Testable by authoring.** A fixture folder is a complete test input.
9. **Remote-safe.** Clients are views. They act through the kernel, which does the writes. See "Remote attach and sharing".
10. **Grows without limit.** Logs are append-only and indexed. No step reads a whole log. Context moves by pointer, not by paste: a sub-agent gets a brief and pulls the rest.
11. **One goals file.** Every agent reads `.arbos/GOALS.md` first. Only the main chat writes it.

## On-disk layout

Sample tree for a place with a root agent, one sub-agent, and one cron:

```text
myproject/
├── .git/                       the project's own repo (unchanged)
├── .gitignore                  contains `.arbos/`  (unchanged)
└── .arbos/                     ← its own nested git repo (.arbos/.git)
    ├── .gitignore              `runtime/`, `*/jobs/*/out.log` over the cap, `*/trace/`
    ├── PROTOCOL.md             generated by the kernel: how to read, message, wake
    ├── GOALS.md                the master goals file; owner: the main chat (root); every agent reads it first
    ├── project.toml            project id, name, defaults (model, allowlist), schema version
    ├── agents.md               generated roster: id, name, parent, state, last turn
    ├── shared/                 files every agent may read and write (research, decisions, handoffs)
    │   ├── notes.md
    │   └── research/voice-models.md
    ├── user/                   the person, as a participant
    │   ├── inbox/              notices and questions for the user
    │   │   └── 2026-09-12T21-40-03Z-root-0007.md
    │   └── answered/           questions the user answered (kernel moves them here)
    ├── agents/
    │   ├── root/               the main chat. Grows for years.
    │   │   ├── agent.toml      identity and config; owner: kernel (desktop edits via kernel)
    │   │   ├── status.toml     idle | running | paused | waiting; owner: kernel; atomic
    │   │   ├── inbox/          messages and wake requests TO root; anyone may drop a file
    │   │   │   ├── .tmp/       staging; readers ignore dot-dirs
    │   │   │   └── 2026-09-12T21-52-10Z-user-0009.md
    │   │   ├── plan/           one file per goal; owner: agent + kernel, under plan/.lock
    │   │   │   ├── .lock
    │   │   │   ├── 0001-ship-voice-mode.md
    │   │   │   ├── 0002-daily-slop-scan.md      (every = "1d" → this is a cron)
    │   │   │   └── 0003-wait-for-jacob.md       (do = "ask")
    │   │   ├── plan.md         generated render of plan/; read-only view
    │   │   ├── transcript/     append-only event log in sealed segments; owner: kernel
    │   │   │   ├── segments.toml   segment → first global line, bytes, sealed?
    │   │   │   ├── 0001.jsonl      sealed at 8 MB; never changes again
    │   │   │   ├── 0002.jsonl      the open segment; appends go here
    │   │   │   └── 0002.offsets    byte offset per line; random access without a scan
    │   │   ├── journal.md      one line per turn: time, turn, cause, outcome, files touched; owner: kernel
    │   │   ├── checkpoints/    compaction summaries as files; owner: kernel
    │   │   │   ├── INDEX.md    one line per checkpoint: id, date span, lines lo..hi, title
    │   │   │   ├── ck0006.md
    │   │   │   └── ck0007.md   the latest; the model's view starts here
    │   │   ├── turns/          one folder per turn; owner: kernel
    │   │   │   ├── t0041/
    │   │   │   │   ├── meta.toml   cause, node, started, ended, verdict, lines lo..hi, repo_head, files
    │   │   │   │   └── cause.md    the inbox file that started it (moved here = the claim)
    │   │   │   └── t0042/
    │   │   │       └── meta.toml   (no `ended` yet → running, or dead if pid is gone)
    │   │   ├── jobs/           unchanged: jobs/jN/{meta.json,out.log,exit,...}
    │   │   ├── pages/  images/ unchanged
    │   │   └── waiting/        parked questions to the user or approvals; owner: kernel
    │   │       └── approval-c17.toml
    │   └── fix-linux-build/    a sub-agent. Same shape, short life.
    │       ├── agent.toml      parent = "root"
    │       ├── status.toml
    │       ├── inbox/
    │       ├── plan/
    │       ├── plan.md
    │       ├── transcript/
    │       ├── journal.md
    │       ├── checkpoints/
    │       └── turns/
    │           └── t0001/cause.md   the brief: task + pointers, not a dump
    ├── hooks/                  unchanged
    ├── skills/                 unchanged
    └── runtime/                NOT committed. Process facts and caches only.
        ├── lock                flock, pid
        ├── kernel.toml         listen addresses, pid, started, version   (was kernel.json)
        ├── focus               UI focus for local clients (per client later)
        ├── pids/               chrome, ptys: `<page>.pid`
        ├── chrome-profile/
        └── cache/tgrep/        persistent trigram index of the project and all of .arbos/
```

What moved, and why:

- `kernel.json`, `lock`, `checkpoint`, `focus` → `runtime/`. They describe a process, not the project. They must never be committed.
- `user.md` → `user/inbox/*.md`. One file per notice. Appending by full rewrite is gone.
- `plan.jsonl` + `attempts.jsonl` → `plan/*.md` + `turns/*/meta.toml`. Current state per file; history per folder. No fold rule.
- `wake` marker → any file in `inbox/`. The marker had no writer.
- `attempts.jsonl` for shell nodes → the job folder itself. A shell attempt is a job; `jobs/jN/meta.json` gains a `node` field.
- `transcript.jsonl` → `transcript/NNNN.jsonl` segments plus `segments.toml` and `.offsets`. Same line format. A sealed segment is immutable, so the index and git handle it once and never again.
- Compaction summaries → `checkpoints/ckNNNN.md`. The transcript keeps a one-line pointer event. A person or a sub-agent can read the summary as a document.
- New: `GOALS.md`, `journal.md`, `checkpoints/INDEX.md`. All three exist so that a reader can find the right place in a long history in one or two reads.
- The tgrep cache → `runtime/cache/`. Same machine, same place, persistent across kernel starts.

## File formats

Rule: **TOML for structured things a person edits, Markdown with a TOML front matter for things with a body, JSONL for machine logs that only grow.** JSON stays only where a program is the sole reader and writer (`jobs/*/meta.json`, the wire).

Why TOML over JSON for config: comments, no trailing-comma errors, diffs one key per line. Why TOML over YAML: no indentation traps, no `no` → `false` surprises, one parser already in the tree (`host.rs` uses `toml`). Why Markdown for messages and goals: the model writes and reads prose well; humans too; git diffs are readable.

Front matter uses `+++` fences (the Hugo convention for TOML). Parsers: split on the first two `+++` lines, then `toml::from_str`.

### `agent.toml`

```toml
id = "fix-linux-build"          # must equal the folder name
name = "fix linux build"
title = ""                      # sidebar label, set from the first prompt
parent = "root"                 # "" for a top-level chat
model = "inherit"
allowlist = ["ls", "read", "grep", "edit", "bash", "say", "plan"]
readonly = false
cwd = "/home/jacob/myproject"
paused = false                  # stays here: it is config, not live state
created = 2026-09-12T21:50:01Z
```

Same fields as `agent.md` today (`agent.rs:49-63`). Owner: the kernel. Clients change it through the kernel (`Frame::SetModel`, `Frame::Pause`) so two writers never race.

### `status.toml` (generated, atomic, small)

```toml
state = "running"               # idle | running | paused | waiting
turn = "t0042"                  # when running
since = 2026-09-12T21:52:11Z
waiting_on = ""                 # "user:approval-c17" when state = waiting
last_turn = "t0041"
last_turn_ended = 2026-09-12T21:49:30Z
context_used = 41200            # from the last TurnComplete usage
context_size = 128000
```

This is a **cache**. Truth is `turns/` and `transcript/`. The kernel rewrites it on every state change. A reader that wants speed reads this; a reader that wants certainty reads `turns/`.

### Inbox message: `inbox/<utc-time>-<from>-<seq>.md`

Filename gives order and sender at a glance. `<seq>` is a per-sender counter so two files in the same millisecond do not collide.

```markdown
+++
from = "agent:root"             # user | user:<name> | agent:<id> | cron:<node> | kernel | answer:<waiting-id>
kind = "request"                # message | request | brief | answer | approval | wake
wake = true                     # start a turn if idle (false = read at the next turn)
reply_to = ""                   # inbox filename this answers, when any
hops = 2                        # reply budget left for agent-to-agent chains
attachments = ["images/shot-1.png"]
sent = 2026-09-12T21:52:10Z
+++
Please run the Linux build with the x11 feature and report the first error.
```

Kinds:

- `message`: a note. Read at the next turn. Replaces `say mode:note`.
- `request`: a note that wants a turn. Replaces `say mode:request`.
- `brief`: the first message to a spawned child. Replaces the `spawn:` origin. Carries pointer fields; see below.
- `answer`: the user's reply to a question in `waiting/`.
- `approval`: `allow = true|false` for a `waiting/approval-*.toml`.
- `wake`: no text needed. "Run a turn." Replaces the `Serve` and `Job` wakes and the bare `wake` marker.

### Brief: the kickoff message to a sub-agent

A brief is an inbox file with `kind = "brief"` and pointer fields. The body is short. The pointers are where the weight is.

```markdown
+++
from = "agent:root"
kind = "brief"
wake = true
sent = 2026-09-12T21:50:01Z
project = "/home/jacob/myproject"                  # the folder to work in (child's cwd)
goals = ".arbos/GOALS.md"                          # always; the kernel adds it if missing
task = "Make the Linux desktop build pass with the x11 feature"
context = [                                        # read these before anything else, in order
  ".arbos/agents/root/checkpoints/ck0007.md",     # the parent's latest summary
  ".arbos/agents/root/turns/t0038/",              # the turn where the bug was found
  ".arbos/shared/research/gpui-linux.md",
]
look_in = [                                        # where to grep when you need more
  "desktop/src/",
  ".arbos/agents/root/journal.md",
]
report_to = "agent:root"                           # where the result goes
report_as = "pointer"                              # pointer | text: write the result to a file and say the path
+++
What you should know, compressed: gpui on Linux needs the `x11` or `wayland`
feature on `gpui_platform`. Jacob's machine is X11. The build failed at link
time on 2026-09-11; see turn t0038 for the exact error. Do not touch macOS code.
```

The kernel warns on the parent's transcript when a brief body is over ~2 000 tokens: that is a dump, not a brief. The child's first system message says: read `goals`, then `context` in order, then grep `look_in` as needed.

### Transcript: `transcript/NNNN.jsonl`, `segments.toml`, `NNNN.offsets`

The event format is unchanged (`event.rs:17-96`). What changes is the container.

- `NNNN.jsonl`: a segment. The kernel appends to the highest one. When it passes 8 MB (configurable in `project.toml`), the kernel seals it: it writes `sealed = true` in `segments.toml` and opens the next. A sealed segment never changes again.
- `segments.toml`:

```toml
[[segment]]
file = "0001.jsonl"
first_line = 1                  # global line number of this segment's first line
lines = 48211
bytes = 8388402
sealed = true
from = 2026-01-04T09:00:00Z
to = 2026-06-30T17:12:44Z

[[segment]]
file = "0002.jsonl"
first_line = 48212
sealed = false
```

- `NNNN.offsets`: 8 bytes per line, little-endian, the byte offset of that line's start in the segment. Appended in the same write batch as the JSONL line. Reading global line N = find the segment in `segments.toml`, seek to `offsets[(N - first_line) * 8]`, read one line. If the offsets file is short or missing, one scan of the segment rebuilds it.

Cites keep the form `transcript:1181` (a global line number). The `read` tool resolves them. `grep -n` on a segment gives a local line; add the segment's `first_line - 1`. `arbos-kernel cat <agent> 1181..1240` prints a range for humans.

The `EventKind` gains three small variants: `Inbox { file, text }` when a message is consumed, `Turn { id, phase }` at start and end, and `Compaction { lo, hi, file }` replaces the inline summary with a pointer to a checkpoint file.

### Checkpoint: `checkpoints/ckNNNN.md`

One file per compaction. The transcript keeps a pointer; the text lives here.

```markdown
+++
id = 7
lo = 40120                      # global transcript lines this summary replaces
hi = 47990
from = 2026-06-02T08:00:00Z     # time span of those lines
to = 2026-06-29T18:40:00Z
tokens_before = 91000
tokens_after = 3800
model = "openai/gpt-5-mini"
turn = "t0311"                  # the turn during which the compaction ran
previous = 6                    # the checkpoint this one continues from
+++
# Voice mode: model chosen, streaming pipeline half done

## Decisions
- 2026-06-05: use Moshi via the open API; OpenAI realtime only as fallback (Jacob, transcript:40388).
- 2026-06-18: phone app first; desktop parity later (transcript:44102).

## Facts
- The duplex loop lives in `src/voice/duplex.rs`; interruption works on desktop, not on iOS yet.

## Open
- iOS audio session category is wrong; see turn t0302.

## Then what happened (narrative)
...
```

The summariser prompt asks for exactly these four headings so the files are uniform and greppable. Each summary starts from `previous`, so `ck0007.md` alone is enough to understand the chat up to line 47990: a reader never needs the chain. `INDEX.md` is a generated table with one line per checkpoint: id, dates, lines, title.

### Journal: `journal.md`

One line per turn, appended by the kernel when the turn ends. The densest view of a chat's life; a sub-agent reads its tail before it greps anything.

```markdown
- 2026-09-12 21:49 t0041 ← user · "Linux build fails with x11" · outcome: build passes; PR #44 · files: desktop/Cargo.toml, desktop/src/main.rs · lines 1181..1240
- 2026-09-12 21:58 t0042 ← cron:0002 · daily slop scan · outcome: 3 findings, PRs #41-43 · lines 1241..1402
```

`turns/tNNNN/meta.toml` gains `files = ["desktop/Cargo.toml", ...]`: every path the turn wrote, taken from the `Tool` events' `paths`. "Who touched this file, and when" is then a grep over `journal.md` or `turns/*/meta.toml`, not over transcripts.

### `GOALS.md`: the master goals file

- **Location**: `.arbos/GOALS.md`. One per project, next to `PROTOCOL.md`. Not under `shared/`: it has one owner, and `shared/` is for everyone.
- **Owner**: the main chat (`root`). The kernel enforces it: `write`, `edit`, and `apply_patch` on this path from any other agent fail with "GOALS.md is owned by root; propose the change with `say to=root`". A person may edit it in any editor. Remote clients edit it through `Put` with the owner role.
- **Readers**: every agent, every turn. The kernel injects it as a system message right after `CONTRACT` and before the instance prompt (`project.rs:251-255`), so it is in the cached prefix. Cap: 4 000 tokens; over that the kernel writes a `Notice` to root ("GOALS.md is 5 200 tokens; trim it or move detail to `shared/`") and injects the first 4 000.
- **Format**: Markdown with a small TOML front matter and fixed headings, so a grep for `## Decisions` works across projects.

```markdown
+++
owner = "root"
updated = 2026-09-12T21:40:00Z
version = 14                    # the kernel bumps it on every write; briefs may pin it
+++
# myproject

## Goal
Talk to Arbos in full duplex from the phone and the desktop.

## Constraints
- Open-source speech model first; OpenAI realtime as fallback.
- iOS builds run on the MacBook, not in the cloud.

## Decisions (dated, newest first)
- 2026-09-12: file system holds the state; kernel code stays light.
- 2026-09-12: one main chat per project; sub-agents for sub-problems.

## Current focus
- Phase 1 of the file-system migration.

## Not doing
- Multi-folder projects.

## Where things are
- Research: `.arbos/shared/research/`. Decisions log: this file. History: `agents/root/journal.md`, `agents/root/checkpoints/INDEX.md`.
```

This is the same idea as this Project's own `docs/project-context.md`: stable goals and decisions, not progress. Progress lives in `plan.md` and `journal.md`.

### Plan node: `plan/NNNN-slug.md`

```markdown
+++
id = 2
parent = 0                      # 0 = root goal
seq = 1                         # order among siblings; equal seq = run in parallel
status = "pending"              # pending | active | blocked | done | cancelled | failed
do = "agent"                    # agent | shell | notify | ask
every = "1d"                    # recurrence (this makes it a cron)
at = "09:00"                    # optional: local wall-clock anchor for `every`
after = 2026-09-13T09:00:00Z    # optional: not before this instant (absolute, not "30m")
condition = ""                  # shell predicate polled on `every`; do fires when it exits 0
shell = ""                      # for do = "shell"
notify = ""                     # for do = "notify" or the report after a shell
report_to = "user"              # who gets the notify: user | agent:<id>
check = "cargo build -p arbos-desktop --features x11"
origin = "user"
created = 2026-09-12T21:40:00Z
updated = 2026-09-12T21:49:30Z
next_due = 2026-09-13T09:00:00Z # kernel-owned; the clock
turn = ""                       # kernel-owned; the turn holding it active
+++
# Daily slop scan

Scan the repo for duplicated code and leftover debug prints. Open one PR per finding.

## Outcome
2026-09-12 21:49 — 3 findings, PRs #41 #42 #43.   ← appended by the kernel or the agent, newest last
```

Changes from `Node` today: `after` is an absolute time (a relative "30m" is converted when written, so a rewound or copied state means the same thing). `at` is new: a wall-clock anchor, so "every day at 09:00" is expressible. `hops`, `attachments`, and `origin` prefixes are gone from nodes; they live on messages.

### Turn record: `turns/tNNNN/meta.toml`

```toml
id = "t0042"
agent = "root"
cause = "inbox/2026-09-12T21-52-10Z-user-0009.md"   # moved into this folder as cause.md
node = 2                        # plan node this turn discharges, 0 if none
started = 2026-09-12T21:52:11Z
pid = 48122                     # kernel pid; a dead pid with no `ended` = interrupted
transcript_lo = 1181            # first transcript line this turn wrote
repo_head = "c066c37"           # project git HEAD at start (was .arbos/checkpoint)
# written at the end:
ended = 2026-09-12T21:58:02Z
transcript_hi = 1240
verdict = "success"             # success | fail | interrupted
outcome = "Build passes with x11. Error was a missing feature flag; fixed in PR #44."
verified_by = "self"            # self | kernel | exit
files = ["desktop/Cargo.toml", "desktop/src/main.rs"]   # paths this turn wrote
```

Replaces `Attempt`, `TurnMeta`, `Clock.turns`, and `needs_serve`. "Is a turn running?" = a `meta.toml` with no `ended`. "Did the kernel die mid-turn?" = no `ended` and `pid` not alive. Same test jobs already use (`jobs.rs:339-343`).

### `waiting/approval-<call_id>.toml` and `waiting/ask-<n>.toml`

```toml
kind = "approval"               # approval | ask
agent = "root"
turn = "t0042"
tool = "bash"
command = "rm -rf target"
asked = 2026-09-12T21:53:00Z
question = ""                   # for ask
options = []
```

A copy goes to `user/inbox/` so the user sees it. The answer comes back as an `inbox/` file with `kind = "answer"` or `"approval"` and `reply_to` naming this file. The kernel then deletes the `waiting/` file.

### `project.toml`

```toml
schema = 2                      # bump when the layout changes; the kernel migrates on start
id = "7a3f…"                    # stable across renames and moves
name = "myproject"
[defaults]
model = "inherit"
allowlist = ["ls", "read", "find", "grep", "write", "edit", "apply_patch", "bash", "await", "jobs", "fetch", "search", "spawn", "say", "ask", "plan", "changes", "undo", "browser", "terminal"]
max_children = 24
max_depth = 3
```

### `PROTOCOL.md` (generated)

One page. The kernel writes it at bootstrap and rewrites it when its version changes. Contents: the tree above, the file formats above, and the three verbs: **list** `agents/` to find agents; **write** `agents/<id>/inbox/<name>.md` to message or wake; **read** `GOALS.md` first, then `agents/<id>/status.toml`, `plan.md`, `journal.md`, and `checkpoints/INDEX.md` to know what it is doing; `grep --scope history` and `read transcript:N` for the words. This replaces most of the `CONTRACT` prose about the folder (`prompt.rs:4-9`); the prompt points at the file.

## Agent discovery, messaging, and wake

### Discovery

- `ls .arbos/agents/` is the roster. `agent.toml` gives name and parent. `status.toml` gives live state.
- The kernel also renders `.arbos/agents.md`: one line per agent with id, name, parent, state, and the time of its last turn. It is a view, regenerated on every status change. The desktop sidebar and the `<<peers>>` prompt segment (`prompt.rs:69-88`) read the same files.
- Children: agents whose `agent.toml` has `parent = "<me>"`. Ancestors: follow `parent`. The kernel caps depth and fan-out from `project.toml`.

### Messaging

To send a message from agent A to agent B:

1. Build the file text (front matter + body).
2. Write it to `agents/B/inbox/.tmp/<name>`, `fsync`.
3. `rename` it to `agents/B/inbox/<name>`. The rename is the send. It is atomic on every local file system.

The `say` tool does exactly this and nothing else. It no longer appends to B's transcript (`hooks.rs:790-797` goes away). B's transcript gets the message when B's kernel consumes the file, as an `Inbox` event. One writer per transcript again.

Dedupe (`hooks.rs:834-844`) becomes a check of B's `inbox/` for an identical body from the same sender in the last N minutes. Or drop it: the file is the receipt.

To message the user: write to `.arbos/user/inbox/`. The desktop and the phone show that folder.

### Wake

A wake is an inbox file with `wake = true`. Nothing else. The kernel's watcher (inotify on Linux, FSEvents on macOS, plus a 1 s poll as a fallback) sees the new file and, if the agent is idle and not paused, starts a turn:

1. `mkdir turns/tNNNN` (the allocator, like jobs).
2. `rename inbox/<file>` → `turns/tNNNN/cause.md`. The rename is the claim. If two watchers race, one rename fails and that watcher stops.
3. Write `turns/tNNNN/meta.toml` with `started`, `pid`, `transcript_lo`.
4. Write `status.toml` = running.
5. Run the turn. At the start, append an `Inbox` event with the message text, then the usual `Wake`/`User` events.
6. At the end, rewrite `meta.toml` with `ended`, `verdict`, `outcome`; rewrite `status.toml` = idle.
7. If `inbox/` still has files with `wake = true`, go to 1. Otherwise, files with `wake = false` are consumed at the start of the next turn, whatever causes it.

Steer (a message while a turn runs) is also an inbox file. The turn loop checks `inbox/` at every tool boundary and consumes new files into the transcript (this replaces `control.take_steer()`, `turn.rs:238-247`). The in-memory queue goes away. A steer that arrives while the kernel dies is still in `inbox/` when it restarts.

Stop stays in memory (a cancel token) because it must interrupt a live network stream. It is also written: `Frame::Stop` makes the kernel touch `turns/tNNNN/stop`, and the turn loop checks for it at tool boundaries too, so a stop survives a client disconnect.

Reclaim at kernel start becomes: for every `turns/*/meta.toml` with no `ended` and a dead `pid`, write `ended = now`, `verdict = "interrupted"`, append `Interrupted` and `TurnComplete` to the transcript if the last event is not one, and set `status.toml` = idle. Then let the watcher do its normal first scan of `inbox/`. No guessing from transcript shape (`files.rs:231-254`, `plan.rs:62-108`).

## Plans

A plan is the folder `plan/`. One file per node. The kernel renders `plan.md` from it for the prompt and the UI, exactly as `node::render` does today (`node.rs:609-637`), and the `Frame::Plan` wire shape stays.

Who writes node files:

- **The agent**, through the `plan` tool. The tool becomes thin: it takes the same `add`/`update`/`show` arguments (`tools.rs:139-253`) and writes files under `plan/.lock`. The ~110 lines of goal-text sniffing (`hooks.rs:214-250`) go; instead `plan.md` shows a `(no trigger)` marker, as the render already does (`node.rs:684-695`).
- **The agent, directly.** It may also `write` or `edit` a node file. `arbos-kernel check` (see Testing) lints the folder and the kernel refuses a malformed file with a `Notice` on the transcript rather than a crash.
- **The kernel**, for the fields marked kernel-owned: `status` when it claims or closes a node, `next_due`, `turn`, and the `## Outcome` append.
- **A person**, in an editor. Same rules: hold nothing, save the file, the watcher notices.

Read-modify-write rule: take `flock(plan/.lock)`, read the file, change it, write to `plan/.tmp-…`, `rename`, release. Both the kernel and the `plan` tool do this. An editor does not, so a human save can race a kernel status flip; the loser's change is in git history. That is the accepted cost of hand-editable plans (Open question 6).

Semantics kept from today: sibling order gates (`node.rs:461-471`), roots do not gate each other, recurring nodes never terminate, `done` reopens to `pending`, `cancelled` is final (`node.rs:429-459`), a parent closes when all children close (`hooks.rs:314-349`). Semantics dropped: nodes as messages (`is_inbox`), `hops` on nodes, `wake` flag on nodes (a ready agent node is fired by writing an inbox file `from = "kernel"`, `kind = "wake"`, with the callback text as the body).

Node ids: `NNNN` from the filename, allocated by the writer as max+1 under the lock. The slug is decoration; the kernel matches on the number.

## Crons and scheduling

A **cron** (a job that runs on a schedule) is a plan node with `every`, and optionally `at`. There is no second file and no second mechanism. `plan.md` lists them under "standing", as today.

The watcher is one small loop, ~150 lines, that runs every 5 s (the `tick` today, `serve.rs:146`) and on every plan file change:

1. For every agent, read `plan/*.md` (a directory listing plus small files; cache by mtime).
2. Compute what is due: `status = pending`, not gated by a sibling, `after` passed, `next_due` passed for recurring nodes. Same predicate as `node::fireable` (`node.rs:507-530`).
3. For a `do = "agent"` node: write an inbox file `from = "cron:<id>"`, `kind = "wake"`, body = the callback prompt (`plan.rs:250-283`). Set `status = active`, `turn = ""` (the turn fills it), `next_due = now + every` (missed firings coalesce, as today, `plan.rs:207-210`).
4. For `do = "shell"`: start a job under the agent's `jobs/` with `node = <id>` in `meta.json`. When the job ends (the 200 ms job sweep already exists, `serve.rs:223-275`), set `status` from the exit code, append the tail to `## Outcome`, and if `notify` is set, write a message to `report_to`'s inbox. On failure, write a wake to the agent with the log tail (`plan.rs:512-516`).
5. For `do = "notify"`: write the message to `report_to`'s inbox. Done.
6. For `condition`: run the predicate as a job; on exit 0 do step 3 or 5; otherwise re-arm `next_due`.
7. For `do = "ask"`: write `waiting/ask-<id>.toml` and a copy to `user/inbox/`. Do not fire again until answered.

When a turn started by step 3 ends, the kernel reads `turns/tNNNN/meta.toml` (`node = <id>`) and closes the node: recurring → `pending`; one-shot → `done` or `failed` from the verdict, unless the agent already moved it (same rule as `plan.rs:344-372`).

`at` semantics: `every = "1d", at = "09:00"` means the next due is the next 09:00 local time. `every = "1h", at = ":15"` means quarter past each hour. The local time zone is the kernel machine's, written into `project.toml` as `timezone` so a moved state fires at the intended hour. Full five-field cron syntax is not needed for Jacob's cases; add `cron = "0 9 * * 1-5"` later if wanted.

Floors stay: `every` ≥ 30 s (`node.rs:36`).

## Long-running context and delegation

The main chat may run for years. Three rules make that work: the log only grows and is indexed; the model and the kernel read a window, never the whole; context reaches a sub-agent as pointers it follows, not as text pasted into its prompt.

### 1. The main chat's log: append-only, indexed, never reloaded whole

On disk: `transcript/NNNN.jsonl` segments, `segments.toml`, `NNNN.offsets`, `journal.md`, `checkpoints/`, `turns/`. All append-only or write-once. Nothing is ever rewritten in place except `segments.toml` (atomic, tiny) and the generated views.

In the kernel, a turn holds one thing: the **visible window**. That is the latest checkpoint file plus every event after its `hi`. At turn start the kernel reads `checkpoints/INDEX.md` for the latest `hi`, seeks there with the offsets file, and reads to the end. After each step it appends the new events to that in-memory list; it does not re-read (this replaces the seven `load_transcript` calls, `turn.rs:119-498`). Cost per step is the window, which compaction keeps under `compact_at` tokens, whatever the age of the chat.

The model sees the same window: `[context checkpoint]` (`project.rs:88-93`) renders the checkpoint file's body, then the events after it. This is what compaction does today; the change is that the summary is a file the kernel reads, not a JSONL line it must scan for.

Other whole-file readers become index reads: `last_usage` reads the last line of the open segment; `needs_serve` is gone (turn folders); `last_touched_path` reads the current turn's events only; the UI tail uses `Changed` frames with byte offsets (see "Remote attach and sharing").

Compaction writes, in this order, so a crash leaves a consistent state: (1) `checkpoints/ckNNNN.md` via tmp+rename; (2) the `Compaction { lo, hi, file }` event on the transcript; (3) `INDEX.md` regenerated. If (1) lands and the kernel dies before (2), the file is an orphan with no pointer; the next compaction overwrites or supersedes it. If (2) lands, (3) is a view and regenerates at any time. Nothing in the range `lo..hi` is touched. The full record stays, greppable, forever.

Compaction of the main chat gets one more rule: the summariser prompt must carry the four fixed headings (`Decisions`, `Facts`, `Open`, `Then what happened`), and the kernel appends any new `## Decisions` bullets to root's transcript as a `Notice` suggesting they be added to `GOALS.md`. Root decides; the kernel never writes `GOALS.md` itself.

### 2. Kickoff by brief and pointers; the sub-agent pulls the rest

`spawn` gains `context`, `look_in`, `task`, and `report_as` arguments (`tools.rs:57-93`) and writes the brief file shown under "File formats". The kernel adds `goals = ".arbos/GOALS.md"` and `project` (the child's cwd) if the parent omits them. The child's instance prompt begins:

> Read, in order: `.arbos/GOALS.md`; the files in `context`; the tail of `look_in` journals. Then work. When you need history, `grep --scope history` and `read` the cited lines. Do not read whole transcripts.

The pull is cheap and precise because of the layers below, from coarse to fine. A sub-agent stops at the first layer that answers its question.

| Layer | File | Read cost | Answers |
| --- | --- | --- | --- |
| 0 | `GOALS.md` | one small file | what the project is for, what was decided |
| 1 | `agents/<id>/checkpoints/INDEX.md`, then one `ckNNNN.md` | two small files | what happened in that chat, summarised, with line cites |
| 2 | `agents/<id>/journal.md` (tail, or grep) | one line per turn | which turn did what, touched which files, when |
| 3 | `turns/tNNNN/meta.toml` + `cause.md` | two small files | why a turn ran, its outcome, its line range |
| 4 | `grep --scope history <pattern>` | index lookup | every line anywhere in `.arbos/` that mentions it, with turn ids |
| 5 | `read transcript:1181..1240` | one seek | the exact words |

The kernel resolves layer 4 hits to turn ids by a binary search over `turns/*/meta.toml` line ranges (a per-agent sorted list cached by mtime), so a grep result reads `agents/root transcript:44102 (t0311, 2026-06-18) — "use Moshi via the open API"`. One more `read` gives the full exchange.

The result goes back the same way. With `report_as = "pointer"` (the default), the child writes its result to a file — its own `pages/result.md`, or `shared/<topic>.md` when it is for everyone — and sends the parent one short `request` with the path and a three-line summary. The parent's transcript grows by one message per child, not by the child's work. With `report_as = "text"` the child says the result in full; use it for a one-line answer.

Parent and child share nothing in memory. Everything the child learned is in its own folder and in `shared/`; the parent's compaction and the index see it there. A grandchild can pull from a sibling's folder the same way. Depth and fan-out stay capped by `project.toml`.

### 3. The master goals file

`.arbos/GOALS.md`, format and ownership as given under "File formats". The rules in one place:

- The main chat (`root`) is the only agent that writes it. The kernel refuses writes from anyone else and tells them to `say to=root`.
- A person may edit it directly. Remote clients need the owner role.
- Every agent gets it in its system prefix every turn. A brief may pin `goals_version = 14` so the child notices if root changed the goals mid-task (the kernel then adds a `Notice` to the child).
- It holds goals, constraints, dated decisions, current focus, what is not being done, and where things are. It never holds progress; that is `plan.md` and `journal.md`.
- It is committed like everything else, so `git -C .arbos log -- GOALS.md` is the history of the project's intent.

### The search index

Reuse tgrep. Changes to `PlaceGrep` (`grep.rs`):

- Index dir moves to `.arbos/runtime/cache/tgrep/`. At start, open the existing index; walk for files whose mtime changed since the index was built and upsert them (tgrep has the overlay and publish path, `hybrid.rs:1-13`). Do not rebuild.
- The kernel feeds the overlay itself. After every `append_events` it upserts the open segment; after every write to a file under `.arbos/` (checkpoint, journal, plan node, shared doc) it upserts that file. Sealed segments are indexed once when sealed. Because segments are capped at 8 MB, the 64 MB skip (`walker.rs:43`) never bites, and an upsert of the open segment is bounded work.
- The `grep` tool gains `scope`: `project` (the code, default for a root agent), `history` (everything under `.arbos/` except `runtime/` and `jobs/*/out.log`), `agent:<id>` (one agent's folder), `all`. A sub-agent's default is `history` when the query is about the past and `project` otherwise; the tool description says so and the brief's `look_in` sets it.
- Hits inside a transcript segment come back with the global line, the turn id, and the date, as shown above. Hits in `checkpoints/` and `journal.md` rank first: they are the compressed layers.

Two things this does not need: a vector database, and a second index format. Trigram search over well-structured Markdown and JSONL, with turn ids as the join key, is precise enough, runs offline, and is already in the tree.

## Minimal in-memory state

The kernel keeps in memory only what is an OS object or a pure cache.

| Kept | Why | Disk mirror |
| --- | --- | --- |
| `running: HashMap<agent, TurnHandle>` — cancel token, tokio task | a live HTTP stream and a task cannot be a file | `turns/tNNNN/meta.toml` without `ended` |
| child process handles: jobs being awaited, Chrome, ptys | to `wait()` and `kill()` | `jobs/*/meta.json` pid; `runtime/pids/*.pid` |
| client sockets | inherently process | none; clients reconnect and re-read |
| file watcher handle | OS object | none |
| the visible window of each running turn: latest checkpoint + events since | the model step needs it; it is the only transcript data a turn holds | `checkpoints/`, the open segment |
| caches: parsed `agent.toml` by mtime, plan render by mtime, turn line-range list per agent, tgrep index | speed | rebuilt from files |

Gone from memory: `Clock.turns`, `Clock.mech`, `asks`, `approves`, `sent`, `tails`, `announced`, `TurnControl.steer`, `TurnControl.compact` (a compact request becomes an inbox file `kind = "wake"`, `body = "compact"`, or a `turns/tNNNN/compact` marker for a running turn). The `Wake` struct stays as a short-lived value passed from the watcher to the turn, built from the `cause.md` file.

The kernel must be able to be killed at any instant and restarted with no loss except the tokens of the model call in flight. Test: `kill -9` in a loop during the fixture suite.

## Git save and rewind

### What is a snapshot

`.arbos/` is its own git repository (`.arbos/.git`). The project's own `.gitignore` already ignores `.arbos/` (`.gitignore:3`), so the two histories never mix. Nested repo, not a submodule: the project repo does not reference it.

`.arbos/.gitignore` excludes:

- `runtime/` (lock, pids, listen address, caches, Chrome profile, focus)
- `agents/*/trace/`
- `agents/*/jobs/*/out.log` when over a size cap (the kernel rewrites the ignore list per oversize file; or, simpler, truncate a committed copy `out.head.log` at 1 MB). Open question 3.

Everything else is committed: agents, plans, inboxes, turns, transcripts, `user/`, `shared/`, hooks, skills, `project.toml`.

### When

The kernel runs `git add -A && git commit -q -m "<agent> <turn> <verdict>: <outcome first line>"` in `.arbos/` after every turn ends, and after every cron firing that does not start a turn (shell/notify nodes). One commit per turn is cheap: transcripts only grow, so the diff is an append, and a sealed segment is one blob that never changes again. A `git gc` runs weekly from a kernel-owned recurring node on `root` (visible, cancellable). The `.offsets` files are derived; commit them anyway (they are small and make a checkout usable at once) or list them in `.gitignore` and rebuild on start. Recommend commit.

A commit while another agent's turn is running captures that turn's `meta.toml` without `ended`. That is fine: it is exactly the state a crash would leave, and restart handles it.

### Rewind

`arbos-kernel rewind <commit-or-tag> [--place <dir>]`:

1. Ask the running kernel to stop (or refuse if none): every running turn gets `stop`, the kernel writes `ended` + `interrupted` on each, commits `"pre-rewind"`, and exits. Nothing is lost; the pre-rewind state is a commit you can return to.
2. `git -C .arbos checkout <commit> -- .` then `git -C .arbos commit -m "rewind to <commit>"`. History stays linear; the rewind is itself a commit. (Alternative: `git checkout -b`. Open question 2.)
3. Start the kernel. Reclaim marks any turn without `ended` as interrupted. Any file in `inbox/` fires as usual. Any `next_due` in the past fires once. The agent continues from where the snapshot left it.

Inspect without rewinding: `git -C .arbos log`, `git -C .arbos show <commit>:agents/root/plan.md`, `git -C .arbos diff <a> <b> -- agents/root/journal.md`, `git -C .arbos log -- GOALS.md`. The desktop's history view is `git log` over `.arbos`.

### The project's code

A turn changes two things: `.arbos/` and the project tree. `turns/tNNNN/meta.toml` records `repo_head` (what `.arbos/checkpoint` does today, `tools/git.rs:45-62`). A full rewind offers to `git reset --hard <repo_head>` the project too, with `clean -e .arbos` as `undo` does now (`tools/git.rs:81-99`). Default: rewind `.arbos/` only and print the project sha. Open question 5.

## Testing by authored states

A test is a folder. Write `.arbos/` by hand, run the kernel on it, read the files it left.

Kernel flags to add:

- `arbos-kernel serve <place> --until-idle`: run until every agent is idle and no node is due within `--horizon 1h`, then exit 0. For tests and for cron jobs that only need one pass.
- `--now 2026-09-13T09:00:00Z`: fixed clock. Every `now_ms()` call reads it (`crates/arbos-core/src/lib.rs:32-37` becomes a clock trait or an env override). Crons become deterministic.
- `--provider replay --replies fixtures/replies.jsonl`: a `Provider` that returns scripted assistant messages and tool calls in order, no network. The engine already isolates the provider behind one struct (`crates/arbos-engine/src/provider.rs`).
- `arbos-kernel check <place>`: lint. Parses every `agent.toml`, `plan/*.md`, `inbox/*.md`, `turns/*/meta.toml`. Reports unknown fields, bad statuses, a `parent` that does not exist, a `turn` reference with no folder. Exit 1 on any error. Also useful for a human after hand-editing.

A fixture:

```text
tests/fixtures/cron-fires-and-reports/
├── .arbos/
│   ├── project.toml                    timezone = "UTC"
│   └── agents/root/
│       ├── agent.toml
│       ├── plan/0001-btc-price.md      every = "1h", do = "shell", shell = "echo 42", notify = "BTC: {output}", next_due = 2026-09-13T09:00:00Z
│       └── transcript/                 (empty)
├── replies.jsonl                       (unused here: no model turn)
└── expect.sh                           test -f .arbos/user/inbox/*-cron-0001-*.md && grep -q "BTC: 42" .arbos/user/inbox/*.md
```

Run: copy the fixture to a temp dir, `arbos-kernel serve $tmp --now 2026-09-13T09:00:01Z --until-idle --provider replay --replies replies.jsonl`, then `expect.sh`. Each fixture is a folder; `cargo test` walks them. Because states are git commits, a real session's `.arbos/` at any commit is also a fixture: `git -C .arbos archive <commit>` into `tests/fixtures/…`.

Crash tests: the same runner with `--kill-after 3s` in a loop until idle. Assert the final state is identical to the uninterrupted run, minus `turns/` count.

## Remote attach and sharing

Where the state is: the `.arbos/` folder lives on the machine that runs the kernel. That machine is the agent's home. Every other program — the desktop on a laptop, the phone app, another person's desktop — is a **client**: a view over the network onto that folder, with no copy that is the truth.

Attach point: keep the existing one and grow it. Today the kernel listens on loopback TCP and writes the port to `kernel.json` (`serve.rs:52-56`, `serve.rs:464-470`); the desktop tunnels to it over `ssh -N -L` (`desktop/src/kernel.rs:1-7`). Change: the kernel listens on a Unix socket (`runtime/attach.sock`) and never on a public port itself. The frames stay newline-delimited JSON, carried over a WebSocket when they leave the machine. `runtime/kernel.toml` (was `kernel.json`) lists the addresses. One small process per machine, `arbos-hub`, listens on loopback (`127.0.0.1:7000`), routes `wss://<host>/p/<project-id>` to that project's `attach.sock`, and starts `arbos-kernel serve` for a known place on first attach (the job `desktop/src/kernel.rs::attach_or_spawn` does today, moved server-side). Clients need one address, not one tunnel per project.

**The concrete path for Jacob.** The kernels and the hub run on ArbosLife. A [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) (`cloudflared`, a daemon that opens an outbound connection to Cloudflare, so the server has no open inbound port and needs no public IP or certificate) maps `arbos.<jacob's domain>` → `http://127.0.0.1:7000`. Tunnels carry WebSockets. [Cloudflare Access](https://developers.cloudflare.com/cloudflare-one/policies/access/) sits in front of that hostname: a policy that says which emails (Jacob's, later Alice's) may reach it, with login by Google, GitHub, or a one-time PIN by email. After login, Access forwards every request with a signed header, `Cf-Access-Jwt-Assertion`, that carries the person's email. The hub verifies the signature against the team's public keys (`https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`) and the expected audience, and passes the email to the kernel as the client's identity. Nothing in Arbos stores a password; Cloudflare does the login, the hub does the check, the kernel does the role lookup. Credentials for the Cloudflare account and ArbosLife are in the 1Password vault (`project-context.md`, "Resources"). DNS, tunnel, and policy are three items in one dashboard, or one `cloudflared tunnel` config file plus one Access application, both of which belong in the repo as an example.

Desktop and phone attach the same way: open `wss://arbos.<domain>/p/<id>` with an Access session. The desktop obtains the session by opening the Access login page in the system browser once (`cloudflared access login` does the same thing on the command line and prints a token); the phone does it with an in-app web login sheet the first time, then keeps the token in the keychain. Headless clients (a QA loop, a script) use an [Access service token](https://developers.cloudflare.com/cloudflare-one/identity/service-tokens/): two headers, `CF-Access-Client-Id` and `CF-Access-Client-Secret`, minted per client in the dashboard and revocable there. The ssh path in the desktop stays as the no-Cloudflare fallback; it needs nothing but a key.

**Open-source friendly.** The hub does not depend on Cloudflare. It has a `trust` setting with three values: `cloudflare-access` (verify the JWT header, as above), `token` (the bearer tokens in `runtime/access.toml`, for people who put the hub behind their own TLS: Caddy, Tailscale, an ssh tunnel), and `loopback` (no auth; local only). Anyone can clone the repo, run `arbos-hub` on their own box, and pick one. The example `cloudflared` config and Access policy live in `deploy/cloudflare/` as one way, not the way. Cloudflare's free tier covers a personal setup, which is why it is the default recommendation, not because the design needs it.

**Cloud-run agents, later and optional.** Because a place is only a folder plus a kernel plus an attach socket, a kernel on a rented VM registers with the same hub and is reached by the same URL. A sub-agent that needs a GPU can then be spawned onto that VM with its `.arbos/agents/<id>/` folder synced back (git push of the nested repo) when it finishes. This is a second hub route, not a new design. Not in the first seven phases.

Reading state remotely — three options, and the choice:

- **Sync** (rsync, mutagen, or `git fetch` of `.arbos`): full copy on the client, works offline, but lags and doubles storage on a phone. Keep it for one thing only: `git fetch` of the `.arbos` repo for offline history and rewind inspection.
- **RPC** (client asks for one file): `Frame::Read { path }`, `Frame::List { path }`, `Frame::Tail { path, from_byte }`. Simple, no local copy, and the kernel enforces that `path` is under `.arbos/` and readable by the caller.
- **Stream** (kernel pushes changes): the kernel already has a file watcher for its own scheduler; the same watcher emits `Frame::Changed { path, kind, size }` to attached clients. A client tails the open transcript segment by `Tail` from the byte it has, on each `Changed`; the `.offsets` file lets it ask for a line range instead of a byte range. This replaces the 200 ms full re-read of every transcript (`serve.rs:276-288`) with change-driven, byte-offset tails.

Recommendation: **RPC + stream**, over the attach socket. The client keeps a small local mirror of only the files it displays (`GOALS.md`, `agents.md`, the focused agent's `status.toml`, `plan.md`, `journal.md`, the open transcript segment's tail, `user/inbox/`). A phone never downloads a sealed segment; it asks for line ranges. The mirror is a cache; the desktop's model code reads files from it exactly as it would read a local `.arbos/`, so local and remote are one code path. The ssh `awk` roster script (`desktop/src/kernel.rs:708-755`) and the desktop's own `.arbos/desktop/sessions/` records go away.

Writing from a client: clients never write files over the network. They send an intent, the kernel does the write on the host with the same atomic rules as everyone else:

- `Frame::Deliver { agent, message }` → the kernel writes `agents/<agent>/inbox/<time>-<from>-<seq>.md`. `from` is set by the kernel from the client's identity, never trusted from the frame. Two people sending at once produce two files; nothing is shared or overwritten.
- `Frame::Put { path, base_hash, content }` for editing a plan node, a `shared/` file, or `GOALS.md` (owner role only): compare-and-swap. The kernel takes the plan lock, checks the current file's hash equals `base_hash`, and writes; otherwise it returns `Conflict { current }` and the client merges and retries. No lost updates between two editors.
- `Frame::Stop`, `Pause`, `SetModel`, `Answer`, `Approve` stay as they are; each maps to one file write on the host.

Auth and sharing: two layers, each simple. **Who you are** is Cloudflare Access's job (or the token file, in `token` mode): it gives the hub a verified email. **What you may do** is the kernel's job: `runtime/access.toml` (not committed) maps identities to roles:

```toml
[[person]]
email = "jacob@…"                # from the Access JWT
role = "owner"                   # owner: everything, including GOALS.md and access.toml
[[person]]
email = "alice@…"
role = "writer"                  # deliver, put, answer, approve, stop
[[client]]
name = "qa-loop"                 # a service token; matched by CF-Access-Client-Id
id = "…"
role = "viewer"                  # read, list, tail only
```

The first frame on a connection is `Hello { client }`; the hub has already attached the verified identity, and the kernel answers `Welcome { role, agents }` or closes. Sharing an agent with Alice is two edits: her email in the Access policy (Cloudflare dashboard or API) and one `[[person]]` block here. Revoking is removing either. Every message she sends is an inbox file with `from = "user:alice@…"`, so the transcript shows who said what and root can answer her by name. Over ssh or `loopback`, the OS identity is the auth and this file is skipped for `runtime/attach.sock`. The kernel never holds a TLS key: Cloudflare terminates TLS at its edge and the tunnel is encrypted end to end.

Where this changes the design above: (1) every inbox front matter gets `from = "user:<email or client name>"` so the transcript shows who spoke; (2) `runtime/` gains `access.toml`, `attach.sock`, and `kernel.toml` with a list of addresses; (3) `project.toml` gains `[listen] trust = "cloudflare-access" | "token" | "loopback"` plus the Access team domain and audience for the JWT check; (4) frames gain `Read`, `List`, `Tail`, `Changed`, `Deliver`, `Put`, `Hello`, `Welcome`, and `Put` carries a hash for compare-and-swap; (5) `focus` becomes per-client state, kept by the client, not in the place; (6) a new small binary, `arbos-hub`, and an example `deploy/cloudflare/` folder in the repo.

## Migration phases

Each phase ships alone and leaves the system working.

**Phase 1 — runtime split and nested git.** Move `lock`, `kernel.json`, `checkpoint`, `focus`, the tgrep cache, and the Chrome profile under `.arbos/runtime/`. `git init .arbos` at bootstrap with a `.gitignore` of `runtime/`. Commit after every turn. Ship `arbos-kernel rewind`. Nothing about plans or wakes changes yet, but git save and rewind work from this day. Touches: `place.rs`, `serve.rs:464-470`, `tools/git.rs`, `grep.rs:106-116`, `desktop/src/kernel.rs` (read `runtime/kernel.toml`).

**Phase 2 — inbox files.** Add `inbox/` and the watcher. Route user prompts (`serve.rs:308-326`), `say` (`hooks.rs:767-832`), spawn briefs (`hooks.rs:715-719`), Telegram (`doors.rs:191`), job-done notices (`serve.rs:270-274`), and steer into inbox files. The turn loop consumes `inbox/` at start and at tool boundaries. Delete `is_inbox`, the `origin` prefixes, `hops` on nodes, `sent`, and `TurnControl.steer`. Plan nodes are now only goals. `plan.jsonl` still exists.

**Phase 3 — turn folders.** Add `turns/tNNNN/meta.toml` and `status.toml`. Delete `Clock`, `TurnMeta`, `attempts.jsonl`, `needs_serve`, and `reclaim`'s transcript guessing. Shell attempts become jobs with a `node` field. `user.md` becomes `user/inbox/`.

**Phase 4 — notes.md + subscriptions (revised 2026-09-13, Cursor's agent model; shipped as PR #99).** There are no plan files and no plan node. The `plan` tool is a checklist that writes `agents/<id>/notes.md` (checkbox items `- [ ] [label](target) — status readout` under `##` sections; checked items sink, three kept per section). Every timed or event-driven wake is a file in `agents/<id>/subscriptions/NNNN-slug.toml` (`kind = timer | shell | github_pr | github_ci | inbox`, `every`/`at`/`once`, `cmd`, `repo`/`pr`, `path`, `deliver_to = agent | user`, `notify`, `expires`, kernel-written `next_due`/`last_fired`/`last`/`seen`), fired by one watcher (`subs.rs`) that writes one inbox file (or one user line for `deliver_to = user`). A one-time migrator at kernel start turns `plan.jsonl` into notes lines, subscriptions, and inbox files, and `.arbos/subscriptions.json` into `github_pr` files (old files renamed `*.migrated`). Deleted: `Node`, `When`, `Do`, `Attempt`, `NodeStatus`, `Clock`, `plan.jsonl`, `attempts.jsonl`, the GitHub door's second `Say` write. `at` is UTC until `timezone` lands.

**Phase 5 — long-running context.** Four steps, each shippable: (a) `GOALS.md` — create at bootstrap, inject into every prompt, enforce root-only writes; (b) `checkpoints/` — compaction writes the file and a pointer event, `INDEX.md` generated, the projection reads the file; `journal.md` and `files` in turn meta; (c) transcript segments with `segments.toml` and `.offsets`, a one-time migrator that splits `transcript.jsonl`, and the visible-window rewrite of `turn.rs` so no step reloads the log; (d) tgrep persistent in `runtime/cache/`, fed by the kernel's own writes, `grep --scope`, hits joined to turn ids; `spawn` gains `context`, `look_in`, `report_as` and writes the brief file. Touches: `prompt.rs`, `project.rs`, `compact.rs`, `summarise.rs`, `turn.rs`, `files.rs`, `grep.rs`, `tools.rs:57-93`, `hooks.rs:670-721`.

**Phase 6 — waiting files (revised 2026-09-13).** `ask` parks: the turn ends with the question on the transcript and `waiting/ask-*.toml`; the answer is an inbox file (`kind = "answer"`) that starts a new turn. `approve` blocks with a disk mirror (`waiting/approve-*.toml`, `user/inbox/` copy). Delete `asks` and `approves`. `--until-idle`, `--now`, `--provider replay`, and the fixture runner shipped (#77, #87, #90); `contracts.rs` node cases are gone with the node.

**Phase 7 — attach over files.** Add `Read`, `List`, `Tail`, `Changed`, `Deliver`, `Put`, `Hello`. Replace the 200 ms transcript re-read with watcher-driven tails. Desktop reads its mirror instead of `.arbos/desktop/sessions/` and drops the Go gateway paths. Then `arbos-hub` with `trust = "cloudflare-access"`, `access.toml`, `cloudflared` on ArbosLife with an Access policy, and the phone attaching through it. Cloud-run kernels register with the same hub later, if wanted.

## Open questions

1. **Plan node format: one Markdown file per node (recommended), one `plan.md` with a strict checklist grammar, or keep JSONL.** One file per node gives atomic writes, readable diffs, and room for prose. A single `plan.md` is nicer to read but fragile to parse and a hot spot for write races. JSONL is the status quo with its fold rule. Recommend one file per node; keep `plan.md` as a generated view.

2. **Does the main chat execute, or only delegate?** Cursor's coordinator never edits code. If root's allowlist is narrowed to `read`, `grep`, `find`, `spawn`, `say`, `ask`, `plan`, its transcript stays small and its context lasts longer; every edit happens in a sub-agent whose folder holds the detail. The cost is one spawn for even a one-line fix. Recommend a `[root] role = "coordinator"` setting in `project.toml`, on by default for new projects, off for existing ones. Rewind style is decided inline: a forward commit, with `--branch` as an option.

3. **What to commit from `jobs/`.** Logs can be large. Options: commit everything; ignore `out.log` over 1 MB; commit a 1 MB head copy. Recommend the head copy: rewind still shows what a job printed, and the repo stays small. `images/` commit as-is with a 5 MB per-file cap.

4. **Crons: one mechanism (recommended) or a separate `crons.toml`.** One mechanism means one scheduler, one render, one status model, and a cron is cancellable like any goal. A separate file is easier to spot at a glance. Recommend one mechanism, plus a "standing" section in `plan.md` and `agents.md` so crons are visible without opening node files.

5. **Rewind scope.** `.arbos/` only (recommended default), or `.arbos/` plus `git reset --hard` of the project to the turn's `repo_head`. The project may have human commits since; a hard reset can lose them. Recommend default to `.arbos/` only, print the sha, and offer `--with-project` which refuses if the project has uncommitted changes.

6. **Agents edit plan files directly (recommended) or only via the `plan` tool.** Direct edits make the plan fully file-first and let humans edit in the same way. The risk is a malformed file or a race with a kernel status flip. Recommend allow it, with `check` linting, kernel-owned fields listed in `PROTOCOL.md`, and the loser's version always in git.

7. **Transcript in sealed segments (recommended) or one file forever.** Segments keep the index, git, and every read bounded, and a sealed segment is a natural archive unit. The cost: a cite is a global line the kernel must map to a segment, so `grep -n` on a raw segment needs `first_line` added, and `cite_path` (`project.rs:85`) changes. One file is simpler to point at but crosses tgrep's 64 MB skip and makes every whole-file reader slower every day. Recommend segments at 8 MB, with `arbos-kernel cat` for humans. (Local `say` is decided inline: write the peer's inbox directly when the folder is local; remote clients use `Deliver`.)

8. **Ask blocks or parks.** Park (recommended): the turn ends with the node `pending`, `do = "ask"`, a `waiting/` file, and the answer wakes a new turn. Block: the turn stays open in memory, mirrored on disk, so the model keeps its exact context. Parking is fully restart-safe and costs one re-prompt; blocking keeps context but a kernel restart interrupts it. Recommend park for `ask`; block-with-mirror for `approve`, because an approval is seconds and mid-tool.
