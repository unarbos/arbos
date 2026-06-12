import { lazy, memo, Suspense, useMemo, useRef, useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import { parsePlotSpec } from "../lib/plot";
import { Chart } from "./Chart";

// Code-split: mermaid is a heavyweight bundle, only fetched when a
// transcript actually contains a ```mermaid fence.
const MermaidDiagram = lazy(() => import("./Mermaid"));

// Same deal for katex (math typesetting) — only fetched when a message
// actually contains $…$ / $$…$$ math.
const Katex = lazy(() => import("./Katex"));

/* ------------------------------------------------------------------ */
/* Highlight — a tiny regex tokenizer for code blocks and diff lines.  */
/* Comments, strings, numbers, keywords; everything else default. Not  */
/* a grammar — just enough color for code to read like Cursor's.       */
/* ------------------------------------------------------------------ */

const KEYWORDS = new Set(
  (
    "func var let const if else for while return import from export type " +
    "interface struct class def function async await switch case default " +
    "break continue package range go chan map nil null undefined true false " +
    "new try catch finally throw raise pass with as in not and or is None " +
    "True False this self pub fn mut impl use match enum static void int " +
    "string bool float64 byte error"
  ).split(" "),
);

const TOKEN_RE =
  /(\/\/[^\n]*|#[^\n]*|\/\*[\s\S]*?\*\/)|("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`)|(\b\d[\d_.]*\b)|(\b[A-Za-z_][A-Za-z0-9_]*\b)/g;

const TOKEN_CLASS = {
  comment: "text-syntax-comment",
  string: "text-syntax-string",
  number: "text-syntax-number",
  keyword: "text-syntax-keyword",
} as const;

/** Syntax-tinted code text, used by code blocks and diff cards. */
export function Highlight({ text }: { text: string }) {
  const nodes = useMemo(() => {
    const out: ReactNode[] = [];
    let last = 0;
    let m: RegExpExecArray | null;
    TOKEN_RE.lastIndex = 0;
    while ((m = TOKEN_RE.exec(text)) !== null) {
      if (m.index > last) out.push(text.slice(last, m.index));
      if (m[1]) {
        out.push(<span key={m.index} className={TOKEN_CLASS.comment}>{m[1]}</span>);
      } else if (m[2]) {
        out.push(<span key={m.index} className={TOKEN_CLASS.string}>{m[2]}</span>);
      } else if (m[3]) {
        out.push(<span key={m.index} className={TOKEN_CLASS.number}>{m[3]}</span>);
      } else if (m[4] && KEYWORDS.has(m[4])) {
        out.push(<span key={m.index} className={TOKEN_CLASS.keyword}>{m[4]}</span>);
      } else {
        out.push(m[0]);
      }
      last = m.index + m[0].length;
    }
    if (last < text.length) out.push(text.slice(last));
    return out;
  }, [text]);

  return <>{nodes}</>;
}

/**
 * Lightweight markdown renderer for model output. Handles code blocks, inline
 * code, bold, italic, headers, links, lists, and horizontal rules — not a
 * full CommonMark parser, optimized for typical assistant message patterns.
 *
 * `streaming` renders a blinking caret hugging the final character of the
 * last block instead of wrapping onto a new line after a block element.
 */
export function Markdown({
  content,
  streaming,
}: {
  content: string;
  streaming?: boolean;
}) {
  // Streaming appends only grow the tail, so re-parsing the whole message per
  // delta is O(n²) over its length. Cache the parse and re-parse only from the
  // last COMMITTED boundary: a blank line at fence depth 0 (see safeBoundary).
  // Everything before such a separator is immutable under further appends — a
  // later line can't reach back across a blank line to merge into an earlier
  // block — so committed blocks keep their object identity and the memoized
  // <Block> rows below bail out per delta. (Resuming from merely the last block
  // is unsound: a trailing list/paragraph can still merge with its predecessor
  // as more lines arrive — verified by a differential fuzz against full parse.)
  const cacheRef = useRef<ParseCache | null>(null);
  const blocks = useMemo(() => {
    const cache = cacheRef.current;
    let committedBlocks: BlockNode[] = [];
    let committedStarts: number[] = [];
    let committedLen = 0;
    if (
      streaming &&
      cache &&
      content.length > cache.committedLen &&
      content.startsWith(cache.text.slice(0, cache.committedLen))
    ) {
      committedBlocks = cache.committedBlocks;
      committedStarts = cache.committedStarts;
      committedLen = cache.committedLen;
    }

    const tail = parseBlocks(content.slice(committedLen));
    const blocks = committedBlocks.concat(tail.blocks);
    const starts = committedStarts.concat(
      tail.starts.map((s) => s + committedLen),
    );

    // Advance the committed boundary to the last fence-safe blank separator,
    // so the next delta re-parses only from there.
    if (streaming) {
      const boundary = safeBoundary(content, committedLen);
      if (boundary > committedLen) {
        let k = starts.length;
        while (k > 0 && starts[k - 1] >= boundary) k--;
        cacheRef.current = {
          text: content,
          committedLen: boundary,
          committedBlocks: blocks.slice(0, k),
          committedStarts: starts.slice(0, k),
        };
      } else {
        cacheRef.current = {
          text: content,
          committedLen,
          committedBlocks,
          committedStarts,
        };
      }
    } else {
      cacheRef.current = null;
    }
    return blocks;
  }, [content, streaming]);
  const caret = streaming ? <StreamingCaret /> : null;

  return (
    <div className="space-y-2.5">
      {blocks.map((block, i) => (
        <Block
          key={i}
          block={block}
          caret={caret && i === blocks.length - 1 ? caret : null}
        />
      ))}
      {blocks.length === 0 && caret}
    </div>
  );
}

/** The incremental parse state: the text it covers, the prefix length the
 *  parse has committed through (always a fence-safe blank-line boundary),
 *  and the blocks of that committed prefix with their char offsets. */
interface ParseCache {
  text: string;
  committedLen: number;
  committedBlocks: BlockNode[];
  committedStarts: number[];
}

/**
 * The furthest char offset (≥ `from`) the incremental parse may commit
 * through: just past the last blank line that sits at fence depth 0. Inside
 * an open ``` fence a blank line is fence *content* — its block is still
 * growing — so only blank lines outside any fence count. The scan starts at
 * `from`, which is itself always a depth-0 boundary, so the committed prefix
 * never needs its fence state recomputed. Mirrors parseBlocks' fence rules:
 * any `{3,} run opens, and only a bare run at least as long closes.
 */
function safeBoundary(text: string, from: number): number {
  let boundary = from;
  let fence = 0; // the open fence's backtick-run length; 0 = outside fences
  let i = from;
  while (i < text.length) {
    const nl = text.indexOf("\n", i);
    if (nl === -1) break; // the final, unterminated line can still grow
    const line = text.slice(i, nl);
    if (fence === 0) {
      const open = line.match(/^(`{3,})/);
      if (open) fence = open[1].length;
      else if (line.trim() === "") boundary = nl + 1;
    } else {
      const close = line.match(/^(`{3,})\s*$/);
      if (close && close[1].length >= fence) fence = 0;
    }
    i = nl + 1;
  }
  return boundary;
}

function StreamingCaret() {
  return (
    <span
      aria-hidden
      className="inline-block w-[0.5em] h-[1em] ml-0.5 align-[-0.15em] bg-muted/70 animate-pulse"
    />
  );
}

type BlockNode =
  | { type: "code"; lang: string; content: string }
  | { type: "heading"; level: number; content: string }
  | { type: "hr" }
  | { type: "list"; ordered: boolean; items: string[] }
  | { type: "table"; header: string[]; rows: string[][] }
  | { type: "paragraph"; content: string };

/** Split a `| a | b |` row into trimmed cells. */
function splitTableRow(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((c) => c.trim());
}

function isTableRow(line: string): boolean {
  return /^\s*\|.*\|\s*$/.test(line);
}

/** The `|---|:--:|` row that turns a pipe row above it into a table header. */
function isTableSeparator(line: string): boolean {
  if (!isTableRow(line)) return false;
  const cells = splitTableRow(line);
  return cells.length > 0 && cells.every((c) => /^:?-{2,}:?$/.test(c));
}

function parseBlocks(text: string): { blocks: BlockNode[]; starts: number[] } {
  const lines = text.split("\n");
  // Char offset of each line's start, so every block can record where it
  // begins — the resume point for the streaming tail re-parse.
  const lineStart: number[] = new Array(lines.length);
  for (let n = 0, off = 0; n < lines.length; n++) {
    lineStart[n] = off;
    off += lines[n].length + 1;
  }
  const blocks: BlockNode[] = [];
  const starts: number[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];
    const blockStart = lineStart[i];
    const open = (b: BlockNode) => {
      blocks.push(b);
      starts.push(blockStart);
    };

    // A fence closes only on a backtick run at least as long as the opener
    // (CommonMark), so ````-wrapped output containing ``` blocks stays one
    // block instead of breaking out mid-sample.
    const fenceMatch = line.match(/^(`{3,})(\S*)/);
    if (fenceMatch) {
      const fenceLen = fenceMatch[1].length;
      const lang = fenceMatch[2] || "";
      const codeLines: string[] = [];
      i++;
      while (i < lines.length) {
        const close = lines[i].match(/^(`{3,})\s*$/);
        if (close && close[1].length >= fenceLen) break;
        codeLines.push(lines[i]);
        i++;
      }
      i++;
      open({ type: "code", lang, content: codeLines.join("\n") });
      continue;
    }

    const headingMatch = line.match(/^(#{1,4})\s+(.+)/);
    if (headingMatch) {
      open({
        type: "heading",
        level: headingMatch[1].length,
        content: headingMatch[2],
      });
      i++;
      continue;
    }

    if (/^[-*_]{3,}\s*$/.test(line)) {
      open({ type: "hr" });
      i++;
      continue;
    }

    if (/^[-*+]\s/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^[-*+]\s/.test(lines[i])) {
        items.push(lines[i].replace(/^[-*+]\s/, ""));
        i++;
      }
      open({ type: "list", ordered: false, items });
      continue;
    }

    if (/^\d+[.)]\s/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\d+[.)]\s/.test(lines[i])) {
        items.push(lines[i].replace(/^\d+[.)]\s/, ""));
        i++;
      }
      open({ type: "list", ordered: true, items });
      continue;
    }

    if (isTableRow(line) && i + 1 < lines.length && isTableSeparator(lines[i + 1])) {
      const header = splitTableRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && isTableRow(lines[i])) {
        rows.push(splitTableRow(lines[i]));
        i++;
      }
      open({ type: "table", header, rows });
      continue;
    }

    if (line.trim() === "") {
      i++;
      continue;
    }

    const paraLines: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() !== "" &&
      // The first line is always consumed (it matched no other block, e.g. a
      // pipe row whose separator hasn't streamed in yet), so the loop can
      // never stall; later lines break on any block opener.
      (paraLines.length === 0 ||
        (!lines[i].match(/^`{3,}/) &&
          !lines[i].match(/^#{1,4}\s/) &&
          !lines[i].match(/^[-*+]\s/) &&
          !lines[i].match(/^\d+[.)]\s/) &&
          !lines[i].match(/^[-*_]{3,}\s*$/) &&
          !(isTableRow(lines[i]) && isTableSeparator(lines[i + 1] ?? ""))))
    ) {
      paraLines.push(lines[i]);
      i++;
    }
    if (paraLines.length > 0) {
      open({ type: "paragraph", content: paraLines.join("\n") });
    }
  }

  return { blocks, starts };
}

const COPIED_MS = 1500;

/** A click-to-copy icon button (flashes a green check), shared by code-block
 *  headers here and the transcript's per-message copy in ChatView. */
export function CopyButton({ text, title }: { text: string; title?: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  const copy = () => {
    void navigator.clipboard?.writeText(text);
    setCopied(true);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), COPIED_MS);
  };

  return (
    <button
      type="button"
      onClick={copy}
      title={title ?? "Copy"}
      className={`cursor-pointer transition-colors ${
        copied ? "text-green" : "text-faint hover:text-text"
      }`}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}

function CodeBlock({
  lang,
  content,
  caret,
}: {
  lang: string;
  content: string;
  caret?: ReactNode;
}) {
  return (
    <div className="overflow-hidden rounded-md border border-line/80">
      <div className="flex items-center justify-between bg-card px-3 py-1">
        <span className="text-[11px] text-faint select-none">
          {lang || "text"}
        </span>
        <CopyButton text={content} />
      </div>
      <pre className="overflow-x-auto px-3 py-2 font-mono text-[11.5px] leading-relaxed text-text/90">
        <code>
          {/* While the fence is still streaming (caret present) the text is
              plain — re-tokenizing a growing block per delta is O(n²) churn.
              The block re-renders highlighted once the fence closes (it stops
              being the tail) or the message finalizes. */}
          {caret ? content : <Highlight text={content} />}
          {caret}
        </code>
      </pre>
    </div>
  );
}

/**
 * A ```chart fence: JSON plot spec rendered as an inline SVG chart.
 * While the spec is still streaming in (or simply malformed), fall back —
 * placeholder mid-stream, code block + error once the message is final —
 * so a broken spec is always debuggable rather than invisible.
 */
function ChartFence({ content, caret }: { content: string; caret?: ReactNode }) {
  const parsed = useMemo(() => parsePlotSpec(content), [content]);

  if (!parsed.ok) {
    if (caret) {
      return (
        <div className="rounded-md border border-line/80 bg-card/50 px-3 py-2 text-[11.5px] text-faint">
          rendering chart…{caret}
        </div>
      );
    }
    return (
      <div className="space-y-1">
        <CodeBlock lang="chart" content={content} />
        <div className="text-[11.5px] text-red">chart: {parsed.error}</div>
      </div>
    );
  }

  return (
    <div className="overflow-hidden rounded-md border border-line/80">
      <div className="flex items-center justify-between bg-card px-3 py-1">
        <span className="text-[11px] text-faint select-none">chart</span>
        <CopyButton text={content} />
      </div>
      <div className="px-2 py-2">
        <Chart plot={parsed.plot} />
        {caret}
      </div>
    </div>
  );
}

/**
 * A ```mermaid fence: diagram source rendered as an inline SVG. The header
 * (label + copy) stays put while the lazily loaded renderer handles the
 * stream-tolerant placeholder / error fallback in the body.
 */
function MermaidFence({ content, caret }: { content: string; caret?: ReactNode }) {
  return (
    <div className="overflow-hidden rounded-md border border-line/80">
      <div className="flex items-center justify-between bg-card px-3 py-1">
        <span className="text-[11px] text-faint select-none">mermaid</span>
        <CopyButton text={content} />
      </div>
      <Suspense
        fallback={
          <div className="px-3 py-2 text-[11.5px] text-faint">
            rendering diagram…
          </div>
        }
      >
        <MermaidDiagram content={content} streaming={!!caret} />
      </Suspense>
      {caret}
    </div>
  );
}

const HEADING_CLASS: Record<number, string> = {
  1: "text-[1.15em]",
  2: "text-[1.08em]",
  3: "text-[1.02em]",
  4: "text-[1em]",
};

/**
 * One block row, memoized: the streaming tail re-parse keeps every settled
 * block's object identity, so per delta only the growing tail block (and the
 * one losing the caret) re-renders instead of the whole message.
 */
const Block = memo(function Block({ block, caret }: { block: BlockNode; caret?: ReactNode }) {
  switch (block.type) {
    case "code":
      if (block.lang === "chart") {
        return <ChartFence content={block.content} caret={caret} />;
      }
      if (block.lang === "mermaid") {
        return <MermaidFence content={block.content} caret={caret} />;
      }
      return <CodeBlock lang={block.lang} content={block.content} caret={caret} />;

    case "heading": {
      const Tag = `h${Math.min(block.level, 4)}` as "h1" | "h2" | "h3" | "h4";
      return (
        <Tag
          className={`m-0 mt-1 font-semibold text-bright ${HEADING_CLASS[Math.min(block.level, 4)]}`}
        >
          <InlineContent text={block.content} />
          {caret}
        </Tag>
      );
    }

    case "hr":
      return (
        <>
          <hr className="border-line" />
          {caret}
        </>
      );

    case "list": {
      const Tag = block.ordered ? "ol" : "ul";
      const last = block.items.length - 1;
      return (
        <Tag
          className={`space-y-0.5 ${block.ordered ? "list-decimal" : "list-disc"} pl-5 m-0`}
        >
          {block.items.map((item, i) => (
            <li key={i}>
              <InlineContent text={item} />
              {i === last ? caret : null}
            </li>
          ))}
        </Tag>
      );
    }

    case "table":
      return (
        <div className="overflow-x-auto">
          <table className="border-collapse text-[12.5px]">
            <thead>
              <tr>
                {block.header.map((h, i) => (
                  <th
                    key={i}
                    className="border border-line/70 px-2.5 py-1 text-left font-semibold text-bright"
                  >
                    <InlineContent text={h} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, ri) => (
                <tr key={ri}>
                  {row.map((cell, ci) => (
                    <td
                      key={ci}
                      className="border border-line/70 px-2.5 py-1 align-top"
                    >
                      <InlineContent text={cell} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
          {caret}
        </div>
      );

    case "paragraph":
      return (
        <p className="m-0 whitespace-pre-wrap">
          <InlineContent text={block.content} />
          {caret}
        </p>
      );
  }
});

type InlineNode =
  | { type: "text"; content: string }
  | { type: "code"; content: string }
  | { type: "math"; content: string; display: boolean }
  | { type: "bold"; content: string }
  | { type: "italic"; content: string }
  | { type: "image"; alt: string; src: string }
  | { type: "link"; text: string; href: string }
  | { type: "br" };

function parseInline(text: string): InlineNode[] {
  const nodes: InlineNode[] = [];
  // Pattern priority: code > math > image > link > bold > italic > bare URL >
  // line break. Math comes right after code so TeX containing * _ [ ] isn't
  // shredded by the emphasis/link rules. Display math ($$…$$ / \[…\]) may
  // span newlines; inline math ($…$ / \(…\)) must stay on one line, must not
  // hug whitespace, and the closing $ can't be followed by a digit — so
  // "$5 and $10" reads as money, not math.
  const pattern =
    /(``[^`]+``|`[^`]+`)|(\$\$([\s\S]+?)\$\$)|(\\\[([\s\S]+?)\\\])|(\$([^\s$](?:[^$\n]*?[^\s$])?)\$(?!\d))|(\\\(([^\n]+?)\\\))|(!\[([^\]]*)\]\(([^)]+)\))|(\[([^\]]+)\]\(([^)]+)\))|(\*\*([^*]+)\*\*)|(\*([^*]+)\*)|(\bhttps?:\/\/[^\s<>)\]]+)|(\n)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      nodes.push({ type: "text", content: text.slice(lastIndex, match.index) });
    }

    if (match[1]) {
      // Strip the matching backtick run (` or ``) and CommonMark's one space
      // of padding, so `` `code` `` renders as `code`.
      const content = match[1]
        .replace(/^(``|`)/, "")
        .replace(/(``|`)$/, "")
        .replace(/^ (.+) $/, "$1");
      nodes.push({ type: "code", content });
    } else if (match[2]) {
      nodes.push({ type: "math", content: match[3].trim(), display: true });
    } else if (match[4]) {
      nodes.push({ type: "math", content: match[5].trim(), display: true });
    } else if (match[6]) {
      nodes.push({ type: "math", content: match[7], display: false });
    } else if (match[8]) {
      nodes.push({ type: "math", content: match[9].trim(), display: false });
    } else if (match[10]) {
      nodes.push({ type: "image", alt: match[11], src: match[12] });
    } else if (match[13]) {
      nodes.push({ type: "link", text: match[14], href: match[15] });
    } else if (match[16]) {
      nodes.push({ type: "bold", content: match[17] });
    } else if (match[18]) {
      nodes.push({ type: "italic", content: match[19] });
    } else if (match[20]) {
      nodes.push({ type: "link", text: match[20], href: match[20] });
    } else if (match[21]) {
      nodes.push({ type: "br" });
    }

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    nodes.push({ type: "text", content: text.slice(lastIndex) });
  }

  // Display math renders as its own block, so a \n that streamed in right
  // next to it would otherwise add a stray blank line above/below.
  return nodes.filter((node, i) => {
    if (node.type !== "br") return true;
    const prev = nodes[i - 1];
    const next = nodes[i + 1];
    const isDisplay = (n: InlineNode | undefined) =>
      n?.type === "math" && n.display;
    return !isDisplay(prev) && !isDisplay(next);
  });
}

function InlineContent({ text }: { text: string }) {
  const nodes = useMemo(() => parseInline(text), [text]);

  return (
    <>
      {nodes.map((node, i) => {
        switch (node.type) {
          case "text":
            return <span key={i}>{node.content}</span>;
          case "code":
            return (
              <code
                key={i}
                className="rounded-[4px] bg-hover px-[5px] py-px font-mono text-[0.85em] text-bright"
              >
                {node.content}
              </code>
            );
          case "math":
            return (
              <Suspense
                key={i}
                fallback={<span className="font-mono text-[0.9em]">{node.content}</span>}
              >
                <Katex tex={node.content} display={node.display} />
              </Suspense>
            );
          case "bold":
            return (
              <strong key={i} className="font-bold">
                {node.content}
              </strong>
            );
          case "italic":
            return <em key={i}>{node.content}</em>;
          case "image": {
            // An embedded image in model output — e.g. the hosted URL the
            // image_generation server tool hands back. Same scheme rules as
            // links, plus inline base64 data: images; anything else drops to
            // the alt text.
            const src = node.src.trim();
            if (!/^(https?:|data:image\/)/i.test(src)) {
              return <span key={i}>{node.alt}</span>;
            }
            return (
              <img
                key={i}
                src={src}
                alt={node.alt}
                className="my-2 block max-h-96 max-w-full rounded-md border border-line/60"
              />
            );
          }
          case "link": {
            // Security: only http(s)/mailto links render as anchors; other
            // schemes (javascript:, data:) drop to plain text so a crafted
            // link in model output can't execute on click.
            const href = node.href.trim();
            if (!/^(https?:|mailto:)/i.test(href)) {
              return <span key={i}>{node.text}</span>;
            }
            return (
              <a
                key={i}
                href={href}
                target="_blank"
                rel="noreferrer"
                className="text-accent decoration-accent/40 underline-offset-2 hover:underline"
              >
                {node.text}
              </a>
            );
          }
          case "br":
            return <br key={i} />;
        }
      })}
    </>
  );
}
