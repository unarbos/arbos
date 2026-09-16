> **RECONSTRUCTED — this is not the original file.** The original (10,902 bytes, written 2026-09-14 by the layout worker `bc-2a1318aa`) was lost with the whole `docs/` directory on 2026-09-16 (loss record: `internal/store-docs-loss-2026-09-16.md`). Rewritten on 2026-09-16 by the same worker from the surviving record: the findings ledger `internal/symmetry-findings.md`, the prompt catalogue `internal/symmetry-prompts.md`, the two-style PRs (#209, #223, #224, #225, #226, #240, #252, #279, #282, #288, #292, #294) and the reference stills under `media/cursor-reference/`. Every rule below has been checked against live Cursor in at least one cycle; where a rule is remembered rather than re-verified it is marked *(reconstructed)*. Written to `/tmp` and copied in, per the store-write rule.

# Project chat versus classic agent chat

Cursor has two chat styles, and Arbos renders both. This document says what each shows and hides, element by element, and gives the Arbos rendering rule per transcript kind. It is the spec the symmetry loop measures against; the ledger holds what was found wrong and when it was fixed.

## The two styles

**Project chat** — the root, the coordinator. In Cursor this is the chat of a *Project* (the Agents window's "New Project"). It is built for multitasking: many workers, little detail. It hides the machinery — no thinking blocks, no tool-call cards, no file diffs inline — and shows what a person managing several things needs: the prompt, a folded "Worked Xm Ys ›" header, worker lines, PR/doc/agent chips, short prose. Jacob's reference stills: `media/cursor-reference/chat-2026-09-14/01–03`, and the cycle-14 long-form pairs `media/cursor-reference/cycle-14/long-*-pair-*.png`.

**Classic agent chat** — a delegated worker opened from the Project (or any plain Cursor chat). Full detail: "Thought briefly" / "Thought for Ns" with the thinking expandable, tool-call cards (Read, Grep, Shell with output), Edited / Explored / Ran lines, code blocks, a Files Changed card. Reference stills: `chat-2026-09-14/04–05`, `media/mac/cycle-11/07-working-live`, `08-ran-card`.

**Arbos mapping.** The project's main chat (the root agent, `agent.md` role `coordinator`) is Project style. Every delegated worker chat — opened from the panel roster, a worker line, or a chip — is classic style. The style is a property of the chat's role, not of the window: a worker's chat is classic even when it is the only thing on screen.

## Element by element

| Element | Project chat (root) | Classic agent chat (worker) | Arbos as of cycle 16 |
| --- | --- | --- | --- |
| User prompt | right-aligned bubble, max 70 % of the column, 12 pt pad, right edge flush with the composer's plate; one bubble per prompt; the edit pencil outside, on hover | same | same (#226, F-07, F-16) |
| Steer typed mid-turn | an inline bubble *inside* the running turn's fold; never a second turn | same | same (#240); the live run stays "Running" past it (F-47, #252) |
| Turn header, settled | `Worked Xm Ys ›` folded; opens on click to the timeline | `Worked Xm Ys ⌄` open by default on the newest turn; older turns fold | same; the header is the body size (14 → 13 px in Inter, F-55) |
| Turn header, live | `Working <step> ⌄` — the agent's `status` line, else the kernel's derived step ("Reading main.py"), else "Planning next moves" | same | same (F-22); the heartbeat shimmers at body size (F-06) |
| Wake segments | each coordinator wake — a worker's report, a subscription — is its own `Worked Ns` block with the worker's line under it; one footer per run of turns between two prompts | n/a (a worker has one wake per turn) | same (#292, F-62) |
| Kickoff | a new Project opens on its header (icon, name, one line, "View Project Page") and "Setting up environment" shimmer; then `Worked Ns ›` and the greeting. Cursor's extra `Environment ready 12s` line is its cloud VM and has no Arbos equivalent (F-64) | n/a | same (#223, #226, F-29, F-48, F-49) |
| Thinking | **hidden**: the root shows a "Thinking" line while live and nothing after | `Thought briefly` (< 5 s) / `Thought for Ns`, click to expand the text; consecutive steps merge into one row | same (#224 F-15, #226 F-17, #252) |
| Tool calls, general | **hidden** — except the root's *own* quick calls (bash, read) which draw as one line each | a card per call: Read / Grep / Shell with its output, Edited with the diff | same (F-08) |
| Shell | `Running 1 command` live, then `Ran <description> ⌄` with the command card folded under it; the description is the tool record's `label` (#225) | the card with the output, open | same |
| Reads / searches | `Explored <file>, N searches, ran N commands` as one line *(the Cursor wording, from `chat-2026-09-14/05`)* | `Read <file> L1-3` lines and cards | same |
| Edits | `Edited <file>, updated the project page +N` as one line, no diff | `Edited <file>` line with the diff card | same |
| Prose | short paragraphs, chips inline; one paragraph once — a repeat across a tool call, or the agent's own `say` echoed as its reply, shows once | full prose, code blocks, bullets | same (#288 F-66/F-67); nested list spacing still wider than Cursor's (F-73, open) |
| Tool markup written as prose | **never shown** — cut from the stream and the settled line; a markup-only reply leaves no bubble | same | same (#279, F-63) |
| Chips | PRs `⛓ #139`, agents `⚙ name`, docs `📄 name` inline in prose, in worker lines and in the panel readouts | same | same (#135 and after; F-18) |
| Worker lines | `1 Working · <live step>` while running (several: `N Working` then a line per worker with its name and step); `Done <name>` when finished; a report line `⚙ <name> done — <last words>` under the segment header; a read-only worker carries a mark and a kind chip | n/a | same (#209, #292, #288). Cursor draws no read-only distinction — a deliberate divergence (F-56) |
| Working card | above the composer during a fan-out: "Working", "Stop All", one row per sub-agent with spinner and name; the "Working N" pill under it | n/a | same (#209, F-13) |
| Under-composer pills | Changes / Commit & Push / branch only when the place itself is a repo and not huge; Working / Agents; PRs | same | same (F-05) |
| Footer | thumbs, copy, fork, "Just now / 2m ago" — once per response, under the last answered segment of the run; never while the turn or its workers still run | same, per turn | same (#223, #292) |
| Date line | `Today 6:19 PM` over the first turn of the root; at every day crossing | none over turns; day crossings only | same |
| Ask / approval | in `auto` mode there are no ask cards; a question is plain prose and the turn waits (F-65). In `ask` mode: the question card with options, the typed option name picks once, the answer folds into the card; an approval is Cursor's row `Skip · Run ↵` (Enter runs) | same | same (#224, #225, #240, F-21) |
| Notices and nudges | dim single lines; the kernel's page nudge once, with ↻, repeats collapsed | same | same |
| Files Changed | a card listing changed files with +/− once the turn settles | same | same |
| Checklist (TodoWrite / `plan`) | a card in the turn that made it; the panel's Project section keeps the list | same | same |
| Subscriptions | a small card at the turn that created one; the list lives in the panel | n/a | same |
| Attachments | a thumbnail chip in the tray before sending and in the prompt card after | same | Arbos draws the chip in the card; whether Cursor's card keeps a thumbnail is unconfirmed (F-70, open) |
| Notifications | an OS notification when a reply, question or failure lands in a chat not in view; a dot on the project's tab until the chat is opened | same | same (#297, F-75) |

## Rendering rule per transcript kind

What the desktop does with each kind of line on the wire (`arbos_core::EventKind`, and the live frames), by chat style.

| Kind | Root (Project) | Worker (classic) |
| --- | --- | --- |
| `user` | the prompt bubble; a `user` while a turn is open is a steer, drawn inline in that turn | same |
| `wake` `user` / `kickoff` | the user line is the boundary; the kickoff opens the kickoff view | same |
| `wake` `done` / `say` / `serve` / `job` | a `ChatItem::Wake`: a segment boundary with its own `Worked Ns`; the `say` that caused it is drawn under the header as the report line | same shape, rarely seen |
| `wake` `plan` / `compact` | nothing (a worker's `plan` wake with a brief is its prompt) | the brief as the first card |
| `thinking` deltas | a "Thinking" line while live, dropped when the step settles | `Thought…` row, text expandable; `secs` from the settled record |
| `assistant` deltas / settled | prose; the settled line of step N replaces what step N's deltas built (`step` pairing, #252); `status: <step>` lines are the live headline, not prose; tool markup cut | same |
| `tool` bash | `Running 1 command` → `Ran <label> ⌄`, card folded | card open with output |
| `tool` read / grep / find / search | folded into the `Explored…` line | a card each |
| `tool` write / edit / apply_patch | folded into the `Edited…` line; Files Changed card at the end | line plus diff card |
| `tool` spawn | a worker line (`1 Working · step`), the Working card, the `Working N` pill | n/a |
| `tool` status | the live headline's words; never a row | same |
| `tool` say (to a child) | nothing | n/a |
| `tool` ask / approval | plain prose in `auto`; the question or approval card in `ask` | same |
| `tool` plan / todo | a checklist card; the panel updates | same |
| `tool` subscribe | a small card | same |
| `say` from a child | the report line under its wake's header (`⚙ name done — words`); a question from a child as a From block | n/a |
| `notice` | a dim line; `failed: true` in the danger colour; identical consecutive notices collapse | same |
| `nudge` | one dim line with ↻ per idle period | same |
| `interrupted` | the turn's header reads "Stopped by you Ns" | same |
| `turn_complete` | stamps `Worked Ns` on the turn's prompt or wake; the footer appears | same |
| `notify` / `seen` | the tab's dot and the OS notification; `seen` on view | n/a (workers do not notify) |

## Behaviour the Project chat never shows

These are behaviours, not pixels; when Arbos does one of them it is a finding (features inbox) even when every element above is right. Cursor's coordinator never: writes tool markup as prose (F-63); asks the user for approval to run its own `plan` call (F-10); narrates a command before running it (F-08); refuses to answer a question it was not asked (F-09); repeats a paragraph (F-20, F-67); says it has no workers while the panel shows six archived (F-57); spawns read-only workers for a writing task (F-56); writes headings through the checklist tool (F-71).

## Known gaps (open at the time of writing)

- F-70 — whether Cursor's prompt card keeps an image thumbnail; unconfirmed on the rig.
- F-73 — nested list spacing (`*   1. …`) wider than Cursor's.
- F-74 — the search palette: Cursor's is wide and centred with filter tabs and sections; ours is small and matches chat titles only.
- F-33 — no branch pill for a remote repo.

## Provenance

Rules verified live against Cursor in cycles 7–16 (2026-09-14 to 09-16), on the Linux rig at 1x with the same Inter face on both platforms since #266. The Mac 2x stills of cycles 11–12 (`media/mac/cycle-11/`) predate the Inter change. What Linux cannot show is listed in the ledger under "Untested by decision".
