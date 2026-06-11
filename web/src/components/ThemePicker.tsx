import { useEffect, useRef, useState } from "react";
import { Check, Palette } from "lucide-react";

import { setTheme, useTheme } from "@/lib/theme";
import { THEMES, type Theme } from "@/lib/themes";

/** Three-dot palette preview pulled straight from the theme's own colors. */
function Swatch({ theme }: { theme: Theme }) {
  const dots: string[] = [
    theme.colors.canvas,
    theme.colors.accent,
    theme.colors["syntax-keyword"],
  ];
  return (
    <span className="flex shrink-0 items-center gap-1">
      {dots.map((c, i) => (
        <span
          key={i}
          className="size-3 rounded-full ring-1 ring-line"
          style={{ backgroundColor: c }}
        />
      ))}
    </span>
  );
}

/**
 * Theme selector: a palette icon that drops a list of VS Code themes. Picking
 * one repaints the panel live and persists the choice. Mirrors the tab
 * strip's other icon buttons and the history dropdown's click-away behavior.
 */
export function ThemePicker() {
  const active = useTheme();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

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

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        title="Theme"
        onClick={() => setOpen((v) => !v)}
        className={`flex size-6 cursor-pointer items-center justify-center rounded-md transition-colors hover:bg-hover ${
          open ? "bg-hover text-text" : "text-muted hover:text-text"
        }`}
      >
        <Palette size={13} />
      </button>

      {open && (
        <div className="absolute right-0 top-8 z-30 flex max-h-96 w-60 flex-col overflow-y-auto rounded-lg border border-line bg-card py-1 shadow-xl shadow-black/40">
          {THEMES.map((theme) => (
            <button
              key={theme.id}
              type="button"
              onClick={() => {
                setTheme(theme.id);
                setOpen(false);
              }}
              className="flex w-full cursor-pointer items-center gap-2.5 px-3 py-1.5 text-left text-[12.5px] text-text transition-colors hover:bg-hover"
            >
              <Swatch theme={theme} />
              <span className="min-w-0 flex-1 truncate">{theme.name}</span>
              {theme.id === active.id && (
                <Check size={13} className="shrink-0 text-accent" />
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
