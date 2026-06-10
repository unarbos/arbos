import { useEffect, useRef, useState } from "react";

import { Markdown } from "./Markdown";
import { argsPreview, fmtElapsed } from "@/lib/format";
import type { TranscriptItem } from "@/lib/transcript";

/**
 * The transcript, rendered the way the TUI renders it: an append-only column
 * of lines. User prompts echo as `› text`, tool activity is one dim line per
 * call, the answer is markdown, and `ephemeral` is the rolling one-line
 * preview (reasoning / child narration) that never joins the transcript.
 */
export function ChatView({
  items,
  ephemeral,
}: {
  items: TranscriptItem[];
  ephemeral: string;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [items, ephemeral]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  return (
    <div
      ref={scrollRef}
      onScroll={onScroll}
      className="flex-1 min-h-0 overflow-y-auto"
    >
      <div className="max-w-2xl mx-auto px-4 py-6 space-y-3">
        {items.length === 0 && (
          <div className="text-muted select-none pt-16 text-center">
            <span className="text-primary tracking-[0.3em]">arbos</span>
          </div>
        )}
        {items.map((item) => (
          <Item key={item.id} item={item} />
        ))}
        {ephemeral && (
          <div className="text-muted/80 text-[0.85em] truncate select-none">
            {ephemeral}
          </div>
        )}
      </div>
    </div>
  );
}

function Item({ item }: { item: TranscriptItem }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="text-accent whitespace-pre-wrap break-words">
          <span className="text-muted select-none">› </span>
          {item.text}
        </div>
      );

    case "assistant":
      return (
        <div className="break-words">
          <Markdown content={item.text} streaming={item.streaming} />
          {!item.streaming && item.stopReason && item.stopReason !== "answered" && (
            <div className="text-warn text-[0.8em] mt-1">
              stopped: {item.stopReason}
            </div>
          )}
        </div>
      );

    case "tool":
      return <ToolLine item={item} />;

    case "queued":
      return (
        <div className="text-muted text-[0.85em]">queued · {item.text}</div>
      );

    case "interrupted":
      return <div className="text-warn text-[0.85em]">^C interrupted</div>;

    case "error":
      return (
        <div className="text-danger whitespace-pre-wrap break-words text-[0.9em]">
          {item.message}
          {item.retryable && (
            <span className="text-muted"> — send again to retry</span>
          )}
        </div>
      );

    default: {
      const never: never = item;
      void never;
      return null;
    }
  }
}

const TICK_MS = 500;
const ERROR_PREVIEW_MAX = 600;

/** One tool call as one line: `⚡ name args… ✓ 1.2s`. Failures show why. */
function ToolLine({ item }: { item: Extract<TranscriptItem, { kind: "tool" }> }) {
  const running = !item.result;
  const failed = item.result?.IsError ?? false;

  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const id = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(id);
  }, [running]);

  const elapsed = fmtElapsed((item.completedAt ?? now) - item.startedAt);

  return (
    <div
      className="text-[0.85em] leading-relaxed"
      style={{ paddingLeft: item.depth * 16 }}
    >
      <div className="flex items-baseline gap-2 min-w-0">
        <span className={failed ? "text-danger" : running ? "text-accent" : "text-primary"}>
          {failed ? "✗" : running ? "⚡" : "✓"}
        </span>
        <span className="text-text/90 shrink-0">{item.call.Name}</span>
        <span className="text-muted truncate min-w-0 flex-1">
          {argsPreview(item.call)}
        </span>
        <span className="text-muted/70 tabular-nums shrink-0">{elapsed}</span>
      </div>
      {failed && item.result && (
        <div className="text-danger/90 whitespace-pre-wrap break-words pl-5 pt-0.5">
          {item.result.Content.slice(0, ERROR_PREVIEW_MAX)}
        </div>
      )}
    </div>
  );
}
