import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronRight,
  Columns3,
  Component,
  ExternalLink,
  FileText,
  Folder,
  Loader2,
  Plus,
  RotateCw,
  Rows3,
  Star,
  Terminal,
  X,
} from "lucide-react";

import { Highlight, Markdown } from "./Markdown";
import { PromptEditor } from "./PromptEditor";
import {
  closeBrowserTab,
  createBrowserTab,
  fetchFile,
  HttpError,
  writeFile,
  type FileInfo,
} from "@/lib/api";
import { columnCount, delimiterFor, parseCsv, toCsv } from "@/lib/csv";
import {
  dirSurface,
  fileSurface,
  joinDirPath,
  rawUrl,
  surfaceTitle,
  themedCanvasDoc,
  type Surface,
} from "@/lib/surface";
import { useTheme } from "@/lib/theme";
import { errMsg, toastError } from "@/lib/toast";
import { useDocumentVisible } from "@/lib/useDocumentVisible";

const STAT_POLL_MS = 2000;

/** Marks a poll failure that means the file is gone, not a network blip. */
function isGone(e: unknown): boolean {
  return e instanceof HttpError && e.status === 404;
}

/** The quiet banner a surface wears when its file vanished from disk: the
 * last loaded content stays useful, but the panel stops pretending it's
 * still a live view. */
function MissingNotice() {
  return (
    <div className="shrink-0 select-none border-b border-line/60 px-3 py-1.5 text-[11.5px] text-warn">
      File no longer exists on disk — showing the last loaded version.
    </div>
  );
}

/**
 * A surface tab's body: one openable artifact, rendered by kind. A canvas
 * (HTML) loads in a sandboxed iframe straight off /raw, an image renders
 * directly, markdown renders as a document, anything else as highlighted
 * code. The panel holds a *reference*; the content is fetched live — and
 * watched: while visible it polls the file's mtime, so the agent re-writing
 * an open artifact refreshes the panel within a beat.
 */
export function SurfaceView({
  surface,
  active,
  onNavigate,
  onOpenFile,
  onCloseSelf,
}: {
  surface: Surface;
  active: boolean;
  /** A browser tab navigated — re-reference THIS tab to the new directory. */
  onNavigate?: (surface: Surface) => void;
  /** A browser row opened a file — show it the way every surface opens. */
  onOpenFile?: (surface: Surface) => void;
  /** Close this panel's own tab (a screencast panel closing its browser tab). */
  onCloseSelf?: () => void;
}) {
  // Bumped when the file changes on disk (or on manual reload): re-srcs the
  // iframe / refetches the content.
  const [tick, setTick] = useState(0);
  const [missing, setMissing] = useState(false);
  const mtimeRef = useRef(0);

  const editor = surface.kind === "prompt";
  const browser = surface.kind === "dir";
  const live = surface.kind === "screencast";
  // A sheet is an editable grid that owns its own load/save/poll, like the
  // text editor below — it takes the whole tab, no slim header.
  const sheet = surface.kind === "sheet";
  // Text surfaces ARE their own editor (headerless); canvas/image keep a slim
  // header since there's nothing to type into.
  const editable = surface.kind === "doc" || surface.kind === "code";

  const docVisible = useDocumentVisible();
  useEffect(() => {
    // Canvas/image refresh by re-srcing on this poll; text surfaces and sheets
    // own their own change-watching (TextSurface/SheetSurface), and the browser
    // owns its (DirSurface).
    if (!active || !docVisible || editor || browser || editable || live || sheet) return;
    let stop = false;
    const check = () => {
      fetchFile(surface.path, true)
        .then((info) => {
          if (stop) return;
          setMissing(false);
          if (mtimeRef.current && info.mtime !== mtimeRef.current) {
            setTick((t) => t + 1);
          }
          mtimeRef.current = info.mtime;
        })
        .catch((e: unknown) => {
          // A vanished file is worth saying out loud; a network blip just
          // waits for the next poll.
          if (!stop && isGone(e)) setMissing(true);
        });
    };
    check();
    const id = window.setInterval(check, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, editor, browser, editable, live, sheet, surface.path]);

  const src = `${rawUrl(surface.path)}?v=${tick}`;

  // A prompt surface is the slash-command editor — its own header (Save, not
  // reload/open-raw) and its own change-watching, so it takes the whole tab.
  if (editor) {
    return <PromptEditor surface={surface} active={active} />;
  }
  // A dir surface is the file browser — its own header (breadcrumb) and its
  // own change-watching, same takeover as the editor.
  if (browser) {
    return (
      <DirSurface
        surface={surface}
        active={active}
        onNavigate={onNavigate}
        onOpenFile={onOpenFile}
      />
    );
  }

  // A code or markdown file opens as the editor itself — no header, no chrome,
  // just the file the way a normal editor opens one (it autosaves).
  if (editable) {
    return <TextSurface surface={surface} active={active} />;
  }

  // A CSV/TSV file opens as an editable grid — same headerless, autosaving,
  // disk-watching contract as TextSurface, just cells instead of lines.
  if (sheet) {
    return <SheetSurface surface={surface} active={active} />;
  }

  // A live browser screencast: frames stream over a WebSocket, painted as they
  // arrive. Not file-backed, so it owns its own connection and header.
  if (live) {
    return (
      <ScreencastSurface
        surface={surface}
        active={active}
        onOpenTab={onOpenFile}
        onCloseSelf={onCloseSelf}
      />
    );
  }

  // Canvas/image keep a slim header: reload + open-in-tab, since there's
  // nothing to type into.
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 select-none items-center gap-2 border-b border-line/70 px-3">
        <span className="min-w-0 truncate text-[12.5px] text-bright">
          {surfaceTitle(surface)}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-faint">
          {surface.path}
        </span>
        <button
          type="button"
          title="Reload"
          onClick={() => setTick((t) => t + 1)}
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
        >
          <RotateCw size={12} />
        </button>
        <a
          href={rawUrl(surface.path)}
          target="_blank"
          rel="noreferrer"
          title="Open in browser tab"
          className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
        >
          <ExternalLink size={12} />
        </a>
      </div>

      {missing && <MissingNotice />}
      <div className="min-h-0 min-w-0 flex-1">
        {surface.kind === "canvas" ? (
          <CanvasSurface surface={surface} tick={tick} />
        ) : surface.kind === "pdf" ? (
          <iframe
            key={tick}
            src={src}
            title={surfaceTitle(surface)}
            className="size-full border-0 bg-white"
          />
        ) : (
          <div className="flex size-full items-center justify-center overflow-auto p-4">
            <img
              key={tick}
              src={src}
              alt={surfaceTitle(surface)}
              className="max-h-full max-w-full rounded-md border border-line/60 object-contain"
            />
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * A canvas (agent-authored HTML), rendered as part of the app, not beside
 * it: the file is written against arbos's design tokens, and the panel
 * injects the ACTIVE theme's `--color-*` palette at render time (srcdoc +
 * sandbox), so the canvas wears whatever theme the chat wears — and
 * repaints when the user switches. allow-scripts without allow-same-origin
 * keeps it isolated from the app's origin.
 */
function CanvasSurface({ surface, tick }: { surface: Surface; tick: number }) {
  const theme = useTheme();
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

  if (error) {
    return <div className="p-4 text-[12.5px] text-red">{error}</div>;
  }
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

/** The screencast WebSocket for a given stream id (the agent's live browser). */
function screencastUrl(id: string): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/api/browser/screencast?id=${encodeURIComponent(id)}`;
}

/**
 * A live, USABLE view of the agent's browser: JPEG frames stream over a
 * WebSocket (CDP screencast, relayed by the gateway) and paint as they arrive,
 * and the user's clicks, scrolling, and typing forward back over the same
 * socket into the page. That return path is what makes logins work without any
 * special mode: the agent opens a sign-in page, the user clicks into the panel
 * and types their credentials, and the browser's persistent profile keeps the
 * session for every later run. The connection lives only while the panel is
 * visible — hidden, it disconnects so Chrome isn't streaming to nobody; shown
 * again, it reconnects and the hub's cached last frame paints immediately.
 * surface.path is the stream id.
 */
function ScreencastSurface({
  surface,
  active,
  onOpenTab,
  onCloseSelf,
}: {
  surface: Surface;
  active: boolean;
  /** Open another browser tab's panel beside this one (the header's +). */
  onOpenTab?: (surface: Surface) => void;
  /** Close this panel after its browser tab is closed (the header's ✕). */
  onCloseSelf?: () => void;
}) {
  const [frame, setFrame] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  // Whether the live tab can go back/forward — drives the controls' greying,
  // updated from the screencast stream on every navigation.
  const [nav, setNav] = useState({ canBack: false, canForward: false });
  // The address bar's in-progress edit; null = not editing, show the live URL.
  const [draft, setDraft] = useState<string | null>(null);
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    if (!active) return;
    let stop = false;
    const ws = new WebSocket(screencastUrl(surface.path));
    wsRef.current = ws;
    ws.onmessage = (e) => {
      if (stop) return;
      let u: {
        frame?: string;
        url?: string;
        nav?: { canBack: boolean; canForward: boolean };
      };
      try {
        u = JSON.parse(e.data as string) as typeof u;
      } catch {
        return;
      }
      if (u.frame) setFrame(u.frame);
      if (u.url) setUrl(u.url);
      if (u.nav) setNav(u.nav);
    };
    return () => {
      stop = true;
      if (wsRef.current === ws) wsRef.current = null;
      ws.close();
    };
  }, [surface.path, active]);

  const send = useCallback((ev: Record<string, unknown>) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(ev));
  }, []);

  /** Normalized [0,1] coordinates of a mouse event within the frame image. */
  const norm = (e: React.MouseEvent<HTMLImageElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return {
      x: Math.min(Math.max((e.clientX - r.left) / r.width, 0), 1),
      y: Math.min(Math.max((e.clientY - r.top) / r.height, 0), 1),
    };
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 select-none items-center gap-1 border-b border-line/70 px-2">
        {/* Nav controls, Cursor-style: back/forward navigate real history
            (greyed at the ends), reload re-fetches; star is a placeholder. */}
        <button
          type="button"
          title="Go back"
          disabled={!nav.canBack}
          onClick={() => send({ t: "back" })}
          className={`flex size-6 shrink-0 items-center justify-center rounded-md transition-colors ${
            nav.canBack
              ? "cursor-pointer text-muted hover:bg-hover hover:text-text"
              : "cursor-default text-faint/50"
          }`}
        >
          <ArrowLeft size={14} />
        </button>
        <button
          type="button"
          title="Go forward"
          disabled={!nav.canForward}
          onClick={() => send({ t: "forward" })}
          className={`flex size-6 shrink-0 items-center justify-center rounded-md transition-colors ${
            nav.canForward
              ? "cursor-pointer text-muted hover:bg-hover hover:text-text"
              : "cursor-default text-faint/50"
          }`}
        >
          <ArrowRight size={14} />
        </button>
        <button
          type="button"
          title="Reload this page"
          onClick={() => send({ t: "reload" })}
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
        >
          <RotateCw size={13} />
        </button>
        <button
          type="button"
          title="Bookmark this page (coming soon)"
          className="flex size-6 shrink-0 cursor-default items-center justify-center rounded-md text-faint"
        >
          <Star size={13} />
        </button>
        {/* The address bar: shows where the browser is, and navigates it —
            type a URL (or a search) and press Enter. Esc abandons the edit. */}
        <input
          value={draft ?? url}
          title="Enter a URL or search the web"
          placeholder="Enter a URL or search"
          spellCheck={false}
          onFocus={(e) => {
            setDraft(url);
            e.currentTarget.select();
          }}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => setDraft(null)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              // Read the DOM value, not the draft state: a paste or a
              // programmatic fill can land without the per-key onChange that
              // feeds draft, and Enter must navigate to what's visibly there.
              const target = e.currentTarget.value.trim();
              if (target) send({ t: "nav", s: target });
              setDraft(null);
              e.currentTarget.blur();
            } else if (e.key === "Escape") {
              e.preventDefault();
              setDraft(null);
              e.currentTarget.blur();
            }
          }}
          className="ml-1 min-w-0 flex-1 truncate rounded-md border border-line/60 bg-canvas/60 px-2 py-0.5 font-mono text-[11px] text-muted outline-none transition-colors focus:border-accent/60 focus:text-text"
        />
        {/* DevTools toggles, Cursor-style — placeholders for now. */}
        <button
          type="button"
          title="Console (coming soon)"
          className="flex size-6 shrink-0 cursor-default items-center justify-center rounded-md text-faint"
        >
          <Terminal size={13} />
        </button>
        <button
          type="button"
          title="Components (coming soon)"
          className="flex size-6 shrink-0 cursor-default items-center justify-center rounded-md text-faint"
        >
          <Component size={13} />
        </button>
        <button
          type="button"
          title="New browser tab"
          onClick={() => {
            createBrowserTab()
              .then((t) =>
                onOpenTab?.({
                  kind: "screencast",
                  path: t.stream,
                  title: `Browser ${t.id}`,
                }),
              )
              .catch((e: unknown) =>
                toastError(`New browser tab failed: ${errMsg(e)}`),
              );
          }}
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
        >
          <Plus size={12} />
        </button>
        <button
          type="button"
          title="Close this browser tab"
          onClick={() => {
            const tabID = surface.path.split("/").pop() ?? "";
            closeBrowserTab(tabID)
              .catch(() => {})
              .finally(() => onCloseSelf?.());
          }}
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-red"
        >
          <X size={12} />
        </button>
      </div>
      <div
        tabIndex={0}
        onKeyDown={(e) => {
          // The panel owns the keyboard while focused: printable characters
          // insert as text, editing/navigation keys forward by name. Browser
          // shortcuts (⌘L etc.) are left to the real browser chrome.
          if (e.metaKey || e.ctrlKey || e.altKey) return;
          e.preventDefault();
          if (e.key.length === 1) {
            send({ t: "text", s: e.key });
          } else {
            send({ t: "key", key: e.key });
          }
        }}
        className="flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-auto bg-black/40 p-2 outline-none"
      >
        {frame && url && url !== "about:blank" ? (
          <img
            src={`data:image/jpeg;base64,${frame}`}
            alt={surfaceTitle(surface)}
            title="Click to interact — you can type, click, and sign in here"
            draggable={false}
            onClick={(e) => {
              send({ t: "click", ...norm(e) });
              e.currentTarget.parentElement?.focus();
            }}
            onWheel={(e) => send({ t: "wheel", ...norm(e), dy: e.deltaY })}
            className="max-h-full max-w-full cursor-pointer select-none object-contain"
          />
        ) : (
          // No frame yet — or only about:blank's featureless black frame,
          // which used to paint as a void over this message. Say where to
          // go instead of showing nothing.
          <div className="flex items-center gap-2 text-faint">
            Blank tab — enter a URL or search above.
          </div>
        )}
      </div>
    </div>
  );
}

/** How long to wait after the last keystroke before autosaving. */
const AUTOSAVE_MS = 600;

/**
 * Markdown documents and code files — headerless, the file *is* the panel.
 * Code opens straight into the editor; a markdown doc renders by default and
 * a double-click drops into its source (Esc returns to the rendered view). The
 * editor *just works* like a normal file — type and it autosaves back through
 * the gateway a beat after you stop (⌘S forces it now), so there's no Save
 * step. While clean it polls disk, so an agent rewriting the open file still
 * refreshes; a dirty draft is never clobbered.
 */
function TextSurface({
  surface,
  active,
}: {
  surface: Surface;
  active: boolean;
}) {
  const [state, setState] = useState<
    | { phase: "loading" }
    | { phase: "error"; message: string }
    | { phase: "ready"; truncated: boolean; binary: boolean }
  >({ phase: "loading" });
  // `draft` is what's on screen; `saved` is the file as last written/read.
  const [draft, setDraft] = useState("");
  const [saved, setSaved] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [missing, setMissing] = useState(false);
  const mtimeRef = useRef(0);

  // Code is an editor on open; a doc renders until you double-click into it.
  // Reset when the tab is re-referenced to a different file.
  const [editMode, setEditMode] = useState(surface.kind === "code");
  useEffect(() => {
    setEditMode(surface.kind === "code");
  }, [surface.kind, surface.path]);

  const ready = state.phase === "ready";
  const truncated = ready && state.truncated;
  // A truncated read can't be edited — saving would write back only the slice
  // we hold, so it stays read-only.
  const canEdit = editMode && ready && !truncated && !(ready && state.binary);
  const dirty = ready && draft !== saved;

  // Load the file (and reload it whenever the tab points at a new path).
  useEffect(() => {
    let stale = false;
    fetchFile(surface.path)
      .then((info) => {
        if (stale) return;
        mtimeRef.current = info.mtime;
        setState({
          phase: "ready",
          truncated: info.truncated ?? false,
          binary: info.binary ?? false,
        });
        setDraft(info.content ?? "");
        setSaved(info.content ?? "");
        setSaveError("");
      })
      .catch((e: unknown) => {
        if (!stale) {
          setState({
            phase: "error",
            message: e instanceof Error ? e.message : String(e),
          });
        }
      });
    return () => {
      stale = true;
    };
  }, [surface.path]);

  // Pick up external writes (the agent rewriting an open file) while the panel
  // is clean; a dirty draft is left alone so we never clobber the user's edits.
  const docVisible = useDocumentVisible();
  useEffect(() => {
    if (!active || !docVisible || !ready || dirty) return;
    let stop = false;
    const id = window.setInterval(() => {
      fetchFile(surface.path)
        .then((info) => {
          if (stop) return;
          setMissing(false);
          if (info.mtime === mtimeRef.current) return;
          mtimeRef.current = info.mtime;
          setDraft(info.content ?? "");
          setSaved(info.content ?? "");
          setState((s) =>
            s.phase === "ready"
              ? { ...s, truncated: info.truncated ?? false }
              : s,
          );
        })
        .catch((e: unknown) => {
          // Deleted under us: keep the content, stop pretending it's live.
          if (!stop && isGone(e)) setMissing(true);
        });
    }, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, ready, dirty, surface.path]);

  const flush = useCallback(
    (text: string) => {
      setSaving(true);
      setSaveError("");
      writeFile(surface.path, text)
        .then((info) => {
          mtimeRef.current = info.mtime;
          setSaved(text);
        })
        .catch((e: unknown) => {
          setSaveError(e instanceof Error ? e.message : String(e));
        })
        .finally(() => setSaving(false));
    },
    [surface.path],
  );

  // Autosave: a beat after typing stops, write the draft back. Rescheduled on
  // every keystroke, so a save fires only once you pause.
  useEffect(() => {
    if (!canEdit || !dirty) return;
    const id = window.setTimeout(() => flush(draft), AUTOSAVE_MS);
    return () => window.clearTimeout(id);
  }, [draft, canEdit, dirty, flush]);

  // Flush any pending edit when the tab is closed or re-referenced before the
  // debounce fired, so a quick close never drops the last keystrokes.
  const pending = useRef<{ path: string; text: string } | null>(null);
  pending.current = dirty ? { path: surface.path, text: draft } : null;
  useEffect(
    () => () => {
      const p = pending.current;
      if (p) writeFile(p.path, p.text).catch(() => {});
    },
    [surface.path],
  );

  if (state.phase === "loading") {
    return (
      <div className="flex items-center gap-2 p-4 text-faint">
        <Loader2 size={13} className="animate-spin" /> Loading…
      </div>
    );
  }
  if (state.phase === "error") {
    return <div className="p-4 text-[12.5px] text-red">{state.message}</div>;
  }
  if (state.binary) {
    return (
      <div className="p-4 text-faint">
        Binary file — open it in a browser tab instead.
      </div>
    );
  }

  // A markdown doc wears a Preview/Markdown toggle — rendered view vs its
  // source. Preview still drops to source on double-click and the source
  // editor returns to preview on Esc, but the toggle makes the switch explicit.
  // The source editor autosaves like any text file; a truncated read shows its
  // source read-only since a save would only write back the slice we hold.
  if (surface.kind === "doc") {
    return (
      <div className="flex size-full flex-col">
        {missing && <MissingNotice />}
        <div className="flex h-9 shrink-0 select-none items-center border-b border-line/70 px-3">
          <DocModeToggle
            mode={editMode ? "source" : "preview"}
            onMode={(m) => setEditMode(m === "source")}
          />
        </div>
        {!editMode ? (
          <div className="min-h-0 flex-1 overflow-auto">
            <div
              onDoubleClick={() => !truncated && setEditMode(true)}
              title={truncated ? undefined : "Double-click to edit"}
              className="mx-auto w-full max-w-4xl px-4 py-4"
            >
              <Markdown content={draft} />
            </div>
          </div>
        ) : canEdit ? (
          <>
            <CodeEditor
              value={draft}
              onChange={setDraft}
              highlight={false}
              onSave={() => {
                if (dirty) flush(draft);
              }}
              onExit={() => setEditMode(false)}
            />
            <div className="flex h-7 shrink-0 select-none items-center gap-2 border-t border-line/70 px-3 text-[11px]">
              {saveError ? (
                <span className="min-w-0 flex-1 truncate text-red">{saveError}</span>
              ) : (
                <SaveStatus saving={saving} dirty={dirty} hint="Esc to preview" />
              )}
            </div>
          </>
        ) : (
          <div className="min-h-0 flex-1 overflow-auto">
            <pre className="whitespace-pre-wrap break-words px-4 py-3 font-mono text-[12px] leading-[1.55] text-text/90">
              {draft}
            </pre>
            {truncated && (
              <div className="px-4 pb-3 text-[11.5px] text-faint">
                Truncated — the file is larger than the panel reads, so it opens
                read-only.
              </div>
            )}
          </div>
        )}
      </div>
    );
  }

  // A code file IS the editor — headerless, autosaving.
  if (canEdit) {
    return (
      <div className="flex size-full flex-col">
        {missing && <MissingNotice />}
        <CodeEditor
          value={draft}
          onChange={setDraft}
          highlight
          onSave={() => {
            if (dirty) flush(draft);
          }}
        />
        <div className="flex h-7 shrink-0 select-none items-center gap-2 border-t border-line/70 px-3 text-[11px]">
          {saveError ? (
            <span className="min-w-0 flex-1 truncate text-red">{saveError}</span>
          ) : (
            <SaveStatus saving={saving} dirty={dirty} />
          )}
        </div>
      </div>
    );
  }

  // Read-only: a truncated code read we can't safely save back.
  return (
    <div className="flex size-full flex-col">
      {missing && <MissingNotice />}
      <div className="min-h-0 flex-1 overflow-auto">
        <pre className="px-4 py-3 font-mono text-[12px] leading-[1.55] text-text/90">
          {draft.split("\n").map((line, i) => (
            <div key={i} className="whitespace-pre">
              {line ? <Highlight text={line} /> : " "}
            </div>
          ))}
        </pre>
        {truncated && (
          <div className="px-4 pb-3 text-[11.5px] text-faint">
            Truncated — the file is larger than the panel reads, so it opens
            read-only.
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * The Preview/Markdown segmented toggle a markdown doc wears: rendered view vs
 * its source. A quiet pill that mirrors the editor's own header chrome.
 */
function DocModeToggle({
  mode,
  onMode,
}: {
  mode: "preview" | "source";
  onMode: (m: "preview" | "source") => void;
}) {
  const seg = (m: "preview" | "source", label: string) => (
    <button
      type="button"
      onClick={() => onMode(m)}
      aria-pressed={mode === m}
      className={`cursor-pointer rounded px-2 py-0.5 text-[11.5px] transition-colors ${
        mode === m ? "bg-hover text-bright" : "text-muted hover:text-text"
      }`}
    >
      {label}
    </button>
  );
  return (
    <div className="flex items-center gap-0.5 rounded-md border border-line/60 bg-panel p-0.5">
      {seg("preview", "Preview")}
      {seg("source", "Markdown")}
    </div>
  );
}

/** Spreadsheet column header for a 0-based index: 0→A, 25→Z, 26→AA. */
function colLabel(index: number): string {
  let n = index + 1;
  let label = "";
  while (n > 0) {
    const rem = (n - 1) % 26;
    label = String.fromCharCode(65 + rem) + label;
    n = Math.floor((n - 1) / 26);
  }
  return label;
}

/**
 * A CSV/TSV file as an editable spreadsheet grid — the sheet surface. Same
 * contract as TextSurface: the file *is* the panel, edits autosave a beat after
 * you stop typing (the serialized grid written back through the gateway), and
 * while the grid is clean it polls disk so an agent rewriting the file refreshes
 * the cells — a dirty edit is never clobbered. The delimiter follows the
 * extension (tab for .tsv/.tab, comma otherwise). A truncated or binary read
 * opens read-only, since a save would only write back the slice we hold.
 */
function SheetSurface({
  surface,
  active,
}: {
  surface: Surface;
  active: boolean;
}) {
  const delim = useMemo(() => delimiterFor(surface.path), [surface.path]);
  const [state, setState] = useState<
    | { phase: "loading" }
    | { phase: "error"; message: string }
    | { phase: "ready"; truncated: boolean; binary: boolean }
  >({ phase: "loading" });
  // `rows` is what's on screen; `saved` is the serialized file as last written
  // or read (normalized through toCsv, so a clean load isn't spuriously dirty).
  const [rows, setRows] = useState<string[][]>([]);
  const [saved, setSaved] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [missing, setMissing] = useState(false);
  const mtimeRef = useRef(0);

  const ready = state.phase === "ready";
  const truncated = ready && state.truncated;
  const canEdit = ready && !truncated && !(ready && state.binary);
  const serialized = useMemo(() => toCsv(rows, delim), [rows, delim]);
  const dirty = ready && serialized !== saved;

  // Load the file (and reload whenever the tab points at a new path).
  useEffect(() => {
    let stale = false;
    fetchFile(surface.path)
      .then((info) => {
        if (stale) return;
        mtimeRef.current = info.mtime;
        const parsed = parseCsv(info.content ?? "", delim);
        setState({
          phase: "ready",
          truncated: info.truncated ?? false,
          binary: info.binary ?? false,
        });
        setRows(parsed);
        setSaved(toCsv(parsed, delim));
        setSaveError("");
      })
      .catch((e: unknown) => {
        if (!stale) {
          setState({
            phase: "error",
            message: e instanceof Error ? e.message : String(e),
          });
        }
      });
    return () => {
      stale = true;
    };
  }, [surface.path, delim]);

  // Pick up external writes while the grid is clean; a dirty edit is left alone.
  const docVisible = useDocumentVisible();
  useEffect(() => {
    if (!active || !docVisible || !ready || dirty) return;
    let stop = false;
    const id = window.setInterval(() => {
      fetchFile(surface.path)
        .then((info) => {
          if (stop) return;
          setMissing(false);
          if (info.mtime === mtimeRef.current) return;
          mtimeRef.current = info.mtime;
          const parsed = parseCsv(info.content ?? "", delim);
          setRows(parsed);
          setSaved(toCsv(parsed, delim));
          setState((s) =>
            s.phase === "ready"
              ? { ...s, truncated: info.truncated ?? false }
              : s,
          );
        })
        .catch((e: unknown) => {
          if (!stop && isGone(e)) setMissing(true);
        });
    }, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, ready, dirty, surface.path, delim]);

  const flush = useCallback(
    (text: string) => {
      setSaving(true);
      setSaveError("");
      writeFile(surface.path, text)
        .then((info) => {
          mtimeRef.current = info.mtime;
          setSaved(text);
        })
        .catch((e: unknown) => {
          setSaveError(e instanceof Error ? e.message : String(e));
        })
        .finally(() => setSaving(false));
    },
    [surface.path],
  );

  // Autosave a beat after the last edit, rescheduled on every keystroke.
  useEffect(() => {
    if (!canEdit || !dirty) return;
    const id = window.setTimeout(() => flush(serialized), AUTOSAVE_MS);
    return () => window.clearTimeout(id);
  }, [serialized, canEdit, dirty, flush]);

  // Flush a pending edit if the tab closes before the debounce fired.
  const pending = useRef<{ path: string; text: string } | null>(null);
  pending.current = dirty ? { path: surface.path, text: serialized } : null;
  useEffect(
    () => () => {
      const p = pending.current;
      if (p) writeFile(p.path, p.text).catch(() => {});
    },
    [surface.path],
  );

  const setCell = useCallback((r: number, c: number, value: string) => {
    setRows((prev) => {
      const next = prev.map((row) => row.slice());
      while (next.length <= r) next.push([]);
      const row = next[r];
      while (row.length <= c) row.push("");
      row[c] = value;
      return next;
    });
  }, []);

  const addRow = useCallback(() => {
    setRows((prev) => {
      const width = Math.max(columnCount(prev), 1);
      return [...prev.map((r) => r.slice()), Array.from({ length: width }, () => "")];
    });
  }, []);

  const addColumn = useCallback(() => {
    setRows((prev) => (prev.length ? prev : [[]]).map((r) => [...r, ""]));
  }, []);

  if (state.phase === "loading") {
    return (
      <div className="flex items-center gap-2 p-4 text-faint">
        <Loader2 size={13} className="animate-spin" /> Loading…
      </div>
    );
  }
  if (state.phase === "error") {
    return <div className="p-4 text-[12.5px] text-red">{state.message}</div>;
  }
  if (state.binary) {
    return (
      <div className="p-4 text-faint">
        Binary file — open it in a browser tab instead.
      </div>
    );
  }

  const cols = Math.max(columnCount(rows), 1);
  const gutter =
    "sticky left-0 z-10 select-none border border-line/60 bg-panel px-2 text-center text-[11px] text-faint";

  return (
    <div className="flex size-full flex-col">
      {missing && <MissingNotice />}
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="border-collapse text-[12px]">
          <thead>
            <tr>
              <th className={`${gutter} top-0 z-20`} />
              {Array.from({ length: cols }, (_, c) => (
                <th
                  key={c}
                  className="sticky top-0 z-10 min-w-[7rem] select-none border border-line/60 bg-panel px-2 py-1 text-center text-[11px] font-medium text-muted"
                >
                  {colLabel(c)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, r) => (
              <tr key={r}>
                <td className={gutter}>{r + 1}</td>
                {Array.from({ length: cols }, (_, c) => (
                  <td
                    key={c}
                    className="min-w-[7rem] border border-line/60 p-0 align-top"
                  >
                    {canEdit ? (
                      <input
                        value={row[c] ?? ""}
                        onChange={(e) => setCell(r, c, e.target.value)}
                        spellCheck={false}
                        className="w-full bg-transparent px-2 py-1 text-text outline-none focus:bg-hover"
                      />
                    ) : (
                      <span className="block truncate px-2 py-1 text-text/90">
                        {row[c] ?? ""}
                      </span>
                    )}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {truncated && (
          <div className="px-4 py-2 text-[11.5px] text-faint">
            Truncated — the file is larger than the panel reads, so it opens
            read-only.
          </div>
        )}
      </div>
      <div className="flex h-7 shrink-0 select-none items-center gap-2 border-t border-line/70 px-3 text-[11px]">
        {canEdit && (
          <>
            <button
              type="button"
              onClick={addRow}
              title="Add a row"
              className="flex shrink-0 cursor-pointer items-center gap-1 rounded-md px-1.5 py-0.5 text-muted transition-colors hover:bg-hover hover:text-text"
            >
              <Rows3 size={12} /> Row
            </button>
            <button
              type="button"
              onClick={addColumn}
              title="Add a column"
              className="flex shrink-0 cursor-pointer items-center gap-1 rounded-md px-1.5 py-0.5 text-muted transition-colors hover:bg-hover hover:text-text"
            >
              <Columns3 size={12} /> Column
            </button>
            <span className="h-3.5 w-px shrink-0 bg-line/70" />
          </>
        )}
        {saveError ? (
          <span className="min-w-0 flex-1 truncate text-red">{saveError}</span>
        ) : (
          <SaveStatus saving={saving} dirty={dirty} />
        )}
      </div>
    </div>
  );
}

/** The autosave indicator: a quiet "Saving…/Saved", never a button to press. */
function SaveStatus({
  saving,
  dirty,
  hint,
}: {
  saving: boolean;
  dirty: boolean;
  hint?: string;
}) {
  const tail = hint ? <span className="text-faint/60"> · {hint}</span> : null;
  if (saving) {
    return (
      <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate text-faint">
        <Loader2 size={11} className="animate-spin" /> Saving…{tail}
      </span>
    );
  }
  if (dirty) {
    return (
      <span className="min-w-0 flex-1 truncate text-faint">Editing…{tail}</span>
    );
  }
  return (
    <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate text-faint">
      <Check size={11} /> Saved{tail}
    </span>
  );
}

/**
 * A plain editable text area that *reads* like a code editor: a highlighted
 * copy of the text sits behind a transparent textarea, the two share font,
 * padding and wrapping exactly, and the overlay scrolls with the caret. For a
 * markdown doc the highlight is skipped (its `#`/`*` aren't code tokens).
 */
function CodeEditor({
  value,
  onChange,
  highlight,
  onSave,
  onExit,
}: {
  value: string;
  onChange: (v: string) => void;
  highlight: boolean;
  onSave: () => void;
  /** Esc handler — a doc uses it to drop back to its rendered view. */
  onExit?: () => void;
}) {
  const taRef = useRef<HTMLTextAreaElement>(null);
  const preRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    taRef.current?.focus();
  }, []);

  const syncScroll = () => {
    const ta = taRef.current;
    const pre = preRef.current;
    if (ta && pre) {
      pre.scrollTop = ta.scrollTop;
      pre.scrollLeft = ta.scrollLeft;
    }
  };

  const shared =
    "m-0 size-full whitespace-pre-wrap break-words border-0 px-4 py-3 font-mono text-[12px] leading-[1.55]";

  return (
    <div className="relative min-h-0 w-full flex-1 overflow-hidden">
      {highlight && (
        <pre
          ref={preRef}
          aria-hidden
          className={`${shared} pointer-events-none absolute inset-0 overflow-hidden text-text/90`}
        >
          {/* Trailing newline keeps the last line's height in step with the
              textarea, which always reserves a row after a final newline. */}
          <Highlight text={value + "\n"} />
        </pre>
      )}
      <textarea
        ref={taRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onScroll={syncScroll}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "s") {
            e.preventDefault();
            onSave();
          } else if (e.key === "Escape" && onExit) {
            e.preventDefault();
            onExit();
          }
        }}
        spellCheck={false}
        className={`${shared} resize-none overflow-auto bg-transparent outline-none ${
          highlight ? "text-transparent caret-text" : "text-text"
        }`}
      />
    </div>
  );
}

/* ------------------------------------------------------------------ */
/* The file browser: a surface whose path is a directory. Same typed   */
/* reference as every surface — navigation just re-references the tab, */
/* and opening a file goes through the same door every surface opens.  */
/* ------------------------------------------------------------------ */

/** The breadcrumb's segments: each one a directory reference to jump to. */
function crumbs(path: string): { label: string; path: string }[] {
  if (path === "." || path === "") return [{ label: "workspace", path: "." }];
  const abs = path.startsWith("/");
  const out = abs
    ? [{ label: "/", path: "/" }]
    : [{ label: "workspace", path: "." }];
  let cur = "";
  for (const part of path.split("/").filter(Boolean)) {
    cur = cur ? cur + "/" + part : part;
    out.push({ label: part, path: abs ? "/" + cur : cur });
  }
  return out;
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * One directory, as a browsable panel: breadcrumb header, entries below
 * (directories first, dotfiles dimmed). Clicking a directory re-references
 * this same tab (the breadcrumb walks back up); clicking a file opens it as
 * a surface — viewer chosen by extension, exactly like a chip in the chat.
 * Watched like every surface: the dir's mtime is polled while visible, so an
 * agent writing files refreshes an open browser within a beat.
 */
function DirSurface({
  surface,
  active,
  onNavigate,
  onOpenFile,
}: {
  surface: Surface;
  active: boolean;
  onNavigate?: (surface: Surface) => void;
  onOpenFile?: (surface: Surface) => void;
}) {
  const [info, setInfo] = useState<FileInfo | null>(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);
  const mtimeRef = useRef(0);

  useEffect(() => {
    let stale = false;
    fetchFile(surface.path)
      .then((i) => {
        if (stale) return;
        setInfo(i);
        setError("");
        mtimeRef.current = i.mtime;
      })
      .catch((e: unknown) => {
        if (!stale) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stale = true;
    };
  }, [surface.path, tick]);

  const docVisible = useDocumentVisible();
  useEffect(() => {
    if (!active || !docVisible) return;
    let stop = false;
    const id = window.setInterval(() => {
      fetchFile(surface.path, true)
        .then((i) => {
          if (stop) return;
          if (mtimeRef.current && i.mtime !== mtimeRef.current) {
            setTick((t) => t + 1);
          }
          mtimeRef.current = i.mtime;
        })
        .catch(() => {});
    }, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, surface.path]);

  const segments = crumbs(surface.path);
  const entries = info?.entries ?? [];

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-9 shrink-0 select-none items-center gap-1 overflow-hidden border-b border-line/70 px-3">
        <Folder size={13} className="mr-1 shrink-0 text-muted" />
        {segments.map((seg, i) => {
          const last = i === segments.length - 1;
          return (
            <span key={seg.path} className="flex min-w-0 shrink items-center gap-1">
              {i > 0 && <ChevronRight size={11} className="shrink-0 text-faint" />}
              {last ? (
                <span className="truncate text-[12.5px] text-bright">{seg.label}</span>
              ) : (
                <button
                  type="button"
                  onClick={() => onNavigate?.(dirSurface(seg.path))}
                  className="cursor-pointer truncate text-[12.5px] text-muted transition-colors hover:text-text"
                >
                  {seg.label}
                </button>
              )}
            </span>
          );
        })}
        <span className="flex-1" />
        <button
          type="button"
          title="Reload"
          onClick={() => setTick((t) => t + 1)}
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted transition-colors hover:bg-hover hover:text-text"
        >
          <RotateCw size={12} />
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {error && <div className="px-4 py-3 text-[12.5px] text-red">{error}</div>}
        {!error && info === null && (
          <div className="flex items-center gap-2 px-4 py-3 text-faint">
            <Loader2 size={13} className="animate-spin" /> Loading…
          </div>
        )}
        {!error && info !== null && entries.length === 0 && (
          <div className="px-4 py-3 text-[12.5px] text-faint">Empty directory.</div>
        )}
        {entries.map((e) => {
          const child = joinDirPath(surface.path, e.name);
          const dim = e.name.startsWith(".");
          return (
            <button
              key={e.name}
              type="button"
              onClick={() =>
                e.dir ? onNavigate?.(dirSurface(child)) : onOpenFile?.(fileSurface(child))
              }
              className="flex w-full cursor-pointer items-center gap-2 px-3 py-[3px] text-left transition-colors hover:bg-hover"
            >
              {e.dir ? (
                <Folder size={13} className="shrink-0 text-accent/80" />
              ) : (
                <FileText size={13} className="shrink-0 text-faint" />
              )}
              <span
                className={`min-w-0 flex-1 truncate text-[12.5px] ${
                  dim ? "text-faint" : "text-text"
                }`}
              >
                {e.name}
              </span>
              {!e.dir && e.size !== undefined && e.size > 0 && (
                <span className="shrink-0 text-[11px] text-faint">{fmtSize(e.size)}</span>
              )}
            </button>
          );
        })}
        {info?.truncated && (
          <div className="px-4 py-2 text-[11.5px] text-faint">
            Truncated — the directory has more entries than the panel lists.
          </div>
        )}
      </div>
    </div>
  );
}
