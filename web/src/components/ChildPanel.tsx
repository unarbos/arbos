import { useEffect, useRef } from "react";
import { Loader2, Orbit, X } from "lucide-react";

import { TranscriptList } from "./ChatView";
import type { TranscriptItem } from "@/lib/transcript";

/**
 * A sub-agent's chat, opened from its tab in the parent transcript: a sheet
 * over the right side of the chat, rendering the child's items with the same
 * vocabulary as the main view. Read-only — the parent owns the conversation;
 * this is a window onto the worker.
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
    <div className="fixed inset-y-0 right-0 z-30 flex w-[min(560px,92vw)] flex-col border-l border-line bg-canvas shadow-[-24px_0_48px_rgba(0,0,0,0.35)]">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line/70 px-3.5 select-none">
        <Orbit size={14} className="shrink-0 text-accent" />
        <span className="min-w-0 flex-1 truncate text-[12.5px] text-bright">
          {title}
        </span>
        {running && (
          <Loader2 size={13} className="shrink-0 animate-spin text-faint" />
        )}
        <button
          type="button"
          onClick={onClose}
          title="Close (esc)"
          className="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded text-muted transition-colors hover:bg-card hover:text-text"
        >
          <X size={14} />
        </button>
      </div>
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-y-auto"
      >
        {items.length === 0 ? (
          <div className="px-4 py-6 text-faint">
            {running ? "Waiting for the sub-agent's first events…" : "No transcript."}
          </div>
        ) : (
          <TranscriptList items={items} />
        )}
      </div>
    </div>
  );
}
