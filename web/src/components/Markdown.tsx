import { useMemo, useRef, useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";

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
  comment: "text-[#7d8a6f]",
  string: "text-[#c69a7b]",
  number: "text-[#a8b58e]",
  keyword: "text-[#b48cb4]",
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
  const blocks = useMemo(() => parseBlocks(content), [content]);
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

function parseBlocks(text: string): BlockNode[] {
  const lines = text.split("\n");
  const blocks: BlockNode[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

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
      blocks.push({ type: "code", lang, content: codeLines.join("\n") });
      continue;
    }

    const headingMatch = line.match(/^(#{1,4})\s+(.+)/);
    if (headingMatch) {
      blocks.push({
        type: "heading",
        level: headingMatch[1].length,
        content: headingMatch[2],
      });
      i++;
      continue;
    }

    if (/^[-*_]{3,}\s*$/.test(line)) {
      blocks.push({ type: "hr" });
      i++;
      continue;
    }

    if (/^[-*+]\s/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^[-*+]\s/.test(lines[i])) {
        items.push(lines[i].replace(/^[-*+]\s/, ""));
        i++;
      }
      blocks.push({ type: "list", ordered: false, items });
      continue;
    }

    if (/^\d+[.)]\s/.test(line)) {
      const items: string[] = [];
      while (i < lines.length && /^\d+[.)]\s/.test(lines[i])) {
        items.push(lines[i].replace(/^\d+[.)]\s/, ""));
        i++;
      }
      blocks.push({ type: "list", ordered: true, items });
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
      blocks.push({ type: "table", header, rows });
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
      blocks.push({ type: "paragraph", content: paraLines.join("\n") });
    }
  }

  return blocks;
}

const COPIED_MS = 1500;

function CopyButton({ text }: { text: string }) {
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
      title="Copy"
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
          <Highlight text={content} />
          {caret}
        </code>
      </pre>
    </div>
  );
}

const HEADING_CLASS: Record<number, string> = {
  1: "text-[1.15em]",
  2: "text-[1.08em]",
  3: "text-[1.02em]",
  4: "text-[1em]",
};

function Block({ block, caret }: { block: BlockNode; caret?: ReactNode }) {
  switch (block.type) {
    case "code":
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
}

type InlineNode =
  | { type: "text"; content: string }
  | { type: "code"; content: string }
  | { type: "bold"; content: string }
  | { type: "italic"; content: string }
  | { type: "link"; text: string; href: string }
  | { type: "br" };

function parseInline(text: string): InlineNode[] {
  const nodes: InlineNode[] = [];
  // Pattern priority: code > link > bold > italic > bare URL > line break
  const pattern =
    /(``[^`]+``|`[^`]+`)|(\[([^\]]+)\]\(([^)]+)\))|(\*\*([^*]+)\*\*)|(\*([^*]+)\*)|(\bhttps?:\/\/[^\s<>)\]]+)|(\n)/g;
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
      nodes.push({ type: "link", text: match[3], href: match[4] });
    } else if (match[5]) {
      nodes.push({ type: "bold", content: match[6] });
    } else if (match[7]) {
      nodes.push({ type: "italic", content: match[8] });
    } else if (match[9]) {
      nodes.push({ type: "link", text: match[9], href: match[9] });
    } else if (match[10]) {
      nodes.push({ type: "br" });
    }

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    nodes.push({ type: "text", content: text.slice(lastIndex) });
  }

  return nodes;
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
                className="rounded-[4px] bg-white/[0.08] px-[5px] py-px font-mono text-[0.85em] text-bright"
              >
                {node.content}
              </code>
            );
          case "bold":
            return (
              <strong key={i} className="font-bold">
                {node.content}
              </strong>
            );
          case "italic":
            return <em key={i}>{node.content}</em>;
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
