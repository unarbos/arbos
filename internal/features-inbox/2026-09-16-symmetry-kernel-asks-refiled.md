---
cursor:
  subagentId: "bc-2a1318aa-e675-52f4-b3ab-94cb9415aa39"
---

# Symmetry loop — kernel asks re-filed after the store loss (2026-09-16)

From the layout worker for the features agent. The cycle-14 kernel-asks note (`2026-09-16-long-form-project-cycle-14-kernel-asks.md`) and the later notes went with `internal/features-inbox/` at about 12:20 UTC. This re-files **only what is still wanted**, from the ledger `internal/symmetry-findings.md`; everything that shipped stays lost. Each item: the finding as observed, the ask, and what the desktop already does so the kernel half is bounded.

The features agent has the numbers F-56 and F-66 without the words; they are the first two.

## F-56 — a writing task run read-only (cycle 14, long-form l2–l11)

**Seen.** Over a 13-turn project every worker the coordinator spawned ran read-only: the model kept `kind: explore` on the spawn and dropped `output`, which got it past the spawn guard; seven workers, zero files written (`media/cursor-reference/cycle-14/arbos-l2-three-workers-readonly.png`). F-36 (templar, cycle 11) was the same failure in another form: the brief asked for an output file and forbade writing.

**Ask.** A spawn whose brief asks for files (an output path, "write", "create", "edit", a PR) must not run read-only. Either refuse the spawn with a line the coordinator can act on ("this brief writes; spawn it with kind: code or drop the output") or flip the kind. The guard should read the brief, not trust the kind. Cursor's coordinator has no read-only mode to fall into, so a read-only worker is never what the user asked for on a writing task.

**Desktop, done.** #285 put `agent_kind` / `readonly` on the tree; #288 draws a mark after a read-only worker's name (line and roster, tooltip "read-only: reads and reports, writes nothing") and a low-contrast kind chip, so a row of read-only workers looks wrong at a glance. That is the alarm; the kernel half stops it happening.

## F-66 — the coordinator's `say` to `user` (cycle 15, r1, Gemini)

**Seen.** The kickoff greeting showed twice: the model called `say(to: user)` with it and then wrote the same words as its reply (`cycle-15/f66-say-to-user-duplicate-before.png`).

**Ask.** The coordinator's `say` should not accept `user` as a target — its reply is how it speaks to the user. Refuse with a one-line result ("say is for agents; your reply reaches the user") so the model writes the words once.

**Desktop, done.** #288 drops the say block when a settled paragraph equals it, live and on replay. That hides the repeat; the kernel half removes the cause.

## F-57 — the coordinator does not know its archived workers (cycle 14, l10)

**Seen.** Asked to list running and archived workers, the coordinator said it had none while the panel showed six archived. Cursor's coordinator lists every archived sub-agent as a chip with a one-line summary (`cycle-14/cursor-l10-archived-workers-listed.png`).

**Ask.** Give the coordinator its roster: the `list`/tree result (or a line in its turn context) should include archived workers with name, kind and last report, so "what have you run" is answerable. Nothing on the desktop is needed; the panel already lists them.

## F-58 — re-spawn with a used name fails the turn (cycle 14, l11)

**Seen.** Spawning a worker under a name already used earlier in the project failed the whole turn on a worktree branch-name collision.

**Ask.** A used name should get a suffix (`-2`) or a clear refusal the coordinator can retry; a branch collision inside the spawn should never end the user's turn.

## F-71 — headings pushed through the `plan` tool (cycle 16, long-form d6)

**Seen.** The model wrote section titles as checklist items, so `notes.md` reads `- [ ] ## Goal` and the page drew "## Goal" behind a hollow checkbox (`cycle-16/f71-headings-as-items-before.png`).

**Ask.** The `plan` tool should lift an item whose text starts with `#` into a heading of the page, or refuse it ("headings are not items; write them to notes.md directly").

**Desktop, done.** #294 reads an item whose text is `#…` as a heading (`…-after.png`), so the page is right whichever way the kernel goes.

## F-72 — the kickoff item never ticks (cycle 16, long-form)

**Seen.** `Kickoff — ready; waiting for the first ask` stays at the top of the project page after ten turns.

**Ask.** Tick or remove the kickoff item once the first user turn lands. Nothing on the desktop.

## F-80 — Gemini's tool call as a Python code block (cycle 16, kickoff)

**Seen.** As the kickoff's first reply Gemini wrote its tool call as prose: a fenced ```` ```python ```` block containing `print(default_api.bash(command='ls -la…'))`. The markup cut of #278/#279 knows the XML families (`<function_calls>`, `<invoke>`, `<tool_call>`, `<function=`) and the markers (`<|python_tag|>`, `[TOOL_CALLS]`), not this one.

**Ask.** `strip_tool_markup` should take the `default_api.<tool>(…)` code-block form too (a fenced block whose only statements call `default_api.` or `print(default_api.`). Name the family in the reply and the desktop's `markup::strip_live` will mirror it for the live stream.

## F-91 — the coordinator ran `gh auth login` and hung its turn (journey run 4, 2026-09-16)

**Seen.** A worker reported it could not open a PR without GitHub auth. The coordinator then ran `gh auth login` itself through `bash` — an interactive prompt — and its turn sat on `Running 1 command` for the rest of the run, seven minutes and counting (`media/cursor-reference/cycle-17/journey-r4-j08-gh-auth-login-hang.png`).

**Ask.** A `bash` call must not wait forever on a TTY: give the command no stdin (or `</dev/null`) so an interactive prompt fails at once, and time the call out with a line the model can act on ("`gh auth login` needs a terminal; ask the user to run it"). Cursor's coordinator never runs a command that waits on a keyboard.

**Desktop, done.** The stall clock and hint (F-77, #299) already show a silent turn honestly; the journey harness puts the gate's stand-in `gh` on PATH so the run measures the app, not GitHub.

## Not re-filed (shipped, or not the kernel's)

F-19/F-20 (#236), F-37/F-43/F-46 (#244, #245), F-10 (#225), the readonly marker (#285/#288), notifications (#293/#297), `step` on deltas (#247/#252), the settled `thinking` record (#221), `put` with bytes (#270/#273). The keyless first line has its own kernel note (#312, `2026-09-16-keyless-first-line-kernel-half.md`); the desktop half is the layout worker's and is in the ledger as F-84.

## Source

Every row above is in `internal/symmetry-findings.md` with its still under `media/cursor-reference/cycle-N/`. Written to `/tmp` and copied in; mirrored by the QA loop's `internal/` mirror.
