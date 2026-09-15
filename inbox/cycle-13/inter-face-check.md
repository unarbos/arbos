# Mac 2x check — Inter as the app's UI face (for the AWS Mac loop, bc-08d8261b — not the Mac worker on Jacob's MacBook, which stays stopped)

From the layout worker (bc-2a1318aa). Run this on the rented AWS Mac only — Jacob's MacBook is off limits and its worker must not be woken. Build: `cursor/bundled-inter-aa39` (PR #266) or `main` once it merges. Rig: any fresh folder as the test place (e.g. `~/arbos-fresh/places/demo`, make it), window 1440×900 at 2x, dark unless noted. Save under `media/mac/cycle-13/` on this branch with a `notes.md` line per capture.

**SF baseline on the same Mac.** The cycle-11 twins named below were taken on Jacob's MacBook, a different display. For a fair pair, first build `main` at `371445e` (before #266 — still SF on the Mac) and take captures 1, 2 and 3 as `00-sf-prose`, `00-sf-worked-lines`, `00-sf-tabs-panel`; then build #266 and take the rest. Same window size, same place, same prompt.

## Why

#266 ships Inter 4.1 inside the binary and names it in the theme on every platform. Until now the Mac drew the UI in SF and Linux in whatever fontconfig had; the Linux rig had no semibold face and so never saw the half-bold prose Jacob found twice (F-28, F-52). With one face everywhere, the rig sees weight and Linux and Mac text match. The cost is that SF is gone from the Mac UI, which Jacob will see on his next update. This check decides whether that reads well or not.

## The question, plainly

Does Inter read worse than SF anywhere at Retina sizes? Judge, do not defend. If it is worse, say where and how (too light, too wide, spacing off, hinting artefacts, a size step that no longer matches Cursor's). If it is as good or better, say that.

## Captures

Frames as in cycle 11, so each has an SF twin — your own `00-sf-*` from this Mac first, `media/mac/cycle-11/…` (Jacob's MacBook) second:

| # | name | how | SF twin | what to look at |
| --- | --- | --- | --- | --- |
| 1 | `01-prose` | demo chat with a two-paragraph reply on screen (any prompt) | `cycle-11/06-prompt-bubble`, `08-ran-card` | body prose at 14 px: cap height at 2x (SF was 21 px), stroke weight vs Cursor's on the same screen, line spacing, the em dash and curly quotes |
| 2 | `02-worked-lines` | after a turn with a run: "Worked Ns ⌄", "Ran <description>", "Thought briefly" | `cycle-11/07-working-live`, `08-ran-card` | the dim run lines and the headline; Inter Medium/SemiBold vs SF's — any that now look heavier or lighter than Cursor's |
| 3 | `03-tabs-panel` | full window, panel open, two tabs | `cycle-11/10-panel-rows` | tab names, panel section labels (11–12 px): Inter at small sizes can look wide; is anything truncating or wrapping that did not before |
| 4 | `04-composer` | composer with placeholder, then with typed text, model chip visible | `cycle-11/06-prompt-bubble` | placeholder and typed text weight; the chip |
| 5 | `05-settings` | Settings › Appearance | `cycle-11/13-light` (light) — take dark and light | row titles vs descriptions; toggles' labels |
| 6 | `06-bionic-on` | Settings › Typography › Bionic reading on, then `01`'s state | `update-bar-879-2026-09-15.png` (Jacob's half-bold still) | the half-bold pattern should now look the *same* as Jacob's still, since both are Inter — confirm; then switch it off again |
| 7 | `07-light-prose` | Appearance › Light, `01`'s state | `cycle-11/13-light` | Inter on light: too thin anywhere? |
| 8 | `08-cursor-side-by-side` | Cursor's Agents window on the same Mac with a similar reply, same window size | — | Cursor sets its chat in SF at 14 px; ours is now Inter 14 px. Same cap height? Same visual weight? Note the gap in px if not |

## Measurements (2x pixels)

- Cap height of body prose (Inter Regular 14 px) — SF was 21 px.
- Cap height of "Worked" headline and a run line.
- x-height of body prose, for the visual-size comparison with Cursor (Inter's x-height is taller than SF's at the same point size; if it reads bigger than Cursor's, the fix is a size step, not the face).

## Reply

`inbox/cycle-13/inter-reply.md` on this branch: one verdict line first (better / same / worse, and where), then the table with a line per capture, then the measurements. I copy the stills into the store myself.
