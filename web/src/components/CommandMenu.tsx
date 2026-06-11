import { useEffect, useRef } from "react";
import { SquareSlash } from "lucide-react";

import type { SlashCommand } from "@/lib/api";

/**
 * Cursor-style slash command popup: anchored to the composer and filtered as
 * the user types a command name. Purely presentational — the composer keeps
 * focus and drives the highlight through its own key handling (↑/↓ move,
 * Tab/Enter accept, Esc dismisses).
 */
export function CommandMenu({
  commands,
  highlight,
  below,
  onHover,
  onPick,
}: {
  commands: SlashCommand[];
  highlight: number;
  /** Render under the composer (a fresh tab's composer sits at the page top). */
  below?: boolean;
  onHover: (i: number) => void;
  onPick: (c: SlashCommand) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the highlighted row in view as ↑/↓ walk past the fold.
  useEffect(() => {
    const el = listRef.current?.children[highlight] as HTMLElement | undefined;
    el?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  return (
    <div
      data-keep-focus
      className={`absolute inset-x-0 z-30 max-h-64 overflow-hidden rounded-lg border border-line bg-card shadow-xl shadow-black/40 ${
        below ? "top-full mt-1" : "bottom-full mb-1"
      }`}
    >
      <div ref={listRef} className="max-h-64 overflow-y-auto py-1">
        {commands.map((c, i) => (
          <button
            key={c.name}
            type="button"
            onMouseEnter={() => onHover(i)}
            // mousedown, not click: the composer must keep focus through a pick.
            onMouseDown={(e) => {
              e.preventDefault();
              onPick(c);
            }}
            className={`flex w-full cursor-pointer items-baseline gap-2 px-3 py-1.5 text-left transition-colors ${
              i === highlight ? "bg-hover" : ""
            }`}
          >
            <span className="flex shrink-0 items-baseline gap-1.5 font-mono text-[12.5px] text-text">
              <SquareSlash size={11} className="translate-y-px text-muted" />
              /{c.name}
            </span>
            {c.argument_hint && (
              <span className="shrink-0 font-mono text-[11px] text-faint">
                {c.argument_hint}
              </span>
            )}
            {c.description && (
              <span className="min-w-0 flex-1 truncate text-[11.5px] text-muted">
                {c.description}
              </span>
            )}
          </button>
        ))}
      </div>
    </div>
  );
}
