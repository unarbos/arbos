import { useMemo } from "react";
import katex from "katex";
import "katex/dist/katex.min.css";

/* ------------------------------------------------------------------ */
/* Katex — renders a TeX snippet ($x$, $$x$$, \(x\), \[x\]) as typeset */
/* math. Loaded lazily from Markdown.tsx so the katex bundle is only   */
/* fetched when a transcript actually contains math.                   */
/* ------------------------------------------------------------------ */

export default function Katex({
  tex,
  display,
}: {
  tex: string;
  display?: boolean;
}) {
  const html = useMemo(() => {
    try {
      return katex.renderToString(tex, {
        displayMode: display,
        throwOnError: false,
        strict: false,
        output: "html",
      });
    } catch {
      // throwOnError:false still throws on some malformed input (e.g. bad
      // unicode); fall back to the raw source rather than crashing the row.
      return null;
    }
  }, [tex, display]);

  if (html === null) {
    return <span className="font-mono text-[0.9em]">{tex}</span>;
  }

  return display ? (
    <span
      className="my-1 block overflow-x-auto py-0.5 text-center"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  ) : (
    <span dangerouslySetInnerHTML={{ __html: html }} />
  );
}
