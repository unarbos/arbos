---
cursor:
  subagentId: "bc-938c8002-ea3d-53bf-9cf2-9dcfc9cf48ec"
---

# Multitasking audit: scenarios QA should add

From `docs/multitasking-audit-2026-09-13.md`. Kernel `844e835` (integration), desktop stack `cursor/project-panel-94d6` at `d4981aa`. Drivers that reproduce each finding are in `media/audit-multitasking/transcripts/` (`kclient.py` talks to the kernel socket; the `desktop_*.py` scripts use `internal/parity/arbosdriver.py`).

Each scenario: setup, action, the pass condition the motto needs, and what happens today.

## Responsiveness

1. **Typed while running lands as a steer (desktop).** Start a 60 s root turn (`spawn wait=true` on a `sleep 45` worker). Type a line at +8 s. Pass: a `kind = "steer"` file appears in `agents/root/inbox/` within 1 s and the line is on the transcript at the next tool boundary. Today: inbox empty, line lands after `turn_complete` (+61 s); composer shows "Interrupt now · Edit · ✕".
2. **Typed while running lands as a steer (kernel).** Same, via `arbos-kernel run --steer`. Pass today (+0.8 s to the boundary). Keep as the regression guard.
3. **A steer does not cancel the work the user asked for.** Send a prompt whose first action is one `spawn`; steer at +4 s before the model answers. Pass: the spawn runs, the steer lands after it. Today: `spawn` returns "skipped: user steered"; the worker starts 30 s late.
4. **Queue survives a window restart.** Type a follow-up during a turn, quit the desktop, relaunch. Pass: the follow-up runs when the turn ends. Today: lost (`home-root-restart-test.txt`).
5. **Voice utterance during a running turn steers.** With the mock voice harness (#100 `tests/`), speak while root runs. Pass: `channel = "voice"`, `kind = "steer"` inbox file. Expected to pass; pin it.
6. **Heartbeat.** Stub model silent 12 s. Pass: `working 5`, `working 10` frames, "Thinking for Ns" row, row gone at the first delta. Passes per the heartbeat note; add the narrator: no spoken line for 20 s → a spoken "still thinking".

## Delegation

7. **Root delegates a three-part request.** Coordinator place; prompt with two coding/writing parts and one read-only question. Pass: two `spawn` calls in one response, the question answered inline, root idle within 10 s, both briefs carry the six fields with `read_first` naming `project-context.md`. Passes today; also assert `rules` is not the parameter description verbatim and `isolate=worktree` is set only when a worker edits files outside `.arbos/`.
8. **Existing place keeps its tools.** Open a place with no `[root] role`. Pass: root may run `bash`; no coordinator contract in the prompt. Passes; pin it.
9. **Cap counts live children only.** Spawn 8 short workers, wait for all to finish, spawn one more. Pass: the ninth starts. Today: "you already have 8 live children".
10. **Notes stay current without being asked.** After scenario 7 completes, read `.arbos/notes.md`. Pass: one item per workstream, fresh readouts, `<tldr>` absent under six items. Today: template unchanged after eight root turns.
11. **A child does not write the project page.** Worker calls `write .arbos/notes.md`. Pass: refused with `PAGE_REFUSAL`. Expected pass.

## Push-back

12. **One report per child.** Worker without `wait`. Pass: root gets exactly one waking file (`done`) per child turn and one root turn. Today: the child's `say` plus the done file, two root turns, the same result read to the user twice.
13. **`spawn wait=true` gives the result once.** Pass: tool result carries the child's words; no done file follows for that turn. Today: the done file follows and root repeats its one-line answer.
14. **Child done does not interrupt the user.** Root idle, user mid-thought (composer has text, nothing sent). Child finishes. Pass: the panel row turns to a check, a compact card lands in the transcript, the composer text is untouched, no root model turn unless notes need it. Today: a root turn runs and usually speaks; raw "Turn ended. Last words: … (transcript: …)" is shown.
15. **Done storm is batched.** Five workers ending within 15 s. Pass: at most one root turn per idle window, one message ("5 finished"), notes updated once. Today: one turn and one message per child (four identical "still blocked" lines).
16. **Root does not repeat a result.** Scenario B's haiku. Pass: the haiku is read to the user once. Today: three times.

## Plan representation

17. **Plan strip on an empty chat.** New place, new chat. Pass: nothing above the composer. Today: "Plan · 1 standing" with the kernel's weekly `git gc` chore.
18. **Project page never fills the chat column.** Seed `.arbos/notes.md` with 21 open items; end a root turn. Pass: the transcript keeps at least half the column; the page renders in the Project panel only. Today: the strip takes the whole column, raw `[label](url)` markdown, ▷/✕ on notes rows.
19. **Standing work appears once.** One `subscribe add kind=timer`. Pass: one row, in the Project panel's Standing section. Today: in the strip and in the panel.
20. **Ask is a card where it was asked.** Root parks a question, the user scrolls up, sends nothing. Pass: card visible at its turn; a later answer writes a user bubble. Today: card pinned above the composer; the answer shows only as the model's next turn.

## Ask parking

21. **A typed thought while a question stands is never lost.** Park an ask; type an unrelated line with no option picked. Pass: either the text becomes the answer's free text, or it runs as a normal message after the user resolves the card; the words are on the transcript either way. Today: the answer is sent as a skip and the words vanish (transcript lines 119-123).
22. **Kernel restart with a parked ask.** Already in `2026-09-13-ask-parks.md`; keep.

## Reconnect

23. **Deleted child under a live window.** Delete a finished child's folder while the window shows it (or delete the chat from another window). Pass: the window drops the session; kernel log shows no repeated `attach_open` for it. Today: reconnect about twice a second forever, ghost `transcript.jsonl` re-created in the deleted folder.
24. **Relaunch restores the active tab.** Two tabs, second active, quit, relaunch. Pass: second tab active. Today: tab 0.

## Context

25. **Child can reach history.** Brief says "find what the earlier worker wrote about X". Pass: the child uses `grep path=.arbos/agents …` or a `scope: history` sugar and finds it. Today: possible but nothing tells the child; root is told never to read worker transcripts.
26. **Project context arrives once.** Pass: the child's prompt has `project-context.md` injected and the brief does not tell it to read the same file again. Today: injected and re-read (one extra tool call per worker).
