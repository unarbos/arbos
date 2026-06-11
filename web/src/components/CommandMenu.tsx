import { useEffect, useRef } from "react";
import { Pencil } from "lucide-react";

import type { SlashCommand } from "@/lib/api";

/**
 * Cursor-style slash command popup: anchored to the composer and filtered as
 * the user types a command name. Purely presentational — the composer keeps
 * focus and drives the highlight through its own key handling (↑/↓ move,
 * Tab/Enter accept, Esc dismisses).
 *
 * Each row carries an edit pencil that opens the command's template file in
 * a prompt-editor panel, and a name matching nothing gets a trailing create
 * row — type /myskill, pick "create", write, save, run.
 */
export function CommandMenu({
  commands,
  highlight,
  below,
  createName,
  onHover,
  onPick,
  onEdit,
  onCreate,
}: {
  commands: SlashCommand[];
  highlight: number;
  /** Render under the composer (a fresh tab's composer sits at the page top). */
  below?: boolean;
  /** A typed name matching no command: offer to create it (the last row). */
  createName?: string;
  onHover: (i: number) => void;
  onPick: (c: SlashCommand) => void;
  onEdit: (c: SlashCommand) => void;
  onCreate: (name: string) => void;
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
      className={`absolute inset-x-0 z-30 max-h-80 overflow-hidden rounded-xl border border-line bg-card shadow-xl shadow-black/40 ${
        below ? "top-full mt-1" : "bottom-full mb-1"
      }`}
    >
      <div className="max-h-80 overflow-y-auto py-1.5">
        {commands.length > 0 && (
          <div className="px-3.5 pb-1 pt-1.5 text-[12px] text-muted">
            Commands
          </div>
        )}
        <div ref={listRef}>
          {commands.map((c, i) => (
            <div
              key={c.name}
              onMouseEnter={() => onHover(i)}
              // mousedown, not click: the composer must keep focus through a pick.
              onMouseDown={(e) => {
                e.preventDefault();
                onPick(c);
              }}
              className={`group flex w-full cursor-pointer items-center gap-2 px-3.5 py-1.5 text-left transition-colors ${
                i === highlight ? "bg-hover" : ""
              }`}
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline gap-2">
                  <span className="truncate text-[13px] text-bright">
                    /{c.name}
                  </span>
                  {c.argument_hint && (
                    <span className="shrink-0 truncate text-[12px] text-faint">
                      {c.argument_hint}
                    </span>
                  )}
                </div>
                {c.description && (
                  <div className="truncate text-[12px] leading-snug text-muted">
                    {c.description}
                  </div>
                )}
              </div>
              <button
                type="button"
                title={`Edit /${c.name}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  onEdit(c);
                }}
                className={`shrink-0 cursor-pointer rounded p-1 text-faint transition-opacity hover:text-text ${
                  i === highlight ? "opacity-100" : "opacity-0 group-hover:opacity-100"
                }`}
              >
                <Pencil size={12} />
              </button>
            </div>
          ))}
          {createName && (
            <div
              onMouseEnter={() => onHover(commands.length)}
              onMouseDown={(e) => {
                e.preventDefault();
                onCreate(createName);
              }}
              className={`w-full cursor-pointer px-3.5 py-1.5 text-left transition-colors ${
                highlight === commands.length ? "bg-hover" : ""
              } ${commands.length > 0 ? "mt-1 border-t border-line/60 pt-2" : ""}`}
            >
              <div className="text-[13px] text-bright">/{createName}</div>
              <div className="truncate text-[12px] leading-snug text-muted">
                Create this command — write a prompt you can run with /
                {createName}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
