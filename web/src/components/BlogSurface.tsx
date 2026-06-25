import { useCallback, useEffect, useRef, useState } from "react";
import {
  Bold,
  ChevronDown,
  Code,
  ExternalLink,
  Eye,
  Image as ImageIcon,
  Italic,
  Link as LinkIcon,
  List,
  ListOrdered,
  Loader2,
  Minus,
  PenLine,
  Quote,
  RotateCw,
  Sigma,
  Smile,
  Strikethrough,
  Table as TableIcon,
  Video,
} from "lucide-react";

import { fetchFile, HttpError, writeFile, writeFileBase64 } from "@/lib/api";
import {
  addIsland,
  blogAssetPath,
  blogAssetSrc,
  blogDir,
  EDIT_CHROME_CSS,
  mathIsland,
  parseBlog,
  serializeBlog,
  tableHtml,
  type BlogModel,
} from "@/lib/blog";
import { rawUrl, surfaceTitle, themedCanvasDoc, type Surface } from "@/lib/surface";
import { useTheme } from "@/lib/theme";
import type { Theme } from "@/lib/themes";
import { useDocumentVisible } from "@/lib/useDocumentVisible";

const STAT_POLL_MS = 2000;
const AUTOSAVE_MS = 700;

const FONT_SANS =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, sans-serif';
const FONT_MONO =
  'ui-monospace, "SF Mono", "Cascadia Mono", "JetBrains Mono", Menlo, Consolas, monospace';
const FONT_SERIF =
  'Newsreader, Charter, "Iowan Old Style", "Source Serif 4", Georgia, Cambria, "Times New Roman", serif';

function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

function themeTokens(theme: Theme): string {
  const vars = Object.entries(theme.colors)
    .map(([t, v]) => `--color-${t}:${v};`)
    .join("");
  return (
    `:root{${vars}` +
    `--color-hover:color-mix(in srgb, var(--color-bright) 8%, transparent);` +
    `--font-sans:${FONT_SANS};--font-mono:${FONT_MONO};--font-serif:${FONT_SERIF};` +
    `color-scheme:${theme.dark ? "dark" : "light"};}`
  );
}

/**
 * The HTML for the same-origin edit iframe: the blog's own head styles (so the
 * article reads exactly as it will when published) + the active theme tokens +
 * editor chrome, over the article body with islands shown as inert placeholders
 * and nothing executable. A <base> resolves the file's relative assets.
 */
function buildEditDoc(model: BlogModel, theme: Theme): string {
  const base = `${rawUrl(blogDir(model.path))}/`;
  const cover = model.coverSrc
    ? `<figure class="cover" data-cover><img src="${escapeAttr(model.coverSrc)}" alt=""></figure>`
    : `<figure class="cover empty" data-cover></figure>`;
  return (
    `<!DOCTYPE html><html><head><meta charset="utf-8">` +
    `<base href="${escapeAttr(base)}">` +
    model.editHead +
    `<style id="arbos-theme">${themeTokens(theme)}</style>` +
    `<style id="arbos-edit">${EDIT_CHROME_CSS}</style>` +
    `</head><body><article class="article">` +
    cover +
    `<h1 class="title" data-title>${escapeText(model.title)}</h1>` +
    `<p class="byline" data-byline>${escapeText(model.byline)}</p>` +
    `<div class="article-body" data-editable>${model.bodyHtml}</div>` +
    `</article></body></html>`
  );
}

function isGone(e: unknown): boolean {
  return e instanceof HttpError && e.status === 404;
}

/**
 * A blog (ADR-0039 interactive HTML) opened as a Twitter-Articles-style editor:
 * an Edit mode that edits the article body in place with a formatting toolbar
 * (the HTML stays the single source of truth — islands and scripts are
 * preserved), and a Preview mode that renders the live, sandboxed document so
 * animations, charts, and runnable demos run. Agent vibe-coding edits the same
 * file; while the editor is clean it picks those changes up on the disk poll.
 */
export function BlogSurface({
  surface,
  active,
}: {
  surface: Surface;
  active: boolean;
}) {
  const theme = useTheme();
  const docVisible = useDocumentVisible();

  const [mode, setMode] = useState<"edit" | "preview">("edit");
  const [state, setState] = useState<"loading" | "ready" | "error">("loading");
  const [errorMsg, setErrorMsg] = useState("");
  const [missing, setMissing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [words, setWords] = useState(0);
  const [previewTick, setPreviewTick] = useState(0);
  // Bumped to rebuild the edit iframe (initial load / external reload).
  const [editKey, setEditKey] = useState(0);
  const [editDoc, setEditDoc] = useState("");

  const modelRef = useRef<BlogModel | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const coverSrcRef = useRef<string | null>(null);
  const dirtyRef = useRef(false);
  const mtimeRef = useRef(0);
  const saveTimer = useRef<number | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  // What a chosen image is for: the cover, or an inline insert at the caret.
  const imgTarget = useRef<"cover" | "body">("cover");

  const editDocRef = useRef("");
  editDocRef.current = editDoc;

  const rebuild = useCallback(
    (model: BlogModel) => {
      modelRef.current = model;
      coverSrcRef.current = model.coverSrc;
      setEditDoc(buildEditDoc(model, theme));
      setEditKey((k) => k + 1);
    },
    [theme],
  );

  // Load (and reload when the tab is re-pointed at another blog).
  useEffect(() => {
    let stale = false;
    setState("loading");
    fetchFile(surface.path)
      .then((info) => {
        if (stale) return;
        mtimeRef.current = info.mtime;
        dirtyRef.current = false;
        const model = parseBlog(info.content ?? "", surface.path);
        rebuild(model);
        setState("ready");
        setMissing(false);
      })
      .catch((e: unknown) => {
        if (stale) return;
        setState("error");
        setErrorMsg(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stale = true;
    };
    // rebuild changes with theme, but a theme switch shouldn't refetch — the
    // separate theme effect repaints the live iframe instead.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [surface.path]);

  /** Read the article back out of the edit iframe into a saveable string. */
  const collect = useCallback((): string | null => {
    const model = modelRef.current;
    const d = frameRef.current?.contentDocument;
    if (!model || !d) return null;
    const title = d.querySelector("[data-title]")?.textContent ?? "";
    const byline =
      d.querySelector("[data-byline], .byline")?.textContent ?? "";
    const bodyHtml = d.querySelector("[data-editable]")?.innerHTML ?? "";
    return serializeBlog(model, {
      title: title.trim(),
      byline: byline.trim(),
      coverSrc: coverSrcRef.current,
      bodyHtml,
    });
  }, []);

  const flush = useCallback((): Promise<void> => {
    if (!dirtyRef.current) return Promise.resolve();
    const html = collect();
    if (html == null) return Promise.resolve();
    dirtyRef.current = false;
    setSaving(true);
    setSaveError("");
    return writeFile(surface.path, html)
      .then((info) => {
        mtimeRef.current = info.mtime;
      })
      .catch((e: unknown) => {
        dirtyRef.current = true;
        setSaveError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => setSaving(false));
  }, [collect, surface.path]);

  const scheduleSave = useCallback(() => {
    dirtyRef.current = true;
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      void flush();
    }, AUTOSAVE_MS);
  }, [flush]);

  // Flush a pending edit if the tab is closed or re-pointed before it saved.
  useEffect(
    () => () => {
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
      const html = dirtyRef.current ? collect() : null;
      if (html) void writeFile(surface.path, html).catch(() => {});
    },
    [surface.path, collect],
  );

  const recount = useCallback(() => {
    const d = frameRef.current?.contentDocument;
    const text = d?.querySelector("[data-editable]")?.textContent ?? "";
    const m = text.trim().match(/\S+/g);
    setWords(m ? m.length : 0);
  }, []);

  // Wire the iframe up once it has loaded its doc: make the parts editable and
  // listen for edits.
  const onFrameLoad = useCallback(() => {
    const d = frameRef.current?.contentDocument;
    if (!d) return;
    for (const sel of ["[data-title]", "[data-byline]", "[data-editable]"]) {
      const el = d.querySelector(sel) as HTMLElement | null;
      if (el) el.contentEditable = "true";
    }
    const onInput = () => {
      scheduleSave();
      recount();
    };
    d.addEventListener("input", onInput);
    recount();
    // A brand-new, empty blog opens ready to type: drop the caret in the title.
    const titleEl = d.querySelector("[data-title]") as HTMLElement | null;
    const bodyEl = d.querySelector("[data-editable]") as HTMLElement | null;
    if (
      titleEl &&
      !titleEl.textContent?.trim() &&
      !bodyEl?.textContent?.trim()
    ) {
      titleEl.focus();
    }
  }, [scheduleSave, recount]);

  // Repaint the live edit iframe when the theme changes (no reload, so the
  // caret and unsaved edits survive).
  useEffect(() => {
    if (mode !== "edit") return;
    const d = frameRef.current?.contentDocument;
    const style = d?.getElementById("arbos-theme");
    if (style) style.textContent = themeTokens(theme);
  }, [theme, mode, editKey]);

  // Pick up external writes (the agent vibe-coding the open file) while clean.
  useEffect(() => {
    if (!active || !docVisible || state !== "ready") return;
    let stop = false;
    const id = window.setInterval(() => {
      fetchFile(surface.path, true)
        .then((info) => {
          if (stop) return;
          setMissing(false);
          if (info.mtime === mtimeRef.current || dirtyRef.current) return;
          mtimeRef.current = info.mtime;
          if (mode === "preview") {
            setPreviewTick((t) => t + 1);
            return;
          }
          fetchFile(surface.path).then((full) => {
            if (stop || dirtyRef.current) return;
            rebuild(parseBlog(full.content ?? "", surface.path));
          });
        })
        .catch((e: unknown) => {
          if (!stop && isGone(e)) setMissing(true);
        });
    }, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, state, mode, surface.path, rebuild]);

  /* --- toolbar commands, applied to the iframe's document --- */

  const exec = useCallback((command: string, value?: string) => {
    const win = frameRef.current?.contentWindow;
    const d = frameRef.current?.contentDocument;
    if (!win || !d) return;
    win.focus();
    d.execCommand(command, false, value);
    d.dispatchEvent(new Event("input"));
  }, []);

  const insertHtml = useCallback(
    (html: string) => exec("insertHTML", html),
    [exec],
  );

  const setBlock = useCallback((tag: string) => exec("formatBlock", tag), [exec]);

  const makeLink = useCallback(() => {
    const url = window.prompt("Link URL");
    if (!url) return;
    exec("createLink", url);
  }, [exec]);

  const insertCode = useCallback(() => {
    insertHtml(
      `<pre><code>${escapeText("// your code here")}</code></pre><p><br></p>`,
    );
  }, [insertHtml]);

  const insertVideo = useCallback(() => {
    const url = window.prompt("Video URL (YouTube, Vimeo, or a video file)");
    if (!url) return;
    const model = modelRef.current;
    if (!model) return;
    const yt = url.match(
      /(?:youtu\.be\/|youtube\.com\/(?:watch\?v=|embed\/))([\w-]{11})/,
    );
    let raw: string;
    if (yt) {
      raw = `<figure class="island" data-island="embed" data-export="Video: ${escapeAttr(url)}"><iframe width="100%" style="aspect-ratio:16/9;border:0;border-radius:10px" src="https://www.youtube.com/embed/${yt[1]}" allowfullscreen></iframe></figure>`;
    } else {
      raw = `<figure class="island" data-island="embed" data-export="Video: ${escapeAttr(url)}"><video controls style="width:100%;border-radius:10px" src="${escapeAttr(url)}"></video></figure>`;
    }
    insertHtml(addIsland(model, "video", `Video: ${url}`, raw));
  }, [insertHtml]);

  const insertRawHtml = useCallback(() => {
    const html = window.prompt("Paste HTML to embed (runs live in Preview)");
    if (!html) return;
    const model = modelRef.current;
    if (!model) return;
    const raw = `<figure class="island" data-island="embed" data-export="Custom HTML block">${html}</figure>`;
    insertHtml(addIsland(model, "html", "Custom HTML block", raw));
  }, [insertHtml]);

  const insertTable = useCallback(() => {
    const spec = window.prompt("Table size as rows x cols (e.g. 3x3)", "3x3");
    if (!spec) return;
    const m = spec.match(/(\d+)\s*[x\u00d7,]\s*(\d+)/i);
    const rows = m ? parseInt(m[1], 10) : 3;
    const cols = m ? parseInt(m[2], 10) : 3;
    insertHtml(tableHtml(rows, cols));
  }, [insertHtml]);

  const insertMath = useCallback(() => {
    const tex = window.prompt(
      "LaTeX (e.g. e^{i\\pi} + 1 = 0 or \\sum_{n=1}^{\\infty} 1/n^2)",
    );
    if (!tex) return;
    const model = modelRef.current;
    if (!model) return;
    const ph = mathIsland(model, tex, true);
    if (!ph) {
      setSaveError("Couldn't render that LaTeX — check the syntax.");
      return;
    }
    insertHtml(ph);
  }, [insertHtml]);

  const pickImage = useCallback((target: "cover" | "body") => {
    imgTarget.current = target;
    fileInput.current?.click();
  }, []);

  const onImageChosen = useCallback(
    async (file: File) => {
      const model = modelRef.current;
      if (!model) return;
      const buf = await file.arrayBuffer();
      let bin = "";
      const bytes = new Uint8Array(buf);
      for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
      const b64 = btoa(bin);
      const safe = file.name.replace(/[^\w.-]+/g, "-");
      const name = `${Date.now().toString(36)}-${safe}`;
      const rel = blogAssetSrc(surface.path, name);
      try {
        await writeFileBase64(blogAssetPath(surface.path, name), b64);
      } catch (e) {
        setSaveError(e instanceof Error ? e.message : String(e));
        return;
      }
      if (imgTarget.current === "cover") {
        coverSrcRef.current = rel;
        const d = frameRef.current?.contentDocument;
        const fig = d?.querySelector("[data-cover]");
        if (fig) {
          fig.classList.remove("empty");
          fig.innerHTML = `<img src="${escapeAttr(rel)}" alt="">`;
        }
        scheduleSave();
      } else {
        insertHtml(`<img src="${escapeAttr(rel)}" alt="">`);
      }
    },
    [surface.path, insertHtml, scheduleSave],
  );

  const toPreview = useCallback(async () => {
    await flush();
    setPreviewTick((t) => t + 1);
    setMode("preview");
  }, [flush]);

  if (state === "error") {
    return <div className="p-4 text-[12.5px] text-red">{errorMsg}</div>;
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <Toolbar
        mode={mode}
        words={words}
        saving={saving}
        saveError={saveError}
        path={surface.path}
        onEdit={() => setMode("edit")}
        onPreview={toPreview}
        onReload={() => setEditKey((k) => k + 1)}
        exec={exec}
        setBlock={setBlock}
        makeLink={makeLink}
        insertCode={insertCode}
        insertVideo={insertVideo}
        insertRawHtml={insertRawHtml}
        insertTable={insertTable}
        insertMath={insertMath}
        insertDivider={() => insertHtml("<hr><p><br></p>")}
        pickCover={() => pickImage("cover")}
        pickImage={() => pickImage("body")}
        insertEmoji={(e) => exec("insertText", e)}
      />

      {missing && (
        <div className="shrink-0 select-none border-b border-line/60 px-3 py-1.5 text-[11.5px] text-warn">
          File no longer exists on disk — showing the last loaded version.
        </div>
      )}

      <input
        ref={fileInput}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (f) void onImageChosen(f);
        }}
      />

      <div className="min-h-0 min-w-0 flex-1">
        {state === "loading" ? (
          <div className="flex items-center gap-2 p-4 text-faint">
            <Loader2 size={13} className="animate-spin" /> Loading…
          </div>
        ) : mode === "edit" ? (
          <iframe
            key={editKey}
            ref={frameRef}
            srcDoc={editDoc}
            title={surfaceTitle(surface)}
            onLoad={onFrameLoad}
            className="size-full border-0 bg-canvas"
          />
        ) : (
          <PreviewFrame surface={surface} theme={theme} tick={previewTick} />
        )}
      </div>
    </div>
  );
}

/** The live, sandboxed render — animations, charts and demos run here. */
function PreviewFrame({
  surface,
  theme,
  tick,
}: {
  surface: Surface;
  theme: Theme;
  tick: number;
}) {
  const [html, setHtml] = useState<string | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let stale = false;
    fetchFile(surface.path)
      .then((info) => {
        if (!stale) setHtml(info.content ?? "");
      })
      .catch((e: unknown) => {
        if (!stale) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stale = true;
    };
  }, [surface.path, tick]);

  if (error) return <div className="p-4 text-[12.5px] text-red">{error}</div>;
  if (html === null) {
    return (
      <div className="flex items-center gap-2 p-4 text-faint">
        <Loader2 size={13} className="animate-spin" /> Loading…
      </div>
    );
  }
  return (
    <iframe
      srcDoc={themedCanvasDoc(html, theme, surface.path)}
      title={surfaceTitle(surface)}
      sandbox="allow-scripts"
      className="size-full border-0"
    />
  );
}

const COMMON_EMOJI = [
  "😀", "😄", "😁", "😊", "😍", "🤔", "😅", "😂", "🙌", "👏",
  "👍", "👎", "🔥", "✨", "⭐", "🎉", "🚀", "💡", "✅", "❌",
  "⚠️", "📈", "📉", "📊", "🧠", "❤️", "💬", "📝", "🔗", "🌍",
];

/** The formatting bar — Twitter-Articles-style controls over the document. */
function Toolbar({
  mode,
  words,
  saving,
  saveError,
  path,
  onEdit,
  onPreview,
  onReload,
  exec,
  setBlock,
  makeLink,
  insertCode,
  insertVideo,
  insertRawHtml,
  insertTable,
  insertMath,
  insertDivider,
  pickCover,
  pickImage,
  insertEmoji,
}: {
  mode: "edit" | "preview";
  words: number;
  saving: boolean;
  saveError: string;
  path: string;
  onEdit: () => void;
  onPreview: () => void;
  onReload: () => void;
  exec: (command: string, value?: string) => void;
  setBlock: (tag: string) => void;
  makeLink: () => void;
  insertCode: () => void;
  insertVideo: () => void;
  insertRawHtml: () => void;
  insertTable: () => void;
  insertMath: () => void;
  insertDivider: () => void;
  pickCover: () => void;
  pickImage: () => void;
  insertEmoji: (e: string) => void;
}) {
  const [menu, setMenu] = useState<"block" | "insert" | "emoji" | null>(null);
  // Keep the iframe selection alive: a toolbar mousedown must not steal focus.
  const hold = (e: React.MouseEvent) => e.preventDefault();
  const edit = mode === "edit";

  const btn =
    "flex h-7 items-center justify-center rounded-md px-1.5 text-muted transition-colors hover:bg-hover hover:text-text disabled:opacity-40 disabled:hover:bg-transparent";

  return (
    <div className="relative flex h-10 shrink-0 select-none items-center gap-1 border-b border-line/70 px-2">
      {/* Edit / Preview toggle */}
      <div className="flex items-center rounded-md border border-line/70 p-0.5">
        <button
          type="button"
          onClick={onEdit}
          className={`flex h-6 items-center gap-1 rounded px-2 text-[11.5px] transition-colors ${edit ? "bg-hover text-text" : "text-muted hover:text-text"}`}
        >
          <PenLine size={12} /> Edit
        </button>
        <button
          type="button"
          onClick={onPreview}
          className={`flex h-6 items-center gap-1 rounded px-2 text-[11.5px] transition-colors ${!edit ? "bg-hover text-text" : "text-muted hover:text-text"}`}
        >
          <Eye size={12} /> Preview
        </button>
      </div>

      {edit && (
        <>
          <div className="mx-1 h-5 w-px bg-line/70" />
          <button type="button" title="Bold" className={btn} onMouseDown={hold} onClick={() => exec("bold")}>
            <Bold size={14} />
          </button>
          <button type="button" title="Italic" className={btn} onMouseDown={hold} onClick={() => exec("italic")}>
            <Italic size={14} />
          </button>
          <button type="button" title="Strikethrough" className={btn} onMouseDown={hold} onClick={() => exec("strikeThrough")}>
            <Strikethrough size={14} />
          </button>

          <div className="mx-1 h-5 w-px bg-line/70" />
          {/* Block format */}
          <div className="relative">
            <button
              type="button"
              title="Text style"
              className={`${btn} gap-0.5 text-[11.5px]`}
              onMouseDown={hold}
              onClick={() => setMenu(menu === "block" ? null : "block")}
            >
              Body <ChevronDown size={12} />
            </button>
            {menu === "block" && (
              <Dropdown onClose={() => setMenu(null)}>
                {[
                  ["Body", "p"],
                  ["Heading", "h2"],
                  ["Subheading", "h3"],
                ].map(([label, tag]) => (
                  <DropItem
                    key={tag}
                    onClick={() => {
                      setBlock(tag);
                      setMenu(null);
                    }}
                  >
                    {label}
                  </DropItem>
                ))}
              </Dropdown>
            )}
          </div>

          <button type="button" title="Quote" className={btn} onMouseDown={hold} onClick={() => setBlock("blockquote")}>
            <Quote size={14} />
          </button>
          <button type="button" title="Bulleted list" className={btn} onMouseDown={hold} onClick={() => exec("insertUnorderedList")}>
            <List size={14} />
          </button>
          <button type="button" title="Numbered list" className={btn} onMouseDown={hold} onClick={() => exec("insertOrderedList")}>
            <ListOrdered size={14} />
          </button>
          <button type="button" title="Link" className={btn} onMouseDown={hold} onClick={makeLink}>
            <LinkIcon size={14} />
          </button>

          <div className="relative">
            <button type="button" title="Emoji" className={btn} onMouseDown={hold} onClick={() => setMenu(menu === "emoji" ? null : "emoji")}>
              <Smile size={14} />
            </button>
            {menu === "emoji" && (
              <Dropdown onClose={() => setMenu(null)} wide>
                <div className="grid grid-cols-6 gap-0.5 p-1">
                  {COMMON_EMOJI.map((e) => (
                    <button
                      key={e}
                      type="button"
                      className="flex size-7 items-center justify-center rounded hover:bg-hover"
                      onMouseDown={hold}
                      onClick={() => {
                        insertEmoji(e);
                        setMenu(null);
                      }}
                    >
                      {e}
                    </button>
                  ))}
                </div>
              </Dropdown>
            )}
          </div>

          <div className="mx-1 h-5 w-px bg-line/70" />
          {/* Insert */}
          <div className="relative">
            <button
              type="button"
              title="Insert"
              className={`${btn} gap-0.5 text-[11.5px]`}
              onMouseDown={hold}
              onClick={() => setMenu(menu === "insert" ? null : "insert")}
            >
              Insert <ChevronDown size={12} />
            </button>
            {menu === "insert" && (
              <Dropdown onClose={() => setMenu(null)}>
                <DropItem onClick={() => { pickImage(); setMenu(null); }}>
                  <ImageIcon size={13} /> Image
                </DropItem>
                <DropItem onClick={() => { insertVideo(); setMenu(null); }}>
                  <Video size={13} /> Video
                </DropItem>
                <DropItem onClick={() => { insertTable(); setMenu(null); }}>
                  <TableIcon size={13} /> Table
                </DropItem>
                <DropItem onClick={() => { insertMath(); setMenu(null); }}>
                  <Sigma size={13} /> Math (LaTeX)
                </DropItem>
                <DropItem onClick={() => { insertCode(); setMenu(null); }}>
                  <Code size={13} /> Code block
                </DropItem>
                <DropItem onClick={() => { insertRawHtml(); setMenu(null); }}>
                  <Code size={13} /> Embed HTML
                </DropItem>
                <DropItem onClick={() => { insertDivider(); setMenu(null); }}>
                  <Minus size={13} /> Divider
                </DropItem>
              </Dropdown>
            )}
          </div>

          <button type="button" title="Cover image" className={`${btn} gap-1 text-[11.5px]`} onMouseDown={hold} onClick={pickCover}>
            <ImageIcon size={14} /> Cover
          </button>
        </>
      )}

      <div className="ml-auto flex items-center gap-2 pl-2 text-[11px] text-faint">
        {saveError ? (
          <span className="text-red" title={saveError}>Save failed</span>
        ) : saving ? (
          <span className="flex items-center gap-1"><Loader2 size={11} className="animate-spin" /> Saving</span>
        ) : (
          <span>{words} words</span>
        )}
        <button type="button" title="Reload from disk" onClick={onReload} className={btn}>
          <RotateCw size={12} />
        </button>
        <a
          href={rawUrl(path)}
          target="_blank"
          rel="noreferrer"
          title="Open in browser tab"
          className={btn}
        >
          <ExternalLink size={12} />
        </a>
      </div>
    </div>
  );
}

function Dropdown({
  children,
  onClose,
  wide,
}: {
  children: React.ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  useEffect(() => {
    const close = () => onClose();
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [onClose]);
  return (
    <div
      onClick={(e) => e.stopPropagation()}
      className={`absolute left-0 top-9 z-20 overflow-hidden rounded-lg border border-line/70 bg-card py-1 shadow-xl ${wide ? "w-56" : "min-w-36"}`}
    >
      {children}
    </div>
  );
}

function DropItem({
  children,
  onClick,
}: {
  children: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] text-text transition-colors hover:bg-hover"
    >
      {children}
    </button>
  );
}
