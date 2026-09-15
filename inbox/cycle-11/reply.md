# Reply — Mac captures for symmetry cycle 11 (from the Mac worker, bc-b337f0b1)

Done on Jacob's MacBook (macOS 26.5.2, 2x). Build: `cursor/mac-fixes-2-aa39` head `e081523` (#226 was still open). Everything is on this branch under `media/mac/cycle-11/`, 2880×1800 px each (window 1440×900 pt, whole window, no downscale):

- `01-first-launch.png`, `02-kickoff.png`, `02b-kickoff-after-done.png`, `02c-kickoff-repo.png`
- `03-opener-tilde.png`, `04-opener-code.png`, `05-opener-create.png`, `05b-opener-create-opened.png`
- `06-prompt-bubble.png`, `07-working-live.png`, `08-ran-card.png`
- `09-project-page.png`, `09b-after-escape.png`, `10-panel-rows.png`, `10b-panel-rows-after.png`
- `11-approval.png`, `11b-approval-ran.png`, `12-reopen.png`, `13-light.png`, `14-small-window.png`
- `notes.md` — one line per capture plus the cap-height table

Cap heights at 2x: prose `T` 21 px, "Explored" `E` 21, "Worked" `W` 22, "Running"/"Ran" `R` 21, "2 Working" `2` 21, bubble `w` x-height 16 (no capital in the bubble), Working-card title `W` 18. So bubble, prose, Worked and run lines are one size (14 pt); the card title is a step smaller.

Findings worth a look (details in notes.md):
1. `11`: in `ask` mode the bash approval shows as a **Question card** (allow / deny / Other…, Skip · Continue) with `session.permission` null, not the Skip · Run ↵ card; **Enter skips** rather than runs.
2. `07`: no "Working <step>" shimmer line and no "Running 1 command" — only "Running <description>".
3. `14`: at 900×600 the right panel does not fold.
4. `02`: the first open of a folder shows the tab sheet (name/glyph/colour) over the new-Project header.
5. `01`: a saved off-screen frame cannot be reproduced — this build persists no window frame, so it always centres.
6. The `mode: ask — …` notice is a single unwrapped line running off the right edge.
7. Opener: `~/…` paths are created relative to the app's `$HOME` (expected; noting it because the scratch run used `HOME=~/arbos-fresh/home`).

Deviations from the script: `04` used `~/arbos-fresh/places/de` instead of `~/Code` (not present in the scratch home); `10` needed an explicit "use the spawn tool" prompt (gpt-4.1-mini otherwise did the work itself); `11` used `Run: touch hello.txt && ls -la` because `ls -la` is read-only and never asks.
