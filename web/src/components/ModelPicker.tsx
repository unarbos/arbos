import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, Infinity as InfinityIcon, Loader2 } from "lucide-react";

import { fetchModels, type ModelOption } from "@/lib/api";
import { Tooltip } from "./Tooltip";

/** Catalog shared across tabs — fetched once, then reused by every picker. */
let catalogPromise: Promise<ModelOption[]> | null = null;

function loadCatalog(): Promise<ModelOption[]> {
  if (!catalogPromise) {
    catalogPromise = fetchModels()
      .then((c) => c.models)
      .catch(() => {
        catalogPromise = null; // let a later open retry a failed fetch
        return [];
      });
  }
  return catalogPromise;
}

/** A model id shown compactly in the chip: the slug tail, e.g. `kimi-k2`. */
function shortLabel(id: string): string {
  if (!id) return "model";
  const tail = id.split("/").pop() ?? id;
  return tail;
}

const LIST_MAX = 50;

/**
 * The model selector, Cursor-style: a chip that shows the active model and
 * opens a typeahead over the provider's catalog. Type to filter (e.g. "ki"
 * surfaces the kimi models); Enter picks the top match, ↑/↓ move, Esc closes.
 * The composer mounts it opening upward over the seam's set_model; the
 * Settings tab mounts it opening downward (right-aligned, with an emptyLabel
 * for "no override") over the host preference file.
 */
export function ModelPicker({
  current,
  onSelect,
  side = "up",
  align = "left",
  emptyLabel,
}: {
  current: string;
  onSelect: (id: string) => void;
  /** Where the dropdown opens relative to the chip. */
  side?: "up" | "down";
  /** Which chip edge the dropdown hugs (keep it on-screen near a panel edge). */
  align?: "left" | "right";
  /** Chip text when nothing is selected (an unset override). */
  emptyLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const [models, setModels] = useState<ModelOption[] | null>(null);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open || models) return;
    loadCatalog().then(setModels);
  }, [open, models]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = models ?? [];
    if (!q) return all.slice(0, LIST_MAX);
    // Rank by where the query lands: an id hit beats a name-only hit, and an
    // earlier hit beats a later one — so "ki" floats the kimi models above the
    // models that merely contain "ki" inside "thinking".
    const score = (m: ModelOption): number => {
      const idIdx = m.id.toLowerCase().indexOf(q);
      if (idIdx >= 0) return idIdx;
      const nameIdx = (m.name ?? "").toLowerCase().indexOf(q);
      if (nameIdx >= 0) return 1000 + nameIdx;
      return Infinity;
    };
    return all
      .map((m) => ({ m, s: score(m) }))
      .filter((x) => x.s !== Infinity)
      .sort((a, b) => a.s - b.s || a.m.id.length - b.m.id.length)
      .slice(0, LIST_MAX)
      .map((x) => x.m);
  }, [models, query]);

  useEffect(() => setHighlight(0), [query]);

  // Keep the highlighted row in view as ↑/↓ walk past the fold.
  useEffect(() => {
    const el = listRef.current?.children[highlight] as HTMLElement | undefined;
    el?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  const pick = (id: string) => {
    onSelect(id);
    setOpen(false);
    setQuery("");
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlight((h) => Math.min(h + 1, filtered.length - 1));
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlight((h) => Math.max(h - 1, 0));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const m = filtered[highlight];
      if (m) pick(m.id);
    }
  };

  const chip = (
    <button
      type="button"
      aria-label={current ? `Model: ${current}` : "Select model"}
      onClick={() => setOpen((v) => !v)}
      className="flex max-w-[220px] cursor-pointer items-center gap-1 rounded-full border border-line px-2 py-0.5 text-[11px] text-muted transition-colors hover:text-text"
    >
      <InfinityIcon size={11} className="shrink-0" />
      {current || emptyLabel ? (
        <span className="truncate">
          {current ? shortLabel(current) : emptyLabel}
        </span>
      ) : (
        // Still resolving (the session's model arrives with the replay, the
        // catalog with /api/models) — a quiet placeholder beats flashing the
        // literal word "model" as if it were a value.
        <span className="w-12 animate-pulse rounded-sm bg-hover text-transparent select-none">
          &nbsp;
        </span>
      )}
      <ChevronDown size={10} className="shrink-0 text-faint" />
    </button>
  );

  return (
    <div ref={rootRef} className="relative" data-keep-focus>
      {/* The bubble would sit right under the open dropdown, so only offer it
          while closed. */}
      {open ? (
        chip
      ) : (
        <Tooltip side="top" label={current || "Select model"}>
          {chip}
        </Tooltip>
      )}

      {open && (
        <div
          className={`absolute z-30 flex max-h-80 w-80 flex-col overflow-hidden rounded-lg border border-line bg-card shadow-xl shadow-black/40 ${
            side === "up" ? "bottom-full mb-1" : "top-full mt-1"
          } ${align === "left" ? "left-0" : "right-0"}`}
        >
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder="Search models…"
            autoFocus
            className="border-b border-line/60 bg-transparent px-3 py-2 text-[12.5px] text-bright outline-none placeholder:text-faint"
          />
          <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto py-1">
            {models === null && (
              <div className="flex items-center gap-2 px-3 py-2 text-[12px] text-faint">
                <Loader2 size={12} className="animate-spin" />
                Loading models…
              </div>
            )}
            {models !== null && filtered.length === 0 && (
              <div className="px-3 py-2 text-[12px] text-faint">
                No models match
              </div>
            )}
            {filtered.map((m, i) => (
              <button
                key={m.id}
                type="button"
                onMouseEnter={() => setHighlight(i)}
                onClick={() => pick(m.id)}
                className={`flex w-full cursor-pointer flex-col items-start gap-0.5 px-3 py-1.5 text-left transition-colors ${
                  i === highlight ? "bg-hover" : ""
                }`}
              >
                <span className="flex w-full items-center gap-2">
                  <span
                    className={`min-w-0 flex-1 truncate font-mono text-[12px] ${
                      m.id === current ? "text-accent" : "text-text"
                    }`}
                  >
                    {m.id}
                  </span>
                  {m.context_length ? (
                    <span className="shrink-0 text-[10.5px] text-faint">
                      {Math.round(m.context_length / 1000)}k
                    </span>
                  ) : null}
                </span>
                {m.name && (
                  <span className="w-full truncate text-[11px] text-muted/70">
                    {m.name}
                  </span>
                )}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
