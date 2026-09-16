> **REWRITTEN 2026-09-16 after the `docs/` loss.** The original (24,912 bytes, written 2026-09-13 20:36 UTC by the audit worker `bc-938c8002-ea3d-53bf-9cf2-9dcfc9cf48ec`) had no recoverable copy. This is the same worker's rewrite. Sources, in order of weight: (1) the surviving evidence the original was built on, all still in the store — the stills and transcripts in `media/audit-multitasking/` and the 26 QA scenarios in `internal/qa/inbox/2026-09-13-multitasking-audit.md`; (2) the four PRs that shipped the fixes and quote the findings and fix numbers back: [#122](https://github.com/unarbos/arbos/pull/122), [#128](https://github.com/unarbos/arbos/pull/128), [#129](https://github.com/unarbos/arbos/pull/129), [#135](https://github.com/unarbos/arbos/pull/135); (3) the worker's own session record of the text it wrote. Every measurement and transcript cite below was re-checked against (1) on 2026-09-16; every `file:line` cite is against the audited commits and was not re-checked against `main`, which has moved. Paragraphs that could not be re-verified from (1) or (2) are marked **[reconstructed]**. The appendix "Where the fixes stand" is new and is built from (2) only.
>
> The loss, its cause and the recovery inventory: `internal/store-docs-loss-2026-09-16.md`. This file was written to `/tmp` first and copied in.

# Multitasking audit: did we build the motto right? (2026-09-13)

Audit of `unarbos/arbos` kernel at `844e835` (integration head was `2338c69`; the crates are identical to `844e835` on the desktop stack) and the desktop stack #71 → #101 → #105 (`cursor/project-panel-94d6` at `d4981aa`). Both built from source and run under Xvfb with the driver. Every claim below has a still in `media/audit-multitasking/`, a transcript in `media/audit-multitasking/transcripts/`, or a `file:line` cite. Model: `openai/gpt-5.4-mini` over OpenRouter.

Terms used once: **root** = the project's main chat agent. **Turn** = one run of the model loop, from a wake to `turn_complete`. **Steer** = words injected into a turn that is already running, read at its next tool step. **Queue** = words delivered as the next turn. **Done file** = the inbox file the kernel writes to a parent when a child's turn ends. **Plan strip** = the block pinned above the composer that at audit time showed subscriptions, open notes items, and queued messages.

Transcript cites use the first column of `transcripts/root-transcript-compact.txt` (the 0-based event index), not the file's line number.

## Verdict (10 lines)

1. The kernel is built right. Steer, queue, coordinator role, six-field brief, done files, parked asks, heartbeat: all exist as files on disk and all work when driven directly.
2. The desktop is not. Typing during a running turn never reaches the kernel; it sits in the window's memory until the turn ends (still `02b`). Cursor steers by default; Arbos queues by default and offers "Interrupt now" as the only fast path.
3. That memory queue is lost when the window quits (restart test: the follow-up never arrived).
4. A typed thought while a question is parked is thrown away and the question is marked "skipped" (transcript index 119–123).
5. Delegation by default works: a three-part request became two parallel workers plus one inline answer in 7.7 s. Root then ignored the project page for eight turns until told to write it.
6. Push-back is noisy: two children produced four root turns and four user messages; the haiku was read to the user three times. Cause: children both `say` and get a done file, `wait=true` gets the result twice, and the kernel opens one root turn per done file with no batching.
7. The child cap counts finished workers. After eight workers ever, root cannot spawn again ("at the 8-child limit", five times in one session). This alone breaks "everything is delegated".
8. The plan strip is the wrong home: with a real project page (21 open items) it fills the whole chat column and pushes the transcript off screen (still `04b`). The right panel already renders the same page well (still `08`).
9. Voice does it right: the gateway sets `steer = agent is running` on every utterance (`voice_server/kernel.py:381-383` on #100).
10. Priority: fix the desktop send path (steer by default, kernel-held queue), stop the double report and batch done files, count only live children, and move plan items out of the composer. Four kernel changes and two desktop changes get us to the motto.

## 1. Responsiveness: typing while root runs

**What we built.** Kernel: `Frame::User { steer: true }` for a running agent writes an inbox file of kind `steer` (`crates/arbos-kernel/src/serve.rs:645-658`); the turn loop takes every `steer`/`wake` file at each tool boundary and appends them as `user` events (`crates/arbos-engine/src/turn.rs:356-385`). Measured: steer sent at +4.2 s landed at +5.01 s, the first tool boundary; a `steer: false` message sent at +5.2 s waited until the turn ended at +42.7 s (`root-transcript-compact.txt` index 4 and 12–13; frame timings in `scenario-a-frames.jsonl`). The heartbeat exists: a `Working {secs}` frame every 5 s of model silence, shown as "Thinking for Ns" (`crates/arbos-engine/src/provider.rs:423`, `wire.rs:123`).

Desktop: `ChatSession::send` pushes into a `VecDeque` in the window when `streaming` is true (`desktop/src/model/session.rs:1193-1206`); `drain` sends it after the turn ends (`:1209-1216`). The only frame the desktop ever sends carries `steer: false` (`desktop/src/agent/acp.rs:314`). "Force" = stop the turn, then send the queue (`session.rs:1222-1242`), labelled "Interrupt now: stop the running turn and send this" (`composer.rs:1942`). Measured (still `02b`): typed at +8.6 s, kernel inbox empty, kernel transcript unchanged, landed at +61.0 s after `turn_complete`. The composer shows the queued line above the field with "Interrupt now · Edit · ✕" and the disc becomes Stop plus Force.

Aggressive side effect in the kernel: when a steer file is present before a tool batch starts, every not-yet-started tool call is skipped with "skipped: user steered" (`crates/arbos-engine/src/batch.rs:283-289`, "pi's rule"). In scenario A the user's own requested `spawn` was skipped, root then tried to `say` to a worker that did not exist, and the work started 30 s late (transcript index 3, 6, 9).

**Voice.** The gateway's `send_user` sets `steer` to "the agent is running now" when not told otherwise (`voice-server/voice_server/kernel.py:376-389` on #100); `narrator.user_said` sends every utterance that way (`narrator.py:330-345`). A typed line during a call goes the same route with `channel = "text"`.

**What Cursor does.** Typing during a running turn steers at the next tool boundary; queueing is the explicit choice. Tool calls already decided are not cancelled.

**Gap.** Desktop never steers; the queue is client-side memory; the kernel's steer cancels pending tool calls.

**Fix.**
- Desktop sends `steer: true` when the chat is busy; the composer's default action while running is "Send" (steer), with "Queue" as the alternate and "Stop" separate. S, desktop.
- The desktop's queue becomes kernel inbox files (`wake = true`, kind `message`): the kernel already draws them back as `inbox: true` rows (`hooks.rs:331-357`), so the window keeps its row UI and loses the memory. S, desktop.
- Batch rule: a steer that arrives before a batch starts is appended after the batch, not used to skip it; skip only calls that have not started when the steer arrived mid-batch, and say so. S, kernel.

## 2. Delegation by default

**What we built.** `project.toml [root] role = "coordinator"` is written only for a new place (`crates/arbos-core/src/project.rs:85-99`); an existing place keeps every tool until someone adds the line. The role narrows root to `COORDINATOR_TOOLS` in memory each turn (`project.rs:110-118`) and prepends `COORDINATOR_CONTRACT` (5.4 k chars, ~1.35 k tokens) to the instance prompt (`crates/arbos-engine/src/prompt.rs:36-46`, `:91-95`). `spawn` takes the six fields and renders them (`crates/arbos-kernel/src/tools.rs:63-105`, `crates/arbos-core/src/store.rs:241-290`); `read_first` defaults to `project-context.md, then notes.md`.

Measured on a fresh place (project.toml confirmed `role = "coordinator"`): the three-part request produced two `spawn` calls in one response plus an inline `read README.md`, and root's turn ended at +7.7 s (transcript index 22–30). Both briefs had all six fields and `read_first = .arbos/docs/project-context.md`; both children read that file first and kept their own `notes.md` checklist (`transcripts/child-slow-two.txt`). Weak spots: `rules` was the parameter description copied verbatim ("repo and base branch, no merging…", no real branch); `isolate=worktree` was set for a haiku that writes only to `.arbos/docs/`; `output` invented `.arbos/docs/slow-two.md` for a task with no deliverable.

Notes: root did not touch `.arbos/notes.md` in eight turns across three scenarios (still `05`: "Nothing on the project page yet"). Told explicitly, it wrote a correct page in one call (still `08`), so the tool works and the habit does not. Nothing in the kernel checks the page at turn end; `arbos-kernel check` lints shape, not freshness.

Cap: `live_children` counts every child folder, finished or not (`crates/arbos-kernel/src/hooks.rs:313-319`); `MAX_CHILDREN = 8` (`sched.rs:11`). Scenario C: four finished workers plus four new ones hit the cap; the fifth spawn was refused five times ("at the 8-child limit", transcript index 78–110, and once more at 128; the `scenario_c.py` driver reproduces it). Root cannot reap a finished child itself.

**What Cursor does.** The coordinator never edits code, writes `notes.md` every turn, one worker per workstream, no cap that counts finished workers.

**Gap.** Behaviour is prompt-only; the cap blocks a coordinator after eight workers; briefs carry placeholder rules and needless worktrees.

**Fix.**
- Count only children with a running turn or an open `waiting/` file as live; archive finished child folders under `.arbos/archive/` after their done file is consumed (the desktop panel keeps listing them from the archive). S, kernel.
- Turn-end notes nudge: when root's turn ends after a `spawn`, a `done`, or a `subscription` event and `notes.md` mtime is unchanged, append a kernel notice "project page not updated this turn" and pass it as the first line of the next wake. S, kernel.
- Kickoff defaults: `rules` default fills the real base branch from `git`; `isolate` defaults to `worktree` only when `output` or `do` mentions a path outside `.arbos/`. S, kernel.
- Existing places: `arbos-kernel migrate` offers the coordinator role once, with the one-spawn-per-fix cost stated. S, kernel. **[reconstructed]** — this bullet is from the author's record only; no PR picked it up.

## 3. Push-back: how results come back

**What we built.** At every child turn end the kernel writes a done file to the parent, `wake = true`, body "Turn ended. Last words: … (transcript: …)" (`crates/arbos-kernel/src/plan.rs:244-277`). `scan` claims one waking file per idle agent per pass (`plan.rs:33-79`), so each done file is one root turn, one model call. The brief template still tells the child "Report results to your parent with say to={parent}" (`plan.rs:178-189`), so a child reports twice: its own `say` and the kernel's done. `spawn wait=true` marks the child in `waited` only when the wait resolves at turn end without a report (`hooks.rs:276-279`); when the child's `say` resolves the wait first (the common case) the done file still follows.

Measured: scenario B, two children → root turns at +171 s (say), +177 s (done), +191 s (done); the haiku text was sent to the user three times and README twice (transcript index 31–58). Scenario A and the restart test: `wait=true` gave the answer as the tool result, then the done file opened another turn that repeated the same one-line answer (index 7–11 and 16–21; `home-root-restart-test.txt` index 2–10). Scenario C: four children ending within 16 s → four root turns, four identical "still blocked" messages to the user (transcript index 86–110).

User-facing surfaces: the done file lands in the main chat as a raw `say` row from the child, plumbing text included ("Turn ended. Last words: … (transcript: .arbos/agents/…/transcript.jsonl)", still `01`); the panel shows children with a check when idle (still `08`); the narrator has a `child-done-while-talking` scenario (#100 tests). No inline card says "worker X finished: one line" without a root model turn.

**What Cursor does.** One completion notice per worker turn, system-written; the coordinator folds it into notes and speaks only when the user asked for that result.

**Gap.** Two reports per child, one root turn per done file, duplicated answers, kernel plumbing shown to the user, root repeats itself.

**Fix.**
- Drop "Report results with say" from the brief template; the done file is the report. Mark `waited` when a `say` resolves a wait too. S, kernel.
- Batch: when root is idle and several `done`/`subscription` files wait, claim them all into one turn as one message ("3 workers finished: …"). M, kernel.
- Render a done file in the chat as a compact worker card (name, verdict, first line, link), not the raw body; hide the "(transcript: …)" pointer. S, desktop.
- Contract line: "after a done, if the user already has this result, update notes.md and end without a message"; the notes nudge above enforces the first half. S, kernel.

## 4. Plan representation

**What we built.** `wire_rows` builds one list for the plan strip: every subscription (`standing: true`), every unchecked item of the agent's `notes.md`, and every queued inbox file (`crates/arbos-kernel/src/plan.rs:325-370`, `hooks.rs:331-357`). For root, `notes.md` is the project page (`crates/arbos-core/src/notes.rs:59-74`), so the whole project page is pinned above the composer. The desktop draws it in `detail.rs::plan` (`desktop/src/view/detail.rs:1722-1953`) with per-row run/cancel controls left over from the plan engine, then permission, questions, and the local queue (`detail.rs:639-646`). The kernel's own weekly `git gc` chore is a subscription, so a brand-new empty chat opens with "Plan · 1 standing" (still `00`).

Measured: a realistic page (this Project's `notes.md` of 2026-09-13, 21 open items) made the strip "Plan · 21 steps · 1 standing" and it took the entire chat column; raw `[label](url)` markdown leaked into rows; each row had ▷ and ✕ that mean nothing for a notes item (stills `04`, `04b`). The panel's Project section rendered the same page correctly at the same moment, and the full Project page (⌘2) exists (`view/project_page.rs`, still `05`). The panel also has its own Standing section (`view/panel.rs:260-345`), so subscriptions show twice. A parked ask is a card above the composer with options, Skip, Continue (still `06`); it is correct but it too is pinned, and the plan strip stays above it.

**What Cursor does.** Plans and status are documents on the Project page and in the panel. Questions and approvals are cards inline in the transcript at the point they were asked. Queued follow-ups are one line under the composer with "Send now". Nothing else is pinned.

**Gap.** Status pinned to the chat bar; duplicates the panel; plan-engine controls survive; kernel chores leak into the user's list.

**Fix.**
- Remove notes items and subscriptions from the plan strip; keep the strip for queued follow-ups only, one line each. S, desktop.
- Kernel chores (`gc`) are `internal = true` and hidden from every user list. S, kernel.
- Ask and approve become inline transcript cards at their turn, with the answer written as a user bubble; the composer hint "Add more optional details" stays while a card is open. M, desktop.
- Done cards inline (from check 3). The Project panel is the single home for status and standing work; the Standing section shows the `subscriptions/` folder with pause/remove. S, desktop.

## 5. Context to sub-agents

**What we built.** Child kickoff = template line (~180 chars) + six fields; measured 793 chars for a one-line task (`child-slow-two.txt`). Every agent's prompt = `CONTRACT` (6.75 k chars) + `docs/project-context.md` when it is not the template (capped at 16 k chars, `store.rs:24`, `:205-222`) + instance prompt + its own `notes.md`; root adds `COORDINATOR_CONTRACT` (5.4 k). So `project-context.md` is injected every turn for every agent (`crates/arbos-engine/src/project.rs:251-257`) and the child also reads it explicitly because the brief says so (double delivery, one extra tool call). `GOALS.md` is a symlink alias (`store.rs:99-102`). `notes.md` is not injected into children; they get it only if `read_first` names it (the default text does, the model dropped it in three of four briefs).

History: there is no `grep --scope history` flag. The `grep` tool hides `.arbos/` hits unless `path` starts with `.arbos` (`crates/arbos-engine/src/tools/fs.rs:164-174`); the tgrep index includes hidden files (`crates/arbos-kernel/src/grep.rs:33-36`), so `grep path=.arbos/agents pattern=…` searches every transcript. The CONTRACT says "Prior work = transcript.jsonl; grep it" but the coordinator contract says never read a worker's transcript; children have no line telling them the history is greppable.

**What Cursor does.** Workers read `project-context.md` first by convention; the coordinator passes paths, never content; workers pull what they need from the store.

**Gap.** Good shape; missing a one-line pointer for children to `.arbos/` history, the brief re-reads an injected file, and `rules` is placeholder text.

**Fix.**
- Brief template: replace "Read first: project-context.md" (already in the prompt) with "Project context is in your prompt; status is `.arbos/notes.md`; earlier work is greppable with `grep path=.arbos/agents`". S, kernel.
- Add `scope: history` as sugar on `grep` that sets `path=.arbos/agents` and formats hits as `agent/turn/line`. S, kernel.

## 6. Failure modes

- **Long thinking, no heartbeat.** Covered: `Working` frames every 5 s (`provider.rs:423`, `:577-595`), "Thinking for Ns" row in the desktop (QA note `internal/qa/inbox/2026-09-13-thinking-heartbeat.md`). Not covered: the narrator has no spoken heartbeat line for a silent minute.
- **Ask parking clarity.** The park works (turn ends, `waiting/ask-*.toml`, "Waiting for your answer" notice, card; still `06`). Failure: `submit` routes any typed text to `answer_ask` while a question stands (`detail.rs:300-310`); with no option selected `answer_ask` treats it as a skip and drops the text (`session.rs:1602-1631`). Measured: "Unrelated thought: also spawn a worker…" → transcript shows `answer ""` and "The user skipped this question"; the words appear nowhere (transcript index 119–123, still `07`). Fix: text without an option is the answer's free text, or a normal message when it does not read as an answer; never a skip. S, desktop.
- **Done storms.** One root turn per done file, no batching (check 3). With eight children finishing together that is eight model calls and up to eight user messages. Fix: batch (check 3). M, kernel.
- **Reconnect.** The window's queue is memory: a follow-up typed while running, then quit and relaunch, is gone (`home-root-restart-test.txt` has no `RESTART-TEST` line; `session.rs:639-641` starts a loaded session with an empty queue). Relaunch also landed on tab 0 instead of the tab that was active. When a child folder was removed under a running window, the kernel log showed `attach_open`/`attach_close` about twice a second (count 8781 → 9375 in ~5 min) and a ghost `transcript.jsonl` reappeared in the deleted folder; the audit read this as the window retrying a missing agent. **Corrected by #129 (2026-09-13):** the storm was the window probing the attach port on every poll (`kernel::http_base` and `boardhub` retrying a `tcp://` address every 800 ms), present with nothing happening at all; deleting a child folder did not change the rate. #129 fixed the probing and, separately, made a `no agent …` refusal close the row. The ghost transcript observation stands **[reconstructed — not re-measured]**. Fix as filed: kernel-held queue (check 1); on "no agent" the window closes that session instead of retrying. S, desktop.
- **Steer cancels work.** A steer present before a batch skips every call in it (check 1). S, kernel.
- **Cap counts the dead.** Check 2. S, kernel.

## Prioritized fix list

| # | Fix | Size | Lane |
| --- | --- | --- | --- |
| 1 | Desktop sends `steer: true` while busy; Send steers, Queue is the alternate, Stop is separate | S | desktop |
| 2 | Desktop queue becomes kernel inbox files; window draws the `inbox: true` rows it already gets | S | desktop |
| 3 | Typed text while an ask is parked is never a skip: free-text answer, or a normal message | S | desktop |
| 4 | `live_children` counts running or waiting children only; finished folders archive after their done file is consumed | S | kernel |
| 5 | Brief template drops "report with say"; `waited` also set when a `say` resolves the wait | S | kernel |
| 6 | Batch pending `done`/subscription files into one root turn when root is idle | M | kernel |
| 7 | Plan strip shows queued follow-ups only; notes items and subscriptions leave it; kernel chores hidden | S | desktop + kernel |
| 8 | Steer arriving before a batch is appended after it, not used to skip it | S | kernel |
| 9 | Done file rendered as a compact worker card, raw body and transcript pointer hidden | S | desktop |
| 10 | Turn-end notes nudge when root spawned or received a done and `notes.md` did not change | S | kernel |
| 11 | Ask and approve as inline transcript cards; answer becomes a user bubble | M | desktop |
| 12 | Window closes a session on "no agent" instead of reconnecting; relaunch restores the active tab | S | desktop |

Gateway lane: nothing blocking. One nice-to-have: a spoken heartbeat after 20 s of model silence during a call.

The numbering is the one the PRs cite (#122 "fixes 4, 5, 6, 8"; #128 "fixes 7 kernel half, 10, §5"; #129 "fixes 1, 2, 3, 7-desktop, 12"; #135 "fixes 9 and 11"), so the table above is verified against the repository, not only the author's record.

## Plan-strip redesign sketch

At audit time (still `04b`): plan strip with 21 notes rows + standing + queue + ask, all above the composer.

Proposed. Nothing pinned to the composer except the in-call strip and the queued-follow-up line.

```
┌─ chat column ───────────────────────────────┐ ┌─ Project panel ─────────────┐
│  you: Three things. (1) … (2) … (3) …        │ │ Agents                      │
│                                              │ │  ● Add subtract and test    │
│  ┌ worker · Write mountain haiku ── done ┐   │ │  ✓ Write mountain haiku     │
│  │ "Silent peaks at dawn…"  → docs/…md   │   │ │                             │
│  └───────────────────────────────────────┘   │ │ Project              ⌘2  › │
│                                              │ │  <tldr> … 4 bullets         │
│  ┌ question ─────────────────────────────┐   │ │  ## Poems                   │
│  │ Which colour do I prefer?             │   │ │   ○ River poem — verified   │
│  │  (A) red  (B) blue  (C) other…        │   │ │  ## Math subtract           │
│  │  [Skip]                    [Continue] │   │ │   ○ Subtract test — pending │
│  └───────────────────────────────────────┘   │ │                             │
│  you: blue                                   │ │ Standing                    │
│                                              │ │  ↻ CI on #94 · next 21:00   │
│  root: README says "# audit-place".          │ │  ↻ timer · every 1h  ⏸ ✕    │
│                                              │ │                             │
│  ┌ approval ─────────────────────────────┐   │ │ Files                       │
│  │ allow bash: git push origin …         │   │ │  Context · haiku.md · …     │
│  │  [Deny]                       [Allow] │   │ └─────────────────────────────┘
│  └───────────────────────────────────────┘   │
│                                              │
│  ▸ 1 follow-up queued · Send now · Edit · ✕  │   ← only when something is queued
│  ┌──────────────────────────────────────┐    │
│  │ + Send follow-up          GPT · 🎙 ■ │    │   ← Send = steer while running
│  └──────────────────────────────────────┘    │
│  ☎ in-call strip (only during a call)        │
└──────────────────────────────────────────────┘
```

Rules of the sketch:
- Status and standing work live only in the Project panel (and the full page). The panel is the `notes.md` render plus the `subscriptions/` folder; no second copy anywhere.
- Asks, approvals, and worker completions are cards in the transcript at the turn where they happened; answering writes a user bubble below the card and the card folds to one line.
- The composer keeps one optional line above it: queued follow-ups, one row each, with Send now (steer), Edit, and ✕. It appears only when the queue is not empty.
- While a turn runs, Enter steers. Shift+Enter (or the menu) queues. Stop is its own control and never shares a disc with Send.
- The in-call strip is the only other thing allowed under the composer, and only during a call.

## Evidence index

- Stills: `media/audit-multitasking/00-fresh-home-tab-plan-strip.png` (gc chore in an empty chat), `01-root-chat-after-scenarios-ab.png` (raw done rows, repeated answers), `02-desktop-typed-while-running.png` and `02b-zoom-queued-row-and-composer.png` (queued in the window, kernel inbox empty), `03-desktop-after-followup-landed.png`, `04-plan-strip-with-real-project-page.png` and `04b-zoom-plan-strip-21-items.png` (strip fills the column), `05-project-page-full.png` (page empty after eight root turns), `06-ask-parked.png`, `07-ask-skipped-by-unrelated-message.png`, `08-panel-project-after-root-writes-notes.png` (panel and strip showing the same items). All eleven present on 2026-09-16.
- Transcripts and scripts: `media/audit-multitasking/transcripts/` (`root-transcript-compact.txt` with the indices used above, `root-transcript.jsonl`, `home-root-restart-test.txt`, `child-slow-two.txt`, `scenario-a-frames.jsonl`, and the six Python drivers `kclient.py`, `scenario_a.py`, `scenario_b.py`, `scenario_c.py`, `desktop_typed_while_running.py`, `desktop_restart_queue.py`, plus `show.py`). All present on 2026-09-16.
- QA scenarios to add (26): `internal/qa/inbox/2026-09-13-multitasking-audit.md`. Present on 2026-09-16, unchanged since 2026-09-13 20:37.
- Repository tests that pin the findings: `crates/arbos-kernel/tests/multitask_e2e.rs` (#122: scenarios 3, 9, 12, 13, 15 of the QA list) and `crates/arbos-kernel/tests/audit_kernel_2_e2e.rs` (#128: scenario 17, the nudge, `grep scope=history`), both on `main`.

## Appendix (added 2026-09-16): where the fixes stand

Built only from merged pull requests in `unarbos/arbos`; nothing here is from the author's record.

| # | Fix | Shipped in | State on 2026-09-16 |
| --- | --- | --- | --- |
| 1 | Send steers while running; Queue alternate; Stop separate | [#129](https://github.com/unarbos/arbos/pull/129) (merged 09-13 22:23 into `main`) | Done; `notes.md` records the driver measuring a steer in 0.24 s. [#240](https://github.com/unarbos/arbos/pull/240) and [#252](https://github.com/unarbos/arbos/pull/252) keep a steer inside its turn in the transcript |
| 2 | Queue held by the kernel as inbox files | #129 | Done; survives a relaunch. [#135](https://github.com/unarbos/arbos/pull/135) stops a steer from showing as a queued follow-up; [#292](https://github.com/unarbos/arbos/pull/292) stops a worker's report from showing as one |
| 3 | Typed text on a parked ask is never a skip | #129 | Done; the words are the answer and show as a user line. #240: a typed option name is the pick once |
| 4 | Live-children cap; archive finished workers | [#122](https://github.com/unarbos/arbos/pull/122) (cap, archive opt-in), [#144](https://github.com/unarbos/arbos/pull/144) (archive on by default), [#152](https://github.com/unarbos/arbos/pull/152) (waited-for workers archive too), [#148](https://github.com/unarbos/arbos/pull/148) (desktop "N archived" row), [#213](https://github.com/unarbos/arbos/pull/213) (`say` to an archived worker) | Done end to end |
| 5 | One report per child; `waited` on a `say`-resolved wait | #122 | Done; `multitask_e2e.rs` scenarios 12, 13 |
| 6 | Batch done files into one root turn | #122 (`cause-2.md`, `cause-3.md`; `done_batched` log line) | Done; scenario 15 |
| 7 | Strip shows follow-ups only; chores hidden | [#128](https://github.com/unarbos/arbos/pull/128) (`Subscription.internal`), #129 (nothing pinned but the follow-up row and the in-call strip) | Done |
| 8 | A steer never cancels the tool batch | #122 (only a stop word cancels what has not started) | Done; scenario 3 |
| 9 | Done file as a compact worker card | #135 (`worker_card`) | Done |
| 10 | Notes nudge at turn end | #128 (kernel notice), [#136](https://github.com/unarbos/arbos/pull/136) (once per idle period, its own `nudge` event drawn dim) | Done |
| 11 | Ask and approve as inline transcript cards | #135 (`ChatItem::Asked` folds to one line; approve card; plan "Approve and run" card) | Done |
| 12 | Close the row on "no agent"; restore the active tab on relaunch | #129 (row closes; the real storm — attach-port probing — removed) | Row half done. The active-tab half was not found in any merged PR title or body; treat as open |

Not shipped, by the PRs' own account: `isolate` defaulting to `worktree` only when the task edits outside `.arbos/` (#128: "left for a decision"); the coordinator-role offer for existing places (fix list §2, no PR); the spoken heartbeat for the narrator (gateway nice-to-have).
