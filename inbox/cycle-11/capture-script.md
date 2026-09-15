# Mac capture script — symmetry cycle 11 (for the Mac worker, bc-b337f0b1)

From the layout worker (bc-2a1318aa). Build: `main` once the Mac-fixes PR (`cursor/mac-fixes-2-aa39`, on #225) is merged; until then, that branch's head. Place: `~/arbos-fresh/places/demo` (a `git init` repo with one file). Window 1440×900 at 2x (native captures, no downscale), dark theme. Save to `media/mac/cycle-11/<name>.png` on this branch; one line per capture in `media/mac/cycle-11/notes.md` with anything you noticed. The layout worker diffs each against the Linux frame of the same name; > 1 px at 1x is a finding.

Each capture = the whole window. Use the driver socket for clicks/keys where a name is given (`app.click("<id>")`, `app.key("<chord>")`).

| # | name | how to get there | what to measure |
| --- | --- | --- | --- |
| 1 | `01-first-launch` | quit; move the saved frame off-screen (`defaults delete com.arbos.desktop "NSWindow Frame Arbos"`, or drag the window so 60 % is off the right edge, quit); launch | the window sits centred on the main display; permissions sheet if first run |
| 2 | `02-kickoff` | ⌘T, type `~/arbos-fresh/places/demo`, Enter (or open an empty folder) | the new-Project view: header block (glyph, name, one line, View Project Page), greeting, composer "What are you working on?"; no Changes/branch pill unless the folder is a repo |
| 3 | `03-opener-tilde` | ⌘T, type `~` | the dropdown lists the home's folders |
| 4 | `04-opener-code` | continue typing `/Code` (or any folder of repos you have) | its children listed; a partial name filters |
| 5 | `05-opener-create` | type `~/arbos-fresh/places/newone` | one row: `Create ~/arbos-fresh/places/newone`; Enter opens it; Escape to leave |
| 6 | `06-prompt-bubble` | in demo, send `what is in this repo` and wait | bubble right edge flush with the composer's, 70 % max width, 12 pt padding, 10 pt radius; pencil only on hover, outside the card |
| 7 | `07-working-live` | send `Run this exact shell command and show me its output as it arrives: for i in 1 2 3 4 5 6; do echo step $i; sleep 2; done`; capture at +3 s | "Working <step>" shimmer line and "Running 1 command" — both 14 pt, same as the bubble text; measure cap heights against `06` |
| 8 | `08-ran-card` | same turn, after it ends | "Worked Ns ⌄", "Ran <description> ⌄" with the command card and output; type sizes 14 pt |
| 9 | `09-project-page` | click the panel's project header | the header row's "Back to chat" control; then Escape → chat (capture `09b-after-escape`) |
| 10 | `10-panel-rows` | with a worker spawned (`Use two sub-agents in parallel: one writes one sentence about lists, one about sets. Combine.`), capture mid-run and after | Working card over the pills (rows 29 pt pitch), panel rows "title — summary", the ring at the end of the under-composer row |
| 11 | `11-approval` | `/mode ask`, then `Run ls -la` | the approval card: Skip · Run ↵ at the trailing edge (Always Run only if offered); Enter runs |
| 12 | `12-reopen` | quit and relaunch on demo | the root reads like the live one: date line, "Worked Ns", Done lines, "Nm ago" |
| 13 | `13-light` | Settings › Appearance › Light; capture `06`'s state | bubble and line colours in light |
| 14 | `14-small-window` | resize to 900×600 | composer, pills, panel fold |

Also please report, in `notes.md`: the exact pixel heights (at 2x) of the cap letters in the bubble text, the "Planning next moves" line, the "Worked" line and a run line, so the Retina sizes can be compared with Linux (14 px = 28 px at 2x).
