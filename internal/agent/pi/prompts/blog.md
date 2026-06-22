---
description: Research a question across many sources and write a cited, interactive, shareable report
argument-hint: [the question or topic to research]
---

Research the topic below the way a good analyst would, then write it up as a long-form, cited, **interactive** report — a "blog". This is deep research: plan the inquiry, gather from many real sources, reason as you go, and synthesize a structured document a reader can trust and share.

<topic>
$ARGUMENTS
</topic>

If the topic block is empty, research the subject currently under discussion in this conversation; if there is none, ask for one in a single line and stop.

## What a blog is

A single **self-contained interactive HTML document** written to `blog/<short-slug>-<yyyy-mm-dd>.html` — inline CSS, inline JS, no external requests, no build step. The HTML is the source of truth (a markdown export can be generated from it later). It is a *reading document first*: beautiful long-form typography carrying prose, headings, and a **Sources** section, with **interactive blocks** (live charts, runnable demos, diagrams) embedded where they make the argument better than text would. Every non-obvious claim traces to a real source you actually read. Never invent facts, quotes, numbers, or citations — an unverifiable claim is cut, not padded.

A blog renders inside the arbos doc panel (a sandboxed, themed iframe) and shares as a standalone reading link. It is a *canvas with reading typography and citations* — same theme contract as a canvas, different intent.

## Work in three phases

### Phase 1 — Plan the inquiry

Restate the topic as a sharp research question and decompose it into the sub-questions a thorough answer must settle. Track them with the `plan` tool when available — a node per sub-question under a short-lived research root — so the inquiry is visible and steerable; otherwise keep the list in your replies. This plan is the report's skeleton: each sub-question becomes a section. If the angle materially changes the work, pick the most useful framing, note it in one line, and proceed.

### Phase 2 — Gather from real sources

Pull material from the actual world, not memory. Use `fetch` for pages and APIs and `browser` for anything that needs rendering or interaction. **Fan out:** issue parallel `delegate` calls to research independent sub-questions at once, each returning findings with source URLs — that is the engine that makes this deep rather than shallow. As you gather:

- Record findings against their sub-question (in the plan node's outcome, or notes): the claim, the source URL, the supporting quote or figure.
- Triangulate — corroborate a claim across more than one source where it matters, and note disagreement rather than smoothing it over.
- Keep a running source list with a stable number per source, so citations are stable while you write.
- Prefer primary and authoritative sources; treat a single blog or forum post as a lead to verify, not a conclusion.
- Capture the **real numbers** worth charting (counts, durations, before/after, distributions) — these become live interactive islands.

Stop gathering when the sub-questions are answered or you hit diminishing returns — not when you run out of links.

### Phase 3 — Write the interactive HTML document

Write `blog/<short-slug>-<yyyy-mm-dd>.html`. It must open cleanly with `open <file>` and look native inside the arbos panel.

**Beautiful typography is the point — get it right.** The body is a reading document, not a UI pane:

- Wrap the whole article in `<article class="article">`. Body text uses `var(--font-serif, …)` at a reading size (≈18px), `line-height: 1.7`, with a **narrow measure** (`max-width: 38rem`, centered) — never the full panel width. Turn on ligatures and oldstyle numerals: `font-feature-settings: "kern" 1, "liga" 1, "onum" 1, "calt" 1; text-rendering: optimizeLegibility;`.
- A **fluid type scale** with real hierarchy: the `<h1>` is display-scale (`clamp(1.9rem, 1.5rem + 1.4vw, 2.5rem)`, `line-height: 1.15`, `text-wrap: balance`), `<h2>` ≈ 1.4em, `<h3>` ≈ 1.18em. Headings in `var(--font-serif, …)`, color `var(--color-bright, …)`.
- **Vertical rhythm**, not flat spacing: space *between* paragraphs (`margin: 0 0 1.15em`), more space *before* a heading than after it, and a distinct **lede** — the opening standfirst paragraph larger (`≈1.18em`) and in `var(--color-muted, …)`.
- Links underlined on the baseline in `var(--color-accent, …)`; `<blockquote>` set off with a left rule and slightly larger; `<code>` in `var(--font-mono, …)` on a faint tint.

**Theme contract — never invent a palette.** The panel injects the user's active theme as `--color-*` custom properties at render time, so write every color as `var(--color-token, fallback)` using exactly these tokens (fallbacks are the default theme, used only when opened standalone):

| token | use | fallback |
|---|---|---|
| `--color-canvas` | page background | `#2c262a` |
| `--color-card` | section cards, island frames | `#332a2f` |
| `--color-panel` | nested surfaces | `#312b2f` |
| `--color-bar` | code block background | `#262024` |
| `--color-line` | borders (1px hairlines) | `#3e373c` |
| `--color-text` | body prose | `#d6d0d4` |
| `--color-bright` | headings, emphasis | `#ece7ea` |
| `--color-muted` | lede, captions, axis labels | `#968f94` |
| `--color-faint` | finest print | `#6e676c` |
| `--color-accent` | links, primary chart series | `#85aab3` |
| `--color-green` / `--color-red` / `--color-warn` | positive / negative / caution | `#87a883` / `#c97f77` / `#c9a86a` |
| `--font-serif` | article body + headings | system serif stack |
| `--font-sans` | UI chrome, table text, island labels | system sans |
| `--font-mono` | code, numbers, axis labels | system mono |

The user may run a **light** theme: never hardcode white/black or assume darkness. With the tokens above, both just work.

### Document shape

1. **Title** (`<h1>`) and a one-paragraph **lede** (`<p class="lede">`) stating the question and the headline finding.
2. **Summary** — 3–6 takeaways a busy reader can lift on their own (a `<ul>`).
3. **Body sections** (`<h2>` per sub-question), prose first, with interactive islands, tables, or short lists where they carry the point. Synthesize across sources; do not stitch quotes. Surface uncertainty explicitly.
4. **Sources** — a numbered list (`<ol>`), each entry: title, publisher/author, URL, and date if known.

Cite inline with a superscript link and give the cited claim a stable id so the source can point back at it:

```html
<!-- inline, on the claim -->
Gandalf proved it<a class="cite" id="cite-ref-1" href="#cite-src-1">[1]</a>.
<!-- in the Sources list -->
<li id="cite-src-1" class="src">Title — Publisher, https://example.com <a class="backref" href="#cite-ref-1" title="Back to where this is cited">↩</a></li>
```

Each citation marker is `<a class="cite" id="cite-ref-N" href="#cite-src-N">[N]</a>` and each source is `<li id="cite-src-N" class="src">… <a class="backref" href="#cite-ref-N">↩</a></li>`. (If a source is cited from several places, the back-link points at the first; that is fine.)

Just use the `href` — **you do not need to write any JavaScript for the cite links to scroll.** A blog renders in a sandboxed null-origin iframe where a bare `href="#id"` would normally 404 instead of jumping, but the arbos renderer injects an in-page anchor-scroll handler for every blog (both in the doc panel and on the public share link), so a click on any `[N]` marker or `↩` back-link smoothly scrolls to its target. Author the `href`/`id` markup above and the scrolling just works; do not add your own click-interception script.

Style a cited source's `:target` so the jump is visible (e.g. a brief highlight), and make `.backref` quiet (`var(--color-faint)`, `var(--color-accent)` on hover). Place a citation on the specific claim it supports — not vaguely at a paragraph's end. Every figure, quote, and contested claim carries one. Restrained, factual voice; no filler, no emoji.

### Interactive islands — where a blog beats a markdown report

An **island** is a self-contained interactive block that earns its place by making the argument clearer than prose. Mark each one so it survives a later markdown export:

```html
<figure class="island" data-island="chart" data-export="Bar chart: failure rate by tool — Devin 70%, Aider 20%, Arbos n/a.">
  <!-- inline SVG or <canvas> + a small inline <script>, drawn with token colors -->
  <figcaption>Independent failure rates across tools [7].</figcaption>
</figure>
```

- `data-island` names the kind: **`chart`** (hand-drawn inline SVG/canvas, token colors, no chart libraries), **`runnable`** (a short code sample shown with a "Run" button that computes and renders its own output), **`diagram`** (an inline SVG diagram), **`embed`** (a heavier custom interactive — 3D, simulation — still inline).
- `data-export` is the one-line fallback a markdown export uses in place of the live block; for a `chart` or `diagram` it should describe what the figure shows, for a `runnable` it summarizes the demo and result. Always provide it.
- Islands own their own inline `<script>` and never reach outside their subtree. Use real gathered numbers — never invent data to fill a chart.
- Restrained polish: accent used sparingly, subtle hover, no animation noise. An island with no real material is omitted, not padded.

## Deliver

Write `blog/<short-slug>-<yyyy-mm-dd>.html` (create the directory if needed). Then call the `show` tool on the file (title = the report title) so it opens in a panel beside the chat — `.html` opens as a themed, sandboxed reading surface. If `show` is unavailable, reply with the file path instead. Finally reply with a one-line description and the section list — nothing more. Do not paste the HTML into the chat, and when `show` succeeded do not tell the user to open the file. The report shares from its panel as a standalone reading link, and can be exported to markdown.
