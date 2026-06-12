import { useEffect, useState } from "react";
import mermaid from "mermaid";
import { useTheme } from "../lib/theme";
import type { Theme } from "../lib/themes";
import { Highlight } from "./Markdown";

/* ------------------------------------------------------------------ */
/* Mermaid — renders a ```mermaid fence as an inline diagram. Loaded   */
/* lazily from Markdown.tsx so the (large) mermaid bundle is only      */
/* fetched when a diagram actually appears in a transcript.            */
/*                                                                     */
/* Mermaid bakes colors into the SVG at render time, so unlike Chart   */
/* (which uses live CSS variables) we map the active theme's palette   */
/* onto mermaid's `base` theme variables and re-render on theme switch.*/
/* ------------------------------------------------------------------ */

function themeVariables(theme: Theme): Record<string, unknown> {
  const c = theme.colors;
  return {
    darkMode: theme.dark,
    background: c.canvas,
    fontFamily: getComputedStyle(document.body).fontFamily,
    fontSize: "13px",
    primaryColor: c.card,
    primaryTextColor: c.bright,
    primaryBorderColor: c.line,
    secondaryColor: c.panel,
    secondaryBorderColor: c.line,
    tertiaryColor: c.panel,
    tertiaryBorderColor: c.line,
    lineColor: c.muted,
    textColor: c.text,
    mainBkg: c.card,
    nodeBorder: c.line,
    clusterBkg: c.panel,
    clusterBorder: c.line,
    titleColor: c.bright,
    edgeLabelBackground: c.canvas,
    noteBkgColor: c.card,
    noteTextColor: c.text,
    noteBorderColor: c.line,
    actorBkg: c.card,
    actorBorder: c.line,
    actorTextColor: c.bright,
    labelBoxBkgColor: c.panel,
    labelTextColor: c.text,
    signalColor: c.muted,
    signalTextColor: c.text,
    pie1: c.accent,
    pie2: c.green,
    pie3: c.warn,
    pie4: c["syntax-keyword"],
    pie5: c["syntax-string"],
    pie6: c.red,
    pieTitleTextColor: c.bright,
    pieSectionTextColor: c.bright,
    pieLegendTextColor: c.text,
  };
}

let renderSeq = 0;

// Mermaid's initialize() rebuilds its whole config; doing it inside every
// render effect re-initializes per streamed re-render. The config only
// actually changes with the theme, so key one initialize per theme object.
let initializedFor: Theme | null = null;

function ensureInitialized(theme: Theme) {
  if (initializedFor === theme) return;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: "base",
    themeVariables: themeVariables(theme),
  });
  initializedFor = theme;
}

type RenderState =
  | { kind: "pending" }
  | { kind: "ok"; svg: string }
  | { kind: "error"; message: string };

/** First line of a mermaid parse error — they tend to be multi-line dumps. */
function errorMessage(err: unknown): string {
  const text = err instanceof Error ? err.message : String(err);
  return text.split("\n")[0] || "failed to render diagram";
}

const STREAM_DEBOUNCE_MS = 250;

export default function MermaidDiagram({
  content,
  streaming,
}: {
  content: string;
  streaming?: boolean;
}) {
  const theme = useTheme();
  const [state, setState] = useState<RenderState>({ kind: "pending" });

  useEffect(() => {
    let cancelled = false;

    const run = async () => {
      const id = `mermaid-${renderSeq++}`;
      try {
        ensureInitialized(theme);
        const { svg } = await mermaid.render(id, content);
        if (!cancelled) setState({ kind: "ok", svg });
      } catch (err) {
        if (!cancelled) setState({ kind: "error", message: errorMessage(err) });
      } finally {
        // Mermaid leaves its scratch element appended to <body> on failure
        // (a visible "Syntax error" SVG that would otherwise accumulate one
        // copy per failed render attempt) and on some success paths too.
        document.getElementById(`d${id}`)?.remove();
      }
    };

    // While the fence is still streaming in, most intermediate states are
    // unparsable — debounce so we only attempt mostly-settled text.
    const timer = window.setTimeout(run, streaming ? STREAM_DEBOUNCE_MS : 0);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [content, streaming, theme]);

  if (state.kind === "ok") {
    return (
      <div
        className="overflow-x-auto px-3 py-3 [&_svg]:mx-auto [&_svg]:block [&_svg]:max-w-full"
        dangerouslySetInnerHTML={{ __html: state.svg }}
      />
    );
  }

  // Mid-stream (or first render) the diagram is usually incomplete; show a
  // quiet placeholder. Only a final, still-broken fence surfaces the error
  // alongside the source so it's debuggable rather than invisible.
  if (state.kind === "pending" || streaming) {
    return (
      <div className="px-3 py-2 text-[11.5px] text-faint">
        rendering diagram…
      </div>
    );
  }

  return (
    <div className="space-y-1 px-3 py-2">
      <pre className="overflow-x-auto font-mono text-[11.5px] leading-relaxed text-text/90">
        <code>
          <Highlight text={content} />
        </code>
      </pre>
      <div className="text-[11.5px] text-red">mermaid: {state.message}</div>
    </div>
  );
}
