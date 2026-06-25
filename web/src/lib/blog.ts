/**
 * Blog model: a blog is a self-contained interactive HTML document (ADR-0039)
 * and the HTML is the single source of truth. This module is the bridge that
 * lets a WYSIWYG editor edit that HTML safely:
 *
 *  - parseBlog() reads the canonical file into a structured model: the head
 *    (styles), a cover, a title, a byline, and the editable article body. The
 *    body's interactive *islands* (agent-authored blocks with their own inline
 *    <script>) are lifted out and replaced with inert placeholders, and the raw
 *    markup is stashed verbatim so it round-trips untouched.
 *  - serializeBlog() puts the edited title/cover/body back, restores the islands
 *    from the stash, and re-emits the whole document — head, styles, and scripts
 *    preserved. The editor only ever owns the prose; vibe-coding owns the rest.
 *
 * The edit surface renders the body in a *same-origin* iframe so the blog's own
 * <style> applies with full fidelity (isolated from the app chrome) and a
 * <base> resolves the file's relative assets — but with every <script> and
 * island removed, nothing agent-authored executes while editing. Live JS only
 * ever runs in the sandboxed Preview/Share render (themedCanvasDoc).
 */

import DOMPurify from "dompurify";
import katex from "katex";
// The KaTeX stylesheet as a string, inlined into each math island so the
// rendered formula displays in the sandboxed/shared Preview (null origin, no
// network) without a runtime KaTeX dependency.
import katexCss from "katex/dist/katex.min.css?inline";

import { rawUrl } from "./surface";

/** The directory holding a blog file (workspace-relative, no trailing slash). */
export function blogDir(path: string): string {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(0, i) : "";
}

/** A blog's slug — its filename without directory or extension. */
export function blogSlug(path: string): string {
  const base = path.slice(path.lastIndexOf("/") + 1);
  return base.replace(/\.html?$/i, "");
}

/** The asset path for an uploaded image, e.g. blog/assets/<slug>/<name>. */
export function blogAssetPath(blogPath: string, name: string): string {
  return `${blogDir(blogPath)}/assets/${blogSlug(blogPath)}/${name}`;
}

/** The src an inserted asset gets *inside* the document (relative to the file,
 *  so it stays portable when shared). Mirrors blogAssetPath's layout. */
export function blogAssetSrc(blogPath: string, name: string): string {
  return `assets/${blogSlug(blogPath)}/${name}`;
}

/** A display URL for a document-relative asset src, resolved through /raw so it
 *  loads in the edit iframe and the app. Absolute/data/remote srcs pass through. */
export function assetDisplayUrl(blogPath: string, src: string): string {
  if (/^(https?:|data:|blob:|\/)/i.test(src)) return src;
  return rawUrl(`${blogDir(blogPath)}/${src.replace(/^\.\//, "")}`);
}

export interface BlogModel {
  /** The canonical document (head + scripts intact) — the basis for save. */
  doc: Document;
  /** The blog file's workspace-relative path. */
  path: string;
  /** Head markup (styles/meta) for the edit iframe, with scripts/base removed. */
  editHead: string;
  title: string;
  /** Document-relative cover image src, or null for the default banner. */
  coverSrc: string | null;
  /** Editable byline text (plain). */
  byline: string;
  /** Sanitized body HTML with islands swapped for placeholders. */
  bodyHtml: string;
  /** placeholder id -> the island's verbatim outerHTML. */
  islands: Map<string, string>;
}

const ISLAND_SELECTOR =
  "[data-island], figure.island, .island";

function purify(html: string): string {
  // Strip scripts and event handlers from prose; keep data-* (placeholders),
  // structural tags, links and images. Islands are restored raw afterwards.
  return DOMPurify.sanitize(html, {
    USE_PROFILES: { html: true },
    ADD_ATTR: ["target", "contenteditable"],
    FORBID_TAGS: ["style"],
  });
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** The inert stand-in an island shows while editing: kind chip + export label. */
function placeholderHtml(id: string, kind: string, label: string): string {
  return (
    `<div class="island-ph" data-island-id="${escapeAttr(id)}" contenteditable="false">` +
    `<span class="island-ph-kind">${escapeText(kind || "interactive")}</span>` +
    `<span class="island-ph-label">${escapeText(label || "Interactive block — runs in Preview")}</span>` +
    `</div>`
  );
}

/**
 * Find (or synthesize) the article's parts, normalizing legacy blogs that lack
 * the canonical cover/title/byline/article-body structure into it, so saves are
 * stable and the editor has one shape to work with.
 */
function normalize(doc: Document): {
  article: HTMLElement;
  cover: HTMLElement;
  title: HTMLElement;
  byline: HTMLElement;
  body: HTMLElement;
} {
  let article = doc.querySelector("article") as HTMLElement | null;
  if (!article) {
    article = doc.createElement("article");
    article.className = "article";
    while (doc.body.firstChild) article.appendChild(doc.body.firstChild);
    doc.body.appendChild(article);
  }
  if (!article.classList.contains("article")) article.classList.add("article");

  let cover =
    (article.querySelector("[data-cover], figure.cover") as HTMLElement | null) ||
    null;
  const title =
    (article.querySelector("[data-title]") as HTMLElement | null) ||
    (article.querySelector("h1") as HTMLElement | null) ||
    doc.createElement("h1");
  if (!title.hasAttribute("data-title")) title.setAttribute("data-title", "");
  const byline =
    (article.querySelector(".byline") as HTMLElement | null) ||
    (() => {
      const p = doc.createElement("p");
      p.className = "byline";
      return p;
    })();

  let body = article.querySelector(
    "[data-editable], .article-body",
  ) as HTMLElement | null;
  if (!body) {
    body = doc.createElement("div");
    body.className = "article-body";
    body.setAttribute("data-editable", "");
    // Everything in the article that isn't cover/title/byline becomes body.
    const keep = new Set<Node>([cover as Node, title, byline].filter(Boolean));
    const moved: Node[] = [];
    for (const child of Array.from(article.childNodes)) {
      if (keep.has(child)) continue;
      moved.push(child);
    }
    for (const n of moved) body.appendChild(n);
  }
  if (!body.hasAttribute("data-editable")) body.setAttribute("data-editable", "");

  if (!cover) {
    cover = doc.createElement("figure");
    cover.className = "cover";
    cover.setAttribute("data-cover", "");
  }

  // Re-assemble the article in canonical order.
  article.replaceChildren(cover, title, byline, body);
  return { article, cover, title, byline, body };
}

/** Parse a blog HTML file into the editor model. */
export function parseBlog(html: string, path: string): BlogModel {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const { cover, title, byline, body } = normalize(doc);

  // Lift islands out of a *clone* of the body so the canonical doc keeps them.
  const islands = new Map<string, string>();
  const work = body.cloneNode(true) as HTMLElement;
  let n = 0;
  for (const el of Array.from(work.querySelectorAll(ISLAND_SELECTOR))) {
    const id = `isl-${n++}`;
    const kind = el.getAttribute("data-island") || "interactive";
    const label =
      el.getAttribute("data-export") ||
      el.querySelector("figcaption")?.textContent?.trim() ||
      "";
    islands.set(id, (el as HTMLElement).outerHTML);
    const ph = doc.createElement("div");
    ph.innerHTML = placeholderHtml(id, kind, label);
    el.replaceWith(ph.firstElementChild as Node);
  }

  const coverImg = cover.querySelector("img");
  const coverSrc = coverImg?.getAttribute("src") || null;

  // Head styles for the edit iframe, with scripts and <base>/<title> removed.
  const headClone = doc.head.cloneNode(true) as HTMLElement;
  for (const el of Array.from(
    headClone.querySelectorAll("script, base, title"),
  )) {
    el.remove();
  }

  return {
    doc,
    path,
    editHead: headClone.innerHTML,
    title: (title.textContent || "").trim(),
    coverSrc,
    byline: (byline.textContent || "").trim(),
    bodyHtml: purify(work.innerHTML),
    islands,
  };
}

/** Restore island placeholders in a body fragment to their verbatim markup. */
function restoreIslands(
  doc: Document,
  container: HTMLElement,
  islands: Map<string, string>,
): void {
  for (const ph of Array.from(
    container.querySelectorAll(".island-ph[data-island-id]"),
  )) {
    const id = ph.getAttribute("data-island-id") || "";
    const raw = islands.get(id);
    if (raw == null) {
      ph.remove();
      continue;
    }
    const tmp = doc.createElement("div");
    tmp.innerHTML = raw;
    ph.replaceWith(...Array.from(tmp.childNodes));
  }
}

export interface BlogEdits {
  title: string;
  byline: string;
  coverSrc: string | null;
  /** Body HTML as read back from the edit iframe (still holds placeholders). */
  bodyHtml: string;
}

/** Re-emit the full blog document with the user's edits folded in. */
export function serializeBlog(model: BlogModel, edits: BlogEdits): string {
  const doc = model.doc;
  const { cover, title, byline, body } = normalize(doc);

  title.textContent = edits.title;
  byline.textContent = edits.byline;

  if (edits.coverSrc) {
    cover.classList.remove("empty");
    cover.innerHTML = `<img src="${escapeAttr(edits.coverSrc)}" alt="">`;
  } else {
    cover.innerHTML = "";
  }

  body.innerHTML = purify(edits.bodyHtml);
  restoreIslands(doc, body, model.islands);

  const dt = doc.doctype ? "<!DOCTYPE html>\n" : "";
  return dt + doc.documentElement.outerHTML + "\n";
}

/**
 * Register a freshly inserted rich block (raw HTML / video / embed) as an
 * island: it is stashed verbatim and represented by a placeholder, so it is
 * preserved on save and runs live in Preview — the same contract as an
 * agent-authored island. Returns the placeholder HTML to drop into the body.
 */
export function addIsland(
  model: BlogModel,
  kind: string,
  label: string,
  rawHtml: string,
): string {
  const id = `isl-u${model.islands.size}-${Date.now().toString(36)}`;
  model.islands.set(id, rawHtml);
  return placeholderHtml(id, kind, label);
}

/** A blank editable table: rows x cols of empty cells with a header row. The
 *  markup is plain <table> (DOMPurify's html profile keeps it), so it edits
 *  inline in the contenteditable body and round-trips like any other prose. */
export function tableHtml(rows: number, cols: number): string {
  const r = Math.max(1, Math.min(rows, 20));
  const c = Math.max(1, Math.min(cols, 10));
  const cell = (tag: string) => `<${tag}><br></${tag}>`;
  const head = `<thead><tr>${Array.from({ length: c }, () => cell("th")).join("")}</tr></thead>`;
  const bodyRows = Array.from(
    { length: r - 1 },
    () => `<tr>${Array.from({ length: c }, () => cell("td")).join("")}</tr>`,
  ).join("");
  return `<table>${head}<tbody>${bodyRows}</tbody></table><p><br></p>`;
}

/**
 * Render a LaTeX source string to a self-contained math island: KaTeX is run
 * here (in the editor, where it is bundled) to static HTML, and the KaTeX
 * stylesheet is inlined into the island, so the block renders with no runtime
 * dependency in the sandboxed/shared Preview (which has no network and a null
 * origin). The raw LaTeX is kept in data-tex (round-trips, re-editable) and as
 * the placeholder/export label. display=true centers it as a block.
 */
export function mathIsland(
  model: BlogModel,
  tex: string,
  display = true,
): string | null {
  const src = tex.trim();
  if (!src) return null;
  let rendered: string;
  try {
    rendered = katex.renderToString(src, {
      displayMode: display,
      throwOnError: false,
      output: "html",
    });
  } catch {
    return null;
  }
  // The CSS is inlined once per island; duplicate <style> blocks across several
  // math islands are harmless (identical rules) and keep each island portable.
  const raw =
    `<figure class="island" data-island="math" data-tex="${escapeAttr(src)}" ` +
    `data-export="Math: ${escapeAttr(src)}" ` +
    `style="text-align:${display ? "center" : "left"}">` +
    `<style>${katexCss}</style>${rendered}</figure>`;
  return addIsland(model, "math", `Math: ${src}`, raw);
}

/** Editor-chrome CSS injected into the edit iframe (cover, placeholders). */
export const EDIT_CHROME_CSS = `
  [contenteditable]{outline:none}
  [contenteditable]:focus{outline:none}
  .cover{position:relative;display:block;width:100%;aspect-ratio:5/2;margin:0 0 1.8rem;
    border-radius:14px;overflow:hidden;background:var(--color-card,#332a2f);
    border:1px solid var(--color-line,#3e373c)}
  .cover img{width:100%;height:100%;object-fit:cover;display:block}
  .cover:empty,.cover.empty{display:flex;align-items:center;justify-content:center}
  .cover:empty::after,.cover.empty::after{
    content:"No cover image — use the toolbar to add one (5:2 works best)";
    font-family:var(--font-sans,sans-serif);font-size:.82rem;color:var(--color-faint,#6e676c);
    padding:0 1rem;text-align:center}
  [data-title]:empty::before{content:"Title";color:var(--color-faint,#6e676c)}
  .byline:empty::before{content:"Author / subtitle";color:var(--color-faint,#6e676c)}
  [data-editable]:empty::before{content:"Write your story\\2026";color:var(--color-faint,#6e676c)}
  .island-ph{display:flex;flex-direction:column;gap:.25rem;margin:1.8rem 0;padding:1rem 1.1rem;
    border:1px dashed var(--color-line,#3e373c);border-radius:12px;background:var(--color-card,#332a2f);
    font-family:var(--font-sans,sans-serif);cursor:default;user-select:none}
  .island-ph-kind{font-size:.66rem;letter-spacing:.06em;text-transform:uppercase;
    color:var(--color-accent,#85aab3);font-weight:600}
  .island-ph-label{font-size:.85rem;color:var(--color-muted,#968f94)}
`;

/** A starter blog document for "make me a blog" when none exists yet. */
export function defaultBlogTemplate(title = "", byline = ""): string {
  const t = escapeText(title);
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${t || "Untitled blog"}</title>
<style>
  :root{
    --canvas: var(--color-canvas, #2c262a);
    --card: var(--color-card, #332a2f);
    --panel: var(--color-panel, #312b2f);
    --bar: var(--color-bar, #262024);
    --line: var(--color-line, #3e373c);
    --text: var(--color-text, #d6d0d4);
    --bright: var(--color-bright, #ece7ea);
    --muted: var(--color-muted, #968f94);
    --faint: var(--color-faint, #6e676c);
    --accent: var(--color-accent, #85aab3);
    --serif: var(--font-serif, "Iowan Old Style",Georgia,"Times New Roman",serif);
    --sans: var(--font-sans, -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif);
    --mono: var(--font-mono, ui-monospace,SFMono-Regular,Menlo,Consolas,monospace);
  }
  *{box-sizing:border-box}
  html,body{margin:0;padding:0;background:var(--canvas);color:var(--text)}
  body{font-family:var(--serif);font-size:18px;line-height:1.7;
    font-feature-settings:"kern" 1,"liga" 1,"onum" 1,"calt" 1;
    text-rendering:optimizeLegibility;-webkit-font-smoothing:antialiased;
    padding:2.6rem 1.25rem 6rem}
  .article{max-width:38rem;margin:0 auto}
  .cover{display:block;width:100%;aspect-ratio:5/2;margin:0 0 1.8rem;border-radius:14px;
    overflow:hidden;background:linear-gradient(135deg,var(--card),var(--panel))}
  .cover img{width:100%;height:100%;object-fit:cover;display:block}
  h1,h2,h3{font-family:var(--serif);color:var(--bright);font-weight:600}
  h1{font-size:clamp(1.9rem,1.5rem + 1.4vw,2.5rem);line-height:1.15;text-wrap:balance;margin:0 0 .5rem}
  h2{font-size:1.4em;line-height:1.25;margin:2.2em 0 .55em;text-wrap:balance}
  h3{font-size:1.18em;margin:1.8em 0 .4em}
  p{margin:0 0 1.15em}
  .lede{font-size:1.18em;line-height:1.55;color:var(--muted);margin:0 0 1.6em}
  .byline{font-family:var(--sans);font-size:.82rem;letter-spacing:.02em;color:var(--faint);
    text-transform:uppercase;margin:0 0 2.2rem;border-bottom:1px solid var(--line);padding-bottom:1.2rem}
  a{color:var(--accent);text-decoration:underline;text-underline-offset:2px}
  strong,b{color:var(--bright);font-weight:600}
  code{font-family:var(--mono);font-size:.86em;background:var(--bar);padding:.08em .35em;border-radius:4px}
  pre{background:var(--bar);border:1px solid var(--line);border-radius:9px;padding:1rem 1.1rem;
    overflow-x:auto}
  pre code{background:none;padding:0}
  blockquote{margin:1.5em 0;padding:.2em 0 .2em 1.1em;border-left:3px solid var(--line);
    color:var(--muted);font-size:1.06em}
  ul,ol{margin:0 0 1.2em;padding-left:1.4em}
  li{margin:0 0 .5em}
  img{max-width:100%;height:auto;border-radius:10px}
  hr{border:none;border-top:1px solid var(--line);margin:2.4em 0}
  figure{margin:1.8rem 0}
  figcaption{font-family:var(--sans);font-size:.8rem;color:var(--muted);margin-top:.5rem}
  .island{margin:2.2rem 0;padding:1.1rem;background:var(--card);border:1px solid var(--line);border-radius:12px}
</style>
</head>
<body>
<article class="article">
  <figure class="cover" data-cover></figure>
  <h1 class="title" data-title>${t}</h1>
  <p class="byline" data-byline>${escapeText(byline)}</p>
  <div class="article-body" data-editable></div>
</article>
</body>
</html>
`;
}
