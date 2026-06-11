import { useEffect, useRef, useState } from "react";
import { ExternalLink, Loader2, RotateCw } from "lucide-react";

import { Highlight, Markdown } from "./Markdown";
import { fetchFile } from "@/lib/api";
import { rawUrl, surfaceTitle, themedCanvasDoc, type Surface } from "@/lib/surface";
import { useTheme } from "@/lib/theme";

const STAT_POLL_MS = 2000;

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
}: {
  surface: Surface;
  active: boolean;
}) {
  // Bumped when the file changes on disk (or on manual reload): re-srcs the
  // iframe / refetches the content.
  const [tick, setTick] = useState(0);
  const mtimeRef = useRef(0);

  useEffect(() => {
    if (!active) return;
    let stop = false;
    const check = () => {
      fetchFile(surface.path, true)
        .then((info) => {
          if (stop) return;
          if (mtimeRef.current && info.mtime !== mtimeRef.current) {
            setTick((t) => t + 1);
          }
          mtimeRef.current = info.mtime;
        })
        .catch(() => {});
    };
    check();
    const id = window.setInterval(check, STAT_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, surface.path]);

  const src = `${rawUrl(surface.path)}?v=${tick}`;

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

      <div className="min-h-0 min-w-0 flex-1">
        {surface.kind === "canvas" ? (
          <CanvasSurface surface={surface} tick={tick} />
        ) : surface.kind === "image" ? (
          <div className="flex size-full items-center justify-center overflow-auto p-4">
            <img
              key={tick}
              src={src}
              alt={surfaceTitle(surface)}
              className="max-h-full max-w-full rounded-md border border-line/60 object-contain"
            />
          </div>
        ) : (
          <TextSurface surface={surface} tick={tick} />
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

/** Markdown documents and code files: fetched text, rendered read-only. */
function TextSurface({ surface, tick }: { surface: Surface; tick: number }) {
  const [state, setState] = useState<
    | { phase: "loading" }
    | { phase: "error"; message: string }
    | { phase: "ready"; content: string; truncated: boolean; binary: boolean }
  >({ phase: "loading" });

  useEffect(() => {
    let stale = false;
    fetchFile(surface.path)
      .then((info) => {
        if (stale) return;
        setState({
          phase: "ready",
          content: info.content ?? "",
          truncated: info.truncated ?? false,
          binary: info.binary ?? false,
        });
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
  }, [surface.path, tick]);

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

  return (
    <div className="size-full overflow-auto">
      {surface.kind === "doc" ? (
        <div className="mx-auto w-full max-w-4xl px-4 py-4">
          <Markdown content={state.content} />
        </div>
      ) : (
        <pre className="px-4 py-3 font-mono text-[12px] leading-[1.55] text-text/90">
          {state.content.split("\n").map((line, i) => (
            <div key={i} className="whitespace-pre">
              {line ? <Highlight text={line} /> : " "}
            </div>
          ))}
        </pre>
      )}
      {state.truncated && (
        <div className="px-4 pb-3 text-[11.5px] text-faint">
          Truncated — the file is larger than the panel reads.
        </div>
      )}
    </div>
  );
}
