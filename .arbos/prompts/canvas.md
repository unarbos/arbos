---
description: Build a presentational canvas (standalone HTML page) of work done
argument-hint: [what to present; default = this session's work]
---

Create a presentational canvas of the work described below. If the description is empty, present the work done in this conversation so far.

<subject>
$ARGUMENTS
</subject>

## What a canvas is

A single self-contained HTML file — inline CSS, inline JS, no external requests, no build step — that presents the work the way a good internal demo page would. It must open cleanly with `open <file>` in a browser.

## Gather first

Collect the real material before writing any HTML: the diff or files touched, commands run and their results, decisions made and why, numbers worth charting (counts, durations, before/after). Pull from the actual repo state — never invent data.

## Design — the canvas IS part of the arbos UI

A canvas renders inside the arbos panel and must look like the app, not like a separate artifact. The panel injects the user's active theme as `--color-*` custom properties when it renders, so **never invent a palette**: write every color as `var(--color-token, fallback)` using exactly these tokens (fallbacks below are the default theme, used only when the file is opened standalone):

| token | use | fallback |
|---|---|---|
| `--color-canvas` | page background | `#2c262a` |
| `--color-card` | section cards | `#332a2f` |
| `--color-panel` | nested surfaces (stat tiles, table headers) | `#312b2f` |
| `--color-bar` | code block background | `#262024` |
| `--color-line` | all borders (1px hairlines) | `#3e373c` |
| `--color-text` | body prose | `#d6d0d4` |
| `--color-bright` | headings, emphasis, big numbers | `#ece7ea` |
| `--color-muted` | secondary text, captions, axis labels | `#968f94` |
| `--color-faint` | hints, finest print | `#6e676c` |
| `--color-accent` | links, primary chart series, section headers | `#85aab3` |
| `--color-green` / `--color-red` / `--color-warn` | positive / negative / caution series | `#87a883` / `#c97f77` / `#c9a86a` |
| `--font-sans` / `--font-mono` | prose / code+numbers | system stacks |

Rules that keep it native:

- `body { background: var(--color-canvas, #2c262a); color: var(--color-text, #d6d0d4); font: 13px/1.6 var(--font-sans, -apple-system, "Segoe UI", sans-serif); }` — the app is 13px/1.6; don't restyle the baseline.
- Cards exactly like the app's: `background: var(--color-card,…); border: 1px solid var(--color-line,…); border-radius: 8px;`. No shadows, no gradients.
- The user may run a **light** theme: never hardcode white/black or assume darkness; with the tokens above, both just work.
- SVG chart fills use the token colors (`fill="var(--color-accent, #85aab3)"`), labels `var(--color-muted,…)` at 11px `var(--font-mono,…)`.
- Top: title (`--color-bright`), one-sentence summary (`--color-muted`), date in mono.
- Sections as cards: **What changed** (per-area bullets with file paths), **How it works** (short prose + code excerpts in `<pre>` blocks with the path as a caption), **Verification** (commands and observed results), **Numbers** (if any — render small inline SVG bar/line charts by hand; no chart libraries), **Open items**.
- Code excerpts short and real — 5–15 line slices of the actual code, not pseudocode.
- Restrained polish only: accent used sparingly, subtle hover states. No animation noise, no emoji, no lorem ipsum.

## Deliver

Write the file to `canvas/<short-slug>-<yyyy-mm-dd>.html` (create the directory if needed). Then call the `show` tool on the file (title = the page title) so it opens in a panel beside the chat. Finally reply with a one-line description and the section list — nothing more. Do not paste the HTML into the chat, and do not tell the user to open the file.
