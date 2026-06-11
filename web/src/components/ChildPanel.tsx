import { useEffect, useRef } from "react";
import { ChevronLeft, Loader2, Orbit } from "lucide-react";

import { TranscriptList } from "./ChatView";
import type { TranscriptItem } from "@/lib/transcript";

/**
 * A sub-agent's chat, opened from its row in the parent transcript. Renders
 * in place of the parent's view — a normal full-width panel, not a side
 * sheet — with a back header returning to the parent. Read-only: the parent
 * owns the conversation; this is a window onto the worker.
 */
export function ChildPanel({
  title,
  items,
  running,
  onClose,
}: {
  title: string;
  items: TranscriptItem[];
  running: boolean;
  onClose: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-line/70 select-none">
        <div className="mx-auto flex h-10 w-full max-w-4xl items-center gap-2 px-3.5">
          <button
            type="button"
            onClick={onClose}
            title="Back (esc)"
            className="-ml-1.5 flex size-6 shrink-0 cursor-pointer items-center justify-center rounded text-muted transition-colors hover:bg-card hover:text-text"
          >
            <ChevronLeft size={15} />
          </button>
          <Orbit size={14} className="shrink-0 text-muted" />
          <span className="min-w-0 flex-1 truncate text-[12.5px] text-bright">
            {title}
          </span>
          {running && (
            <Loader2 size={13} className="shrink-0 animate-spin text-faint" />
          )}
        </div>
      </div>
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-y-auto"
      >
        {items.length === 0 ? (
          <div className="mx-auto w-full max-w-4xl px-4 py-6 text-faint">
            {running ? "Waiting for the sub-agent's first events…" : "No transcript."}
          </div>
        ) : (
          <TranscriptList items={items} />
        )}
      </div>
    </div>
  );
}
