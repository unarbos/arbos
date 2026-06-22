# ADR-0039 — Interactive blog: HTML is the source, markdown is an export

- Status: Proposed
- Date: 2026-06-19
- Refines: ADR-0021 (tool result content), the `/blog` and `/canvas` prompts, and the share renderer (`internal/gateway/sharedoc.go`)

## Context

Today the agent produces two unrelated long-form artifacts, and neither is what
people now expect a shareable technical write-up to be:

- **`/blog`** writes `blog/<slug>.md` — pure markdown. It renders in the doc
  panel through the React `Markdown` parser (`web/src/components/Markdown.tsx`),
  and a share link renders a *second*, re-implemented markdown→HTML pass in Go
  (`internal/gateway/sharedoc.go`). The artifact is text: headings, prose,
  tables, code, and `[n]` citations. It cannot contain a runnable demo, a live
  chart the reader can poke, a 3D viewer, or a form — the things that make a
  modern technical post worth sharing.
- **`/canvas`** writes `canvas/<slug>.html` — a self-contained interactive HTML
  page (inline CSS/JS, no external requests) rendered in a sandboxed iframe via
  `themedCanvasDoc` (`web/src/lib/surface.ts`). It *is* interactive, but it is
  framed as an internal demo page, has no reading typography, no citation
  model, and no shareable reading link or markdown export.

The bar is set by the current generation of technical write-ups: long-form,
beautiful reading with *real interactive UI* embedded in the page — charts a
reader can poke, runnable demos, diagrams, 3D — "instead of a wall of
markdown," and shareable as a link. The artifact is an HTML document, not
decorated prose.

The synthesis the user wants: a `/blog` whose output is a **beautiful,
interactive HTML document with JavaScript** (the canvas's power) carrying
**reading-grade typography and citations** (the blog's care), that is
**shareable as a link** and **exportable to markdown**. The user phrased it as
three pulls — "just markdown," "full HTML with interactive JS," and "export to
markdown" — which only reconcile one way.

The decision is costly to reverse: it changes the *data shape* of the `/blog`
artifact (what file the agent writes and what is canonical), the rendering
contract (one renderer vs. the current two), the share path, and the tool
surface. That is exactly what ADR-0001 reserves an ADR for.

## Decision

**A blog is an interactive, self-contained HTML document. HTML is the source of
truth; markdown is a lossy, derived export. `/blog` and `/canvas` converge on
one artifact and one renderer; the existing dual markdown parser (TS + Go)
collapses to a single served HTML page.**

This inverts today's model. Today the `.md` is canonical and HTML is rendered on
the fly (twice). Henceforth the `.html` is canonical and the `.md` is generated
*from* it on demand.

### 1. The artifact: one interactive HTML document

`/blog` writes `blog/<slug>-<yyyy-mm-dd>.html`: a single self-contained file —
inline CSS, inline JS, no external network requests, no build step — same
hard constraints as a canvas today (`docs`/`canvas.md` rules carry over). It
differs from a canvas in *intent and structure*, not in mechanism:

- It is a **reading document** first: the article typography system (a reading
  serif via `--font-serif`, a narrow measure, a fluid type scale, real vertical
  rhythm) is the default body style, not an afterthought. See ADR companion work
  on the `.article` type tokens.
- It carries **semantic structure** a markdown export can recover: the document
  is authored as marked-up sections (`<article>`, headings, `<p>`, `<figure>`,
  `<blockquote>`, the citation markup already used by `sharedoc.go`), with
  interactive blocks as clearly delimited islands.
- **Interactive blocks are islands.** A runnable demo, a live chart, a 3D
  viewer, a form is a self-describing element — e.g.
  `<div class="island" data-island="chart" data-export="A bar chart of …">…</div>`
  — whose `data-export` attribute is the markdown fallback. The island owns its
  own inline `<script>`; it never reaches outside its subtree.

A blog *is* a canvas with reading typography and an export contract. The two
prompts converge: `/canvas` stays the name for "present this work as a page,"
`/blog` for "research and write a cited, interactive report," but both emit the
same artifact kind and render through the same path.

### 2. The theme contract is unchanged — and now load-bearing

The artifact is authored against arbos design tokens
(`var(--color-*, fallback)`, `--font-sans`/`--font-mono`, plus the new
`--font-serif`), exactly as canvases are today. The panel injects the active
theme at render time (`themedCanvasDoc`), so the document repaints with the app.
This is what lets an interactive document be *part of the UI* rather than a
foreign artifact, and it is non-negotiable: a blog that hardcodes a palette is
rejected by the same guidance that governs canvases.

### 3. One renderer, two surfaces, one isolation model

The HTML document renders the **same way** in both places it appears, killing
the dual-parser duplication:

- **Owner doc panel:** the existing canvas path — a sandboxed iframe
  (`sandbox="allow-scripts"`, no `allow-same-origin`) with the theme injected
  via `srcdoc`. The blog's JS runs isolated from the app origin, exactly as a
  canvas's does today.
- **Public share link:** the file is served as its **own HTML page** under the
  share route, in its own opaque origin (the existing standalone-share posture
  in `sharedoc.go`). Because the artifact is already self-contained HTML, the
  share path becomes "serve the file with the theme `<style>` prepended,"
  **not** a re-implemented markdown→HTML pass.

`internal/gateway/sharedoc.go`'s markdown renderer and `parseDocCitations` are
**retired for the blog path**: the canonical artifact is HTML, so there is no
second parser to keep in lockstep with `Markdown.tsx`. The TS `Markdown`
component remains for *chat* (assistant messages are still markdown) and for the
markdown-source *editing* view of a doc; it is no longer the blog's renderer.

This is the unification asked for in the prior discussion: we do not merge the
two markdown parsers (they served a JS app and a no-JS guest), we **remove the
need for either on the blog path** by making the artifact HTML.

### 4. Markdown is a derived export, not the source

`export-to-markdown` walks the document's semantic markup and emits a `.md`:

- Headings, prose, lists, tables, blockquotes, code → standard markdown.
- The citation markup → the bare `[n]` + Sources list convention `/blog` already
  uses (round-trips into the existing citation tooltip/jump behavior).
- Each interactive island → its `data-export` text (a one-line description, an
  optional static image/`![]()`, or a fenced ` ```chart `/` ```mermaid ` spec
  where the island *was* one of our renderable fences). The export is **lossy by
  design**: a 3D viewer becomes "[interactive 3D model: …]" + a still. The
  contract is "a reader who only has the markdown still gets the argument," not
  "byte-perfect round-trip."

Export runs on demand (a tool / a panel action / a share-as-markdown option),
producing `blog/<slug>.md` alongside the canonical `.html`. The markdown is
never the thing the agent edits; regenerating it from the HTML is always safe.

### 5. Interactive islands: a small, safe, declarative palette

We do **not** let the agent ship arbitrary unconstrained JS as the primary path.
The first-class islands are a curated set the renderer understands and the
exporter can degrade:

- `chart` — our existing plot spec (reuse `lib/plot` / the `Chart` component's
  spec), rendered live; exports to a ` ```chart ` fence.
- `mermaid` — diagram source; exports to a ` ```mermaid ` fence.
- `runnable` — a small code+output demo (e.g. the gzip widget): code shown,
  a "Run" button, output rendered; exports to a code fence + a results note.
- `embed` — a sandboxed sub-iframe for heavier interactivity (3D, custom
  canvas), still self-contained; exports to a still image + caption.

`data-island` names the kind; `data-export` carries the fallback. Arbitrary
inline `<script>` remains *possible* (it is a canvas, after all) but un-curated
scripts export only as their `data-export` text, so the agent is steered toward
the declarative palette that survives export.

## Consequences

- The `/blog` artifact changes shape: canonical file is `blog/<slug>.html`, not
  `.md`. Existing `.md` blogs still render (chat markdown + the doc panel's
  markdown editing view remain), but new blogs are HTML-source.
- **Net subtraction on the share path:** `sharedoc.go`'s markdown renderer and
  `parseDocCitations` retire for blogs; the share route serves the
  self-contained file with the theme prepended, reusing the canvas isolation
  model. The two-parser consistency burden (TS `Markdown` vs. Go
  `renderMarkdownBody`) disappears for the blog path.
- The doc panel gains nothing new structurally: a blog renders through the
  *existing* `CanvasSurface` iframe path. `SurfaceView`'s `doc` (markdown) branch
  stays for editing `.md` sources; blogs are `canvas`-kind surfaces.
- New surface: the **export-to-markdown** walker (HTML semantic markup → md +
  island fallbacks), the **island palette** (`chart`/`mermaid`/`runnable`/
  `embed`) the renderer and exporter agree on, and updated `/blog` and
  `/canvas` prompt guidance (author an interactive HTML document against the
  tokens; mark islands with `data-island`/`data-export`).
- Typography is single-sourced via the `--font-serif` + `.article` token work
  (companion change), so the doc panel, the share page, and a blog island all
  read the same.
- Security posture is explicit and *unchanged in kind*: a blog's JS runs in the
  same `allow-scripts`, no-`allow-same-origin` sandbox a canvas does, in an
  opaque origin on the share path. The blast radius of agent-authored JS is the
  iframe, never the app or the user's cookies. This ADR does not widen it.
- Authoring cost rises: an interactive HTML document is more work for the agent
  to produce than markdown prose. Mitigated by the declarative island palette
  (the agent emits a spec, the renderer does the work) and by keeping prose the
  default — interactivity is added where it earns its keep, not everywhere.

## Phasing

- **P1 — converge the artifact and renderer.** `/blog` emits self-contained
  HTML authored against the tokens with the `.article` type system; it renders
  through the existing canvas iframe path in the doc panel. Retire the blog
  branch of `sharedoc.go`; serve the file standalone with the theme prepended.
  No islands yet beyond what canvases already do.
- **P2 — export-to-markdown.** The semantic-markup → markdown walker, with
  `data-export` island fallbacks; `blog/<slug>.md` regenerated on demand;
  citations round-trip through the existing `[n]`/Sources convention.
- **P3 — the island palette.** First-class `chart`/`mermaid`/`runnable`/`embed`
  islands the renderer understands and the exporter degrades; prompt guidance
  steering the agent toward them.
- **P4 — share polish.** A clean reading share URL for an interactive blog
  (open-graph still image from the first island/figure, "view source" and
  "download markdown" affordances on the share page).

## Alternatives rejected

- **Keep markdown canonical, render interactivity from fenced specs only.**
  Less disruptive — extend the markdown fence vocabulary (we already have
  ` ```chart `/` ```mermaid `) and render those live. But it caps the ceiling at
  "markdown plus a few known widgets": no arbitrary layout, no 3D, no custom
  interactive demo, and it keeps the two-parser duplication. The target
  artifacts are HTML documents, not decorated markdown.
- **Two separate artifacts forever (`/blog` md, `/canvas` html).** Status quo.
  Keeps the dual parser, gives blogs no interactivity and canvases no reading
  typography or export, and leaves the user maintaining two long-form tools that
  each lack half of what they want.
- **HTML canonical, no markdown export.** Simpler, but loses portability: a blog
  should be quotable, diffable, and readable as text, and many destinations
  (issue trackers, READMEs, plain email) want markdown. The lossy export is the
  bridge, and it is cheap once the document is semantic markup.
- **Server-side render React for the share page (one true renderer in JS).**
  Would let one component render both surfaces, but arbos is a single Go binary
  with no Node runtime; SSR at share time is heavy and reintroduces a JS
  dependency on the public path. Making the artifact self-contained HTML
  achieves "one renderer" without any server-side JS.
- **Unconstrained agent JS as the primary authoring model.** Maximum power, but
  un-exportable (a markdown walker can't summarize arbitrary JS), harder to keep
  on-theme, and a larger thing to review. The curated island palette keeps the
  common cases declarative and degradable while leaving raw `<script>` available
  as the escape hatch.
```
