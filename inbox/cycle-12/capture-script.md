# Mac capture script — symmetry cycle 12 (for the Mac worker, bc-b337f0b1)

From the layout worker (bc-2a1318aa). Thank you for cycle 11 — every finding is fixed on `main` (#228) except the approval card, which is #224 (also on `main`). Build: `main` at or after `848e003` plus `cursor/symmetry-cycle-12-aa39` when it lands (or that branch's head). Same rig as cycle 11: `~/arbos-fresh/places/demo`, 1440×900 at 2x, dark unless noted, save under `media/mac/cycle-12/` on this branch with a `notes.md` line per capture.

This cycle's theme is the class of things Jacob hit last: read-only questions, wrong-shaped replies, mid-turn steers — and the fixes from cycle 11 checked live on the Mac.

| # | name | how | what to check |
| --- | --- | --- | --- |
| 1 | `01-frame-restored` | quit; drag the window so ~60 % is off the right edge; quit; relaunch | centred on the main display (frame persisted now — F-26); then relaunch once more with the window fully on screen → it comes back where it was |
| 2 | `02-kickoff-live` | ⌘T → a brand-new empty folder under `~/arbos-fresh/places/` | "Setting up environment" shimmer (capture at +2 s), then "Worked Ns ›" + the greeting "Hey <name> —" + footer; date line above; no static greeting; the tab sheet sits below the header |
| 3 | `03-readonly-question` | in demo: `Run ls -la and tell me how many files there are.` | "Working <step> ⌄" while live, "Running 1 command" during, "Ran <description> ⌄" after; the answer one line; no narration of the command before it runs |
| 4 | `04-no-run-question` | `In one sentence, what does main.py do? Do not run anything.` | "Explored main.py" or a read line only; no bash; one sentence |
| 5 | `05-steer-mid-turn` | `Run this exact shell command and show me its output as it arrives: for i in 1 2 3 4 5 6; do echo step $i; sleep 3; done`; at +4 s type `Also print the date at the end.` and Enter | the steer bubble sits *inside* the running turn's fold — one "Worked", no second turn (cycle-12 fix); capture at +6 s and after |
| 6 | `06-ask-typed-option` | `Before doing anything, ask me one multiple-choice question with two options, alpha and beta, about which name to use for a new module. Wait for my answer.` — then type `alpha` and Enter (do not click an option) | the card folds to "Question · … · alpha"; the reply says alpha once, never "alphaalpha" (cycle-12 fix); no bare `ask "…"` row above the card |
| 7 | `07-small-window` | resize to 900×600 | the right panel folds; the transcript keeps the full width |
| 8 | `08-notice-wrap` | `/mode ask` then any prompt | the `mode: ask — …` notice wraps within the column |
| 9 | `09-approval-card` | still in ask mode: `Run: touch hello.txt && ls -la` | Cursor's approval row: Skip · Run ↵ (Always Run only if offered); Enter runs; capture before and after |
| 10 | `10-light` | Settings › Appearance › Light, then `03`'s state | colours in light; back to dark after |

`notes.md`: one line per capture plus anything odd; cap heights again for the "Working <step>" headline and the Working-card title (both should now be 21 px at 2x like the prose).
