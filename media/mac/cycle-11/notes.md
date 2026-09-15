# Mac captures — symmetry cycle 11

Build: `cursor/mac-fixes-2-aa39` head `e081523` (#226, on #225 `f1c850b`), `make bundle`, ad-hoc signed, run as a scratch copy from `~/arbos-fresh/Arbos.app` with `HOME=~/arbos-fresh/home` so the home place, state and config are isolated from Jacob's. Mac: MacBook Pro, macOS 26.5.2, 3456×2234 @2x (1728×1117 pt). Window 1440×900 pt at (144,100) → every capture is the whole window at 2880×1800 px (2x, no downscale). Dark theme unless noted. Model `openai/gpt-4.1-mini` via OpenRouter.

| capture | note |
| --- | --- |
| `01-first-launch` | Fresh HOME: window centred at the default 1100×761 (314,178), then set to 1440×900. Permissions sheet open on first run. Rows read Microphone Granted, Screen Recording Granted, Accessibility Not asked, Notifications Denied, Files Granted — grants are per bundle id, so a truly new user would see Not asked on the first two. The "restored off-screen" case could not be produced: this build does not persist a window frame (no `NSWindow Frame` default, nothing in state.toml), so every launch is centred. |
| `02-kickoff` | ⌘T + `~/arbos-fresh/places/demo` + Enter opened a **new empty folder** under the scratch `~` (see 05) and landed on the new-Project view with the **tab sheet (name/glyph/colour, Cancel · Done) open over the header**. Header line "Tracks the decisions, agent work, and follow-through needed to move demo forward." is partly under the sheet. |
| `02b-kickoff-after-done` | Same view after Done: glyph, "demo", one line, View Project Page, greeting "demo is ready. Drag in files…", composer "What are you working on?", no Changes/branch pill (not a repo). |
| `02c-kickoff-repo` | The real git demo repo opened by absolute path: same header block, `main` pill in the under-composer row. |
| `03-opener-tilde` | ⌘T, `~`: dropdown "This Mac", rows `~` and `arbos-fresh/` (the scratch home has one visible folder). |
| `04-opener-code` | Continued `/arbos-fresh/places/de`: children filtered to `demo/`. (`~/Code` does not exist in the scratch home; used the places folder.) |
| `05-opener-create` | `~/arbos-fresh/places/newone`: one row "Create …". Enter created and opened it — under the scratch `~`, i.e. `~/arbos-fresh/home/arbos-fresh/places/newone`. `05b` is the opened new place. |
| `06-prompt-bubble` | "what is in this repo": bubble right edge flush with the composer's right edge; "Worked 8s ⌄", "Explored README.md, 1 search", reply, footer "1m ago". |
| `07-working-live` | Captured 4.2 s after the tool card appeared (10 s from send): **"Running Run a loop printing steps with delay"** line under the bubble, stop button in the composer, "1 working" in the panel. No separate "Working <step>" shimmer line and no "Running 1 command" wording on this Mac. |
| `08-ran-card` | After the turn: "Worked 17s ⌄", "Ran Run a loop printing steps with delay", output lines step 1…6, "Just now". |
| `09-project-page` | Panel project header click → Project page (`showing: project`); Escape returns to the chat (`09b`). |
| `10-panel-rows` | Mid-run with two spawned workers: "2 Working agent-B · Editing sets-definition.md / Working agent-A · Reading notes.md", Working card (Stop All ×) over the "Working 2" pill, panel rows "agent-B — Editing sets-definition.md", "3 working". `10b` after: "Done agent-B / Done agent-A", panel "2 archived". The first attempt with the script's own prompt did not spawn (gpt-4.1-mini ran two commands itself); the capture uses an explicit "use the spawn tool" prompt. |
| `11-approval` | `/mode ask` then `Run: touch hello.txt && ls -la` (the script's `Run ls -la` is read-only and never asks). The approval renders as a **Question card** — "allow bash: touch hello.txt", options A allow / B deny / C Other…, buttons **Skip · Continue** — not the Skip · Run ↵ card; `state.session.permission` stays null. **Enter skipped it** (`11b`: "Question · allow bash: touch hello.txt · skipped", the file was not created). |
| `12-reopen` | Quit + relaunch on demo: date line, "Worked 8s ›", "Ran …", "Nm ago" — reads like the live root. |
| `13-light` | Settings › Appearance › Light (appearance-1): bubbles, lines and panel in light. Back to dark afterwards. |
| `14-small-window` | 900×600: composer and pills fit; **the right panel does not fold** — it keeps its 280 pt and the transcript column is squeezed to ~620 pt. |

Other things seen: the `mode: ask — …` notice is one long line that runs off the right edge of the transcript (no wrap) in 11/12/13/14; the Permissions sheet's Notifications row reads Denied from a fresh state on an ad-hoc build.

## Cap heights at 2x (pixels of ink, antialiased rows included)

Measured on the 2880×1800 captures with a luminance threshold; the reference is 14 px = 28 px em at 2x, SF cap height ≈ 0.70 em ≈ 20 px.

| line | glyph | ink height @2x | line ink rows (incl. descenders) |
| --- | --- | --- | --- |
| bubble text "what is in this repo" (06) | `w` (x-height; the bubble has no capital) | 16 px | 27 px |
| reply prose "This repository…" (06) | `T` | 21 px | 28 px |
| "Explored README.md, 1 search" (06) | `E` | 21 px | 27 px |
| "Worked 8s" (06) / "Worked 17s" (08) | `W` | 22 px | 23 px |
| "Running Run a loop…" (07) | `R` | 21 px | 27 px |
| "Ran Run a loop…" (08) | `R` | 21 px | 27 px |
| "2 Working agent-B…" (10) | `2` | 21 px | 27 px |
| Working card title "Working" (10) | `W` | 18 px | 24 px |

Reading: prose, the Worked line, the run lines and the "N Working" line all sit at the same size (21 px caps = 14 pt); the bubble's x-height 16 px is consistent with the same 14 pt; the Working card's title is one step smaller (~12 pt). No "Planning next moves" line was on screen in any capture.
