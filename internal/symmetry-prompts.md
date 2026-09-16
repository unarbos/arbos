---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Cursor symmetry loop — prompt catalogue

The running list of prompt kinds for the "Arbos chat looks exactly like Cursor's" loop. Each cycle takes kinds not yet covered; once every kind is covered the list starts again, and earlier kinds are re-run now and then for regressions. Rule from Jacob (2026-09-14): where his Mac stills (`media/cursor-reference/chat-2026-09-14/`) and the Linux Cursor build differ, the Mac wins.

Both apps run on the Linux VM: Cursor 3.20.21 (Agents window, dark, sidebar hidden, local project `/tmp/parity-proj` on `master`, Auto model) and Arbos (`main` or the cycle branch, dark, GPT 5.4 Mini via OpenRouter), the same 1440×900 frame. Harness: `/tmp/polish/sidebyside.py` on the VM (stills at t+2 s, t+8 s, t+20 s and idle; a 12 s recording of the working state where a still cannot show the motion). Pairs: `media/cursor-reference/cycle-N/<tag>-<prompt>-<moment>-pair.png` (Cursor left, Arbos right), singles beside them, recordings as `*-working.mp4`.

## Scope (widened 2026-09-15, Jacob)

The loop covers the whole app as a new user meets it, side by side with Cursor, not only the transcript. Each cycle walks this list and records what differs (pixels or behaviour) in `internal/symmetry-findings.md`:

1. Launch and first run: window placement (a saved frame off-screen), the permissions sheet, the Home tab.
2. ⌘T and the folder picker: `~`, `~/`, a partial name, a non-existent path (Create), a folder of repos (`~/Code`), a non-repo folder, an iCloud path (`~/Library/Mobile Documents/…`), a remote host.
3. What a new project lands on (the new-Project view, never the Project page).
4. The Project page and every way in and out: panel header, ⌘2, "View Project Page"; Back to chat, Escape, ⌘1, the tab.
5. The panel: every row and click (project head, agents, archived, Project section, Files, Standing, the gear, search, +).
6. Tabs: cycle (ctrl-tab, ⌘⇧[ ]), close, reopen through the opener, two named coloured tabs, one spinning.
7. Settings: every section, open and close, ⌘, and Escape.
8. The mode chip and `/mode`; approval and ask flows in each mode (auto, ask, plan).
9. Under-composer pills on a repo vs a non-repo folder (branch, machine; Changes, Commit & Push, PRs, Working/Agents).
10. Ask / answer cards; approval cards; the refused-answer shape.
11. Reopen after relaunch: root and worker chats read like the live ones.
12. Resize and small windows (900×600), the panel folded; light theme.

Behaviour is in scope: when Arbos does something Cursor would not (asks, narrates a command before running it, refuses an answer, writes a file on a read question), it is a finding and a features-inbox note the same turn.

## Cold track

Every prompt kind below also runs "cold": a fresh place (`/tmp/cold-<id>`, no `.arbos`, a `git init` for the repo kinds), first message, no history — where Jacob meets things. Harness: `sidebyside.py` with `SBS_PLACE=/tmp/cold-<id>` (fresh store each run). Cold frames carry the tag `cold-<id>`. The kickoff view is the cold frame's start state.

## Mac measurement

Type and spacing are measured on macOS Retina, not only Linux. Each cycle a capture script goes to the Mac worker (bc-b337f0b1) through the repo's `qa-results` branch — `qa-results/inbox/cycle-N/capture-script.md` and a copy of this catalogue — and its 2x stills come back under `media/mac/cycle-N/` on the same branch; the layout worker copies them into the store's `media/mac/cycle-N/` and diffs them against the Linux frames. Anything that differs by more than 1 px at 1x is a finding. The Mac worker's rig: the `main` build, the test place `~/arbos-fresh/places/demo`, the driver socket for scripted clicks.

## Remote track (from 2026-09-15, Jacob)

The loop also tests remote places, cold and warm, on ArbosLife and Templar (SSH via the agents' key in the 1Password Arbos vault; aliases `arboslife`, `templar` in the VM's `~/.ssh/config`). Scratch folders only: `~/arbos-qa/<cycle>/…` under the SSH user's home; never other users' processes, never Jacob's validator or subnet dirs, no sudo, no kill of anything the loop did not start; hosts and keys redacted in stills and logs (the alias may show, the address never). Cursor's Agents window has no SSH-remote flow (its Workspace picker offers repos and the cloud), so remote pairs are Arbos before/after unless a Cursor Remote-SSH IDE frame is the better reference.

1. ⌘T → machine → a scratch folder; no `arbos-kernel` there → the app installs it (same os-arch: scp of its own binary; else build) and connects, with a visible "Installing Arbos on <host>…" state; a second open is instant.
2. Remote kernel older than the desktop → "Updating Arbos on <host>…" before connecting; same version → no update; newer → still connects.
3. A normal prompt on the remote, a worker spawned there, a steer, reopen after relaunch; the SSH connection killed mid-turn and the reconnect watched.
4. The remote binary removed between runs to force the install path again.

Media: `media/remote/cycle-N/`. Kernel gaps (install/update over ssh, version handshake, reconnect) go to the features inbox with the failing step; the desktop owns how the states render.

## Coverage (rotating; Jacob's standing order 2026-09-16)

One row per app area. "Last" is the last cycle that looked at it side by side (or the gate that exercises it every run). Anything older than three cycles is due. The Linux rig is the rig (Jacob, 09-16); what Linux cannot show is listed in `symmetry-findings.md` under "Untested by decision".

| area | last | how | due? |
| --- | --- | --- | --- |
| chat: Project style (root) | 13 | p24, p13 steer; every gate | — |
| chat: classic style (worker) | 12 | pp2/pp4 worker reopen | 15 |
| long-form project (many turns, notes restructure, workers come and go, archived, context re-found, scroll) | 14 (l1–l13), **16 running** (d1–d10, research/docs) | l1–l13, d1–d10 | — |
| kickoff / new-project landing — the first thirty seconds, refused and silent providers | **16** (f1, f2) | f1/f2 cold, `new-project-kickoff` gate row | every cycle from now |
| launch, first run, permissions sheet | 12 | gate `launch` phase (Linux rows only) | — |
| tabs: cycle, close, reopen, colours, spinner | 12 | gate `tabs` phase | 15 |
| opener: ~, partial, create, prefix sibling, remote host | 13 | #258, gate opener rows | — |
| Project page: in/out, back, Escape, ⌘1, files grid, notes render | 13 (l1 page capture) | gate `project-page` rows | — |
| panel: agents tree, archived row, Project section, Files, Standing | 12 | gate `panel` phase | 15 |
| settings: every section, ⌘, / Escape / ⌘W, typography stepper, bionic toggle + eyesight check | 13 | gate `settings` phase (`weight-visible`) | — |
| mode chip, /mode, approval + ask cards per mode | 12 | p9/p10, gate `ask-*` rows | 15 |
| under-composer pills: repo vs non-repo, Changes, Commit & Push, PRs, Working/Agents | 12 | gate `pills` rows | 15 |
| bottom bar (version, Update) | never side by side | Jacob's still `media/mac/update-bar/`; gate has no row | **14** |
| search (⌘F chat search palette) | 10 | gate `search` row | **14** |
| themes: light, dark; tint | 11 | cycle-11 `13-light`; gate `appearance-*` | 15 |
| window sizes: 900×600, panel fold, 1600×1000 | 11 | cycle-11 `14-small-window` | **14** |
| remote places: install/update/reconnect states, kernel version handling, attachments as bytes | 15 (attachment check on Templar) | remote track scenarios 1–2; 3–4 open | 16 |
| voice / dictation entry points (Fn, mic button, call strip) | 9 | gate `composer-voice` row; Fn via driver | **15** |
| relaunch / replay (root + worker read like live) | 13 | l13 relaunch leg | — |
| rewind / fork / checkpoint | 10 | p14 | 15 |
| notices, nudges, refused-answer shape | 13 | F-45 open | — |

## Kinds and their state

| id | kind | prompt (both apps, verbatim) | covered | cycle | notes |
| --- | --- | --- | --- | --- | --- |
| p1 | one-line answer | Reply with exactly: The quick brown fox. | yes | 1 | footer, headline, bubble |
| p2 | code answer (code blocks, bullets, thought line) | hello write bubble sort | yes | 1 | Cursor edited a file instead; still useful |
| p3 | status question | what are we doing right now? | yes | 1 | agent-link chips on the Mac stills |
| p5 | small multi-file edit | Add a mul(a, b) function to math_utils.py and call it from main.py with mul(4, 5). | yes | 2 | Changes pill, Files Changed card |
| p6 | long-running shell command, streaming output | Run this exact shell command and show me its output as it arrives: `for i in 1 2 3 4 5 6; do echo "step $i"; sleep 2; done` | yes | 3 | recording of the working state |
| p7 | failing test, then the fix | Write tests/test_math_utils.py with a failing test asserting add(2, 2) == 5, run it, show the failure, then fix the test so it passes and run it again. | yes | 3 | error and retry path |
| p4 | sub-agent fan-out | Use parallel sub-agents: one reviews math_utils.py for edge cases, one writes docstrings for every function, one drafts a CHANGELOG.md. Then merge their results. | yes | 3 | working lines, worker chips |
| p8 | long answer with headings and a table | Explain the difference between lists, tuples, sets and dicts in Python. Use headings for each, one comparison table, and finish with three bullet recommendations. | yes | 3 | headings, table, bullets |
| p9 | cancel mid-turn | (p6 sent, Stop pressed 5 s in) | yes | 3 | stop button, interrupted line |
| p10 | ask / clarification | Before doing anything, ask me one multiple-choice question with two options, alpha and beta, about which name to use for a new module. Wait for my answer. | yes | 4 | question card |
| p11 | plan-mode request | Make a plan, do not write code yet: how would you add a command-line interface to this project? List the steps. | yes | 4 | plan card / mode chip |
| p12 | multi-file refactor | Rename the function `sub` to `subtract` everywhere in this project, keeping behaviour, and run main.py to check. | yes | 4 | many edits, Files Changed rows |
| p13 | follow-up steer while working | (p6 running; 3 s in send: "Also print the date at the end.") | yes | 4 | steer card |
| p14 | rewind / checkpoint restore | (after p12: restore the checkpoint before it) | yes | 4 | Cursor's ↺ on the prompt card did not reveal on hover of the pinned card — retry restore icon vs rewind |
| p15 | git commit and push | Commit all current changes with a clear message. Do not push. | yes | 5 | Commit & Push pill |
| p16 | web search | Search the web for the current stable Python version and tell me the number with a source link. | yes | 5 | search tool card, source chips |
| p17 | MCP / tool cards | (tool with a card: browser open, terminal) Open a terminal and run `python3 main.py`. | yes | 5 | tool cards |
| p18 | image / file attachment | (attach media/parity/cursor/public-docs-agent-overview.png) What is in this screenshot? | no | | attachment chip in the bubble |
| p19 | very long thinking turn | Think carefully, step by step, about the fastest sorting algorithm for 10 nearly-sorted integers, then answer in one line. | yes | 5 | thinking fold, "Thought for Ns" |
| p20 | Project / Context panel after a busy session | (after p4 + p12: open the Project page) | yes | 6 | panel vs Cursor's right panel |
| p21 | error path | Run `python3 does_not_exist.py` and tell me what happened. | yes | 5 | failed tool card, retry |
| p22 | coordinator shell — regression check (after #190 lands on `main`) | (a) Run `ls -la` and tell me how many files there are. (b) Run the test suite with `python3 -m pytest -q` and report the result. | yes | 6 | (a) must run in the coordinator itself: a `bash` tool row under the turn, no worker spawned; (b) must spawn a worker (a worker line / panel row), never the coordinator's own shell. Drift on either → `features-inbox/`. Re-run every third cycle. |

## Style track (from cycle 7)

Cursor has two chat styles — the Project chat (root) and the agent chat (a delegated subagent or a classic chat); `docs/project-chat-vs-agent-chat.md` says what each shows. From cycle 7 every prompt kind is captured in both styles: the prompt goes to Cursor's Project *Parity* (No Repo, cloud) and to Arbos's root; the first delegated worker is then opened on both sides. Pairs carry the tag `s<N>`: `s1proj-root-pair.png` (Project chat vs Arbos root), `s1proj-worker-pair.png` (subagent vs Arbos worker). Cost note: the Project side runs on Jacob's Cursor cloud, so it takes the short prompts (pp1, pp2) and one edit kind per cycle, not the whole list.

| id | kind | root style covered | worker style covered | notes |
| --- | --- | --- | --- | --- |
| pp1 | one-line answer | 7 | — (no worker) | |
| pp2 | three-worker fan-out | 7 | 7 | Cursor: workers in the Working pill's popover, the report folded into the woken turn; subagent composer disabled |
| pp3 | todo checklist (root) | 8 | — | Arbos: card from the `todo` echoes; Cursor TodoWrite still to capture live |
| pp4 | titled steer to a worker | 8 | — | title shows as the worker's live line until its first `status`; model copies the protocol example at once (kernel note filed) |
| pp2-reopen | fan-out, then relaunch and read back | 9 | 9 | root and worker after a relaunch must read like the live ones |
| p22a | read-only shell question ("Run ls -la and tell me how many files there are.") | 12 (Project vs root) | — | Cursor Project chat "Worked 13s › … 5 items"; Arbos "Worked 6s ⌄ / Ran List directory contents / There are 5 files" |
| p23 | read-only question, no run ("In one sentence, what does main.py do? Do not run anything.") | 12 | — | Arbos: Explored main.py, one sentence; Cursor frame stale (F-44) |
| p24 | multi-step turn: prose, tool, prose, tool, prose in one turn ("Say the single word first. Then run `ls`. Then say second. Then run `pwd`. Then say third.") | 13 (step pairing) | — | Arbos: first / Ran List current directory contents / second / Ran Print current working directory / third — one line per step, nothing doubled (`cycle-13/p24-multi-step-turn-after.png`); Cursor Project chat folds all but the last |
| m1/m2 | tool markup written as prose: a markup-only reply; prose then markup ("Reply with exactly this text … `<function_calls><invoke …>`") | 14 | — | Arbos: no bubble / prose only (#279); Cursor has no equivalent to capture — it never writes markup as prose |
| l2 (re-run) | fan-out with worker reports — per-wake Worked segments | 15 | 14 | Arbos now: a `Worked Ns` per report with the worker's chip line under it (#292) |
| r1 | read-only explore worker beside a writer | 15 | — | mark + kind chip (#288); Cursor draws no distinction |
| f1 | fresh project, provider refuses (cold place, `openai/gpt-5.4-mini` on the blocked key): the first thirty seconds | 16 | — | Arbos: `Switched to … for this turn.` + Details, then the greeting (#298) |
| f2 | fresh project, provider silent (cold place, api_base at a socket that never answers) | 16 | — | Arbos: shimmer with clock at 20 s, hint at 60 s, never the greeting (#298) |
| pp5 | root's own quick command ("what is in this repo") | mac-fixes | — | Cursor: Running 1 command → Ran <description> ⌄ with the card; Arbos matched in the Mac PR (`media/cursor-reference/mac-fixes/repo-question-pair-cursor-vs-arbos-after.png`) |
| p5 | small edit | 2 (classic) | 7 (Arbos) | Cursor Project + worker edit still to capture |
| p4 | fan-out with edits | 3 (classic) | — | |

## Cycle log

- **Cycle 12** (branch `cursor/symmetry-cycle-12-aa39`, on `main` `848e003`): live Cursor (Project *Kickoff*, sidebar open) vs Arbos root on p22a, p23, p13, p21, p10 — the read-only / wrong-shape / steer class. Findings F-40 … F-44: a steer stays inside its turn, a typed option name is the pick once, the `ask` row hidden; F-37 seen again (bash returns after the first line); the Cursor side of the harness now wheels to the bottom before each shot. Mac leg: `qa-results` `inbox/cycle-12/capture-script.md` (ten captures incl. the cycle-11 fixes live). Pairs: `media/cursor-reference/cycle-12/c12b-*-end-pair.png`, before/after `c12-p13-before-cols.png` → `c12after-arbos-p13-end.png`.

- **Cycle 11** (branch `cursor/kickoff-live-aa39`, on #227 + #226): the live kickoff turn replaces the static greeting — header block, "Setting up environment", then "Worked Ns ›" + greeting + footer, the date line over it, typed words wait for it (`media/cursor-reference/cycle-11/kickoff-pair-cursor-vs-arbos.png`); the Mac worker's seven findings (F-21 … F-27, stills in `media/mac/cycle-11/`, reply in `media/mac/cycle-11/reply.md`) and two loop findings (F-28 bionic default, F-29 greeting under the prompt) fixed. Cold track: p1, p5, pp2, p6 on fresh places. Remote track: arboslife 1/2/4 and templar 1/3 run (install from nothing, fan-out + steer, relaunch, tunnel kill → reconnect in 2.8 s); desktop findings F-30 … F-32, F-35, F-39 fixed, F-33/F-38 open, behaviour F-34/F-36/F-37 in `features-inbox/2026-09-15-remote-track-findings.md`; stills `media/remote/cycle-11/`.

- **Mac fixes** (branch `cursor/mac-fixes-2-aa39`, on #225): Jacob's ten findings from the fresh Mac install (F-01 … F-10 in `internal/symmetry-findings.md`), one PR. The loop is widened from this cycle (Scope, Cold track, Mac measurement above); the ledger starts.

- **Cycle 10** (branch `cursor/symmetry-cycle-10-aa39`, on `main`): the approval card takes Cursor's row — "Skip" in plain words, "Always Run" when the agent offers a standing allow, "Run ↵" as the default, Enter in the empty composer runs it (reference `cycle-10/cursor-approval-row-reference.png`, from cycle 5's p16); the Working pill reads "Agents" once the workers are done and opens the same card with a check per worker (`cursor-idle-agents-pill-crop.png`); the ↓ disc sits centred low; two reasoning steps split only by a hidden `status` call read as one "Thought briefly", live and on replay; a worker's `plan` calls are checklist cards like `todo`. Pair: `cycle-10/worker-thoughts-plan-cards-before-after.png`.

- **Cycle 9** ([#223](https://github.com/unarbos/arbos/pull/223), rebased on `main` after #221 merged): the features agent's answers (#221) and the 09-13 report's open desktop rows. Reopen parity: a worker chat read back shows "Thought briefly / Thought for Ns" from the settled `thinking` records (secs kept, consecutive steps merged), the brief from the wake's `brief` field, `todo` calls as checklist cards; a root read back shows the date line, "Worked 29s" from the kernel's clock, three Done lines even for archived workers, "6m ago". Panel rows read "title — summary" (report row 16); the under-composer ring turns while any agent works (row 1; Cursor keeps one there always — `cycle-9/cursor-under-composer-ring.png`). Rows already closed in earlier cycles, confirmed on the stills: "Thought Ns" (3), PRs pill (4), thumbs + time (8), date divider (12), tab spinner on sub-agent work (15, #71 badge), repeated sub-agent block (22). Pairs: `media/cursor-reference/cycle-9/root-reopen-before-after.png`, `root-live-vs-reopen.png`, `worker-reopen-before-after.png`. Model for the thinking runs: `google/gemini-2.5-flash` with `reasoning_effort = "low"` (gpt-5.4-mini streams no reasoning through OpenRouter).

- **Cycle 8** ([#209](https://github.com/unarbos/arbos/pull/209), on `main` with the process-parity slices): the Working card, the `todo` checklist card, the title as the worker's live line (confirmed, nothing to add), `status` as the timeline step (confirmed). Cursor's Project chat shows a card above the pills during a fan-out — "Working", "Stop All ×", a spinner and name per worker — and Arbos showed only the pill (Jacob, from `cycle-7/s1proj-root-pair.png`). Arbos now stacks the same card over the pills on the root: opens by itself at fan-out, × closes it for that fan-out, the Working pill toggles it, a row opens the worker, Stop All cancels them; rows at Cursor's 29 px pitch. Pairs: `media/cursor-reference/cycle-8/working-card-pair-cursor-vs-arbos.png`, `working-card-before-after.png`, `working-card-crop-pair.png`. Also confirmed: a modified tracked `.pyc` raises no Changes pill and no Files Changed row (`cycle-7/s1after-arbos-pyc-only-no-changes.png` vs `s1after-arbos-pyc-and-main-changes-2.png`).

- **Cycle 7** (PR #199): the two-style track opens. Live Cursor Project *Parity* and its subagent vs Arbos root and worker. Root: project header, date line back, fork on hover. Worker: brief as the first card, no duplicated headline, status calls hidden, archived composer in Cursor's words, no pills. Doc: `docs/project-chat-vs-agent-chat.md`. Kernel asks: thought duration stamps, live worker thoughts (`features-inbox/2026-09-15-symmetry-two-styles-kernel-asks.md`).

- **Cycle 1** (PR #187, merged): p1, p2, p3. Prose 14/23, bare code plates, footer order, borderless bubble, no date line, branch/machine pills.
- **Cycle 2** (PR #188): p5. Changes + Commit & Push pills, Files Changed card, capitalised run lines, fontconfig sans on Linux.
- **Cycle 6** (PR #194): p1 and p5 regressions (no drift), p22 (#190 check: (a) as intended, (b) drift — coordinator ran pytest itself, filed), p20 (Project page vs Cursor's Files pane, by design). Kernel calls folded into the run line; Files Changed only after edits or workers.
- **Cycle 5** (PR #193): p21, p17, p19, p16, p15. Prompt card as typed; no bare run line. Cursor's approval card recorded (Skip · Always Run · Run ↵) for a later cycle.
- **Cycle 4** (PR #191): p10, p11, p12, p14, p13. Headline without diff badge; rewind line in plain words. Cursor queues follow-ups (opt-in steer) where Arbos steers by default (Jacob's decision, kept); both models asked in prose, no question card this run.
- **Cycle 3** (PR #189): p6, p7, p4, p8, p9. Fresh turn's timeline stays open, worker lines name their worker, Files Changed without junk and only after real work, Continue Working and Push pills, no bare Worked. Kernel ask filed: the coordinator refuses shell prompts instead of delegating (`features-inbox/2026-09-14-symmetry-cycle-3-coordinator-shell.md`).
