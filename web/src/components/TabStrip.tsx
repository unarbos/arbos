import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Activity,
  AppWindow,
  Check,
  Clock,
  Columns2,
  Copy,
  FolderTree,
  Globe,
  Loader2,
  MessageSquare,
  Orbit,
  Plus,
  Rows2,
  Send,
  Settings,
  Share2,
  SquareTerminal,
  X,
} from "lucide-react";

import { createShareLink, type ShareScope } from "@/lib/api";
import type { SplitDir } from "@/lib/layout";
import { ThemePicker } from "./ThemePicker";
import { Tooltip } from "./Tooltip";

export interface TabInfo {
  key: number;
  title: string | null;
  busy: boolean;
  /** What the tab holds: an agent chat (default), an opened surface, a
   *  machine-spawned run's transcript, a terminal (job tail / shell), the
   *  merged activity + history view, the messenger panel, or the settings
   *  panel. */
  kind?:
    | "chat"
    | "surface"
    | "run"
    | "terminal"
    | "activity"
    | "messenger"
    | "settings"
    | "plan";
  /** Set when this tab holds something shareable (a bound chat or a file
   *  artifact); drives the per-tab Share affordance. Computed by App, which
   *  owns the session id / surface path. Absent = not shareable. */
  shareScope?: ShareScope;
}

/** What the + menu can open: a fresh chat, or one of the companion views. */
export type NewTabKind =
  | "chat"
  | "terminal"
  | "files"
  | "browser"
  | "messenger";

/**
 * Drag wiring shared across strips (the drag state lives in App, so a tab can
 * be dragged from one pane's strip into the other's).
 */
export interface TabDrag {
  onStart: (key: number) => void;
  /** Commit: drop the dragged tab next to `key`; before = left of it. */
  onDropTab: (key: number, before: boolean) => void;
  onEnd: () => void;
  /** A tab dropped on the strip's empty background: append to this pane. */
  onDropStrip: () => void;
}

/**
 * Cursor's tab strip, one per pane: a tab per open agent and a + menu that
 * opens any tab type in this pane. History, activity, and the global actions
 * (split, theme) render once, on the top-right strip, via `actions`; the
 * settings gear renders once, on the top-left strip, via `leading`.
 */
export function TabStrip({
  tabs,
  activeKey,
  canClose,
  onActivate,
  onClose,
  onRename,
  onShare,
  onNew,
  drag,
  leading,
  actions,
}: {
  tabs: TabInfo[];
  activeKey: number;
  canClose: boolean;
  onActivate: (key: number) => void;
  onClose: (key: number) => void;
  onRename: (key: number, title: string) => void;
  /** Open the scoped-share dialog for a tab (only ever called for tabs whose
   *  shareScope is set). */
  onShare: (key: number) => void;
  onNew: (kind: NewTabKind) => void;
  drag: TabDrag;
  leading?: React.ReactNode;
  actions?: React.ReactNode;
}) {
  // The insertion point (which tab edge a drop would land on) and which tab is
  // being dragged, so we can paint the drop line and dim the source tab. Both
  // are purely visual — the actual reorder is committed on drop, in App.
  const [indicator, setIndicator] = useState<{ key: number; before: boolean } | null>(null);
  const [draggingKey, setDraggingKey] = useState<number | null>(null);

  const clearDrag = () => {
    setIndicator(null);
    setDraggingKey(null);
  };

  return (
    <div
      className="relative flex h-9 select-none items-stretch bg-bar"
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      }}
      onDragLeave={(e) => {
        // Only clear when the cursor actually leaves the strip, not when it
        // crosses between child tabs (relatedTarget still inside the strip).
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
          setIndicator(null);
        }
      }}
      onDrop={(e) => {
        e.preventDefault();
        drag.onDropStrip();
        clearDrag();
      }}
    >
      {leading && <div className="flex items-center gap-0.5 px-2">{leading}</div>}
      <div role="tablist" className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
        {tabs.map((tab) => (
          <Tab
            key={tab.key}
            tab={tab}
            active={tab.key === activeKey}
            closable={canClose}
            dragging={tab.key === draggingKey}
            indicator={
              indicator?.key === tab.key ? (indicator.before ? "before" : "after") : null
            }
            onActivate={() => onActivate(tab.key)}
            onClose={() => onClose(tab.key)}
            onRename={(title) => onRename(tab.key, title)}
            onShare={() => onShare(tab.key)}
            onDragStart={() => {
              setDraggingKey(tab.key);
              drag.onStart(tab.key);
            }}
            onDragOverTab={(before) => setIndicator({ key: tab.key, before })}
            onDropTab={(before) => {
              drag.onDropTab(tab.key, before);
              clearDrag();
            }}
            onDragEnd={() => {
              drag.onEnd();
              clearDrag();
            }}
          />
        ))}
        <div className="flex items-center px-1.5">
          <NewTabMenu onNew={onNew} />
        </div>
      </div>
      {actions && <div className="flex items-center gap-0.5 px-2">{actions}</div>}
    </div>
  );
}

/**
 * The once-per-window action cluster: the two split actions (each divides
 * the FOCUSED pane, editor-style — splits nest) and the theme picker. Lives
 * on the top-right strip, beside the history and activity icons. A pane
 * closes by closing its last tab. Tab types (terminal, files, browser,
 * messenger) open from each strip's + menu.
 */
export function GlobalActions({ onSplit }: { onSplit: (dir: SplitDir) => void }) {
  return (
    <>
      <ShareButton />
      <IconButton title="Split pane right" onClick={() => onSplit("right")}>
        <Columns2 size={13} />
      </IconButton>
      <IconButton title="Split pane down" onClick={() => onSplit("down")}>
        <Rows2 size={13} />
      </IconButton>
      <ThemePicker />
    </>
  );
}

/**
 * The share affordance: mints a one-time login link for this agent and shows
 * it in a popover, copied to the clipboard. Links are independent — minting
 * one never kills an earlier unredeemed one or the console link — and each
 * dies on first use or after 24 hours unused. On hosts without an auth gate
 * (loopback-only bind) the endpoint 404s and the popover explains instead.
 * Clipboard state is tri-valued so a blocked clipboard (plain-http origin)
 * is said out loud rather than quietly not copying.
 */
function ShareButton() {
  const [open, setOpen] = useState(false);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<boolean | null>(null); // null = not attempted
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

  const copy = async (link: string) => {
    try {
      await navigator.clipboard.writeText(link);
      setCopied(true);
    } catch {
      setCopied(false); // clipboard blocked (e.g. plain-http origin) — say so below
    }
  };

  const toggle = async () => {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    setUrl(null);
    setError(null);
    setCopied(null);
    try {
      const link = await createShareLink();
      setUrl(link);
      await copy(link);
    } catch {
      setError(
        "Sharing needs a remotely reachable arbos (a forest join or a non-loopback bind).",
      );
    }
  };

  return (
    <div ref={rootRef} className="relative flex">
      <IconButton title="Share agent" pressed={open} onClick={() => void toggle()}>
        <Share2 size={13} />
      </IconButton>
      {open && (
        <div className="absolute right-0 top-7 z-50 flex w-72 flex-col gap-2 rounded-lg border border-line bg-card p-3 shadow-xl shadow-black/40">
          <div className="text-[12px] font-semibold text-bright">Share this agent</div>
          {error ? (
            <div className="text-[12px] text-muted">{error}</div>
          ) : !url ? (
            <div className="flex items-center gap-2 text-[12px] text-muted">
              <Loader2 size={12} className="animate-spin" /> Minting link…
            </div>
          ) : (
            <>
              <div className="flex items-center gap-1.5">
                <input
                  readOnly
                  value={url}
                  onFocus={(e) => e.currentTarget.select()}
                  className="min-w-0 flex-1 rounded-md border border-line bg-canvas px-2 py-1 text-[11.5px] text-text outline-none"
                />
                <IconButton
                  title={copied ? "Copied" : "Copy link"}
                  onClick={() => void copy(url)}
                >
                  {copied ? <Check size={13} /> : <Copy size={13} />}
                </IconButton>
              </div>
              {copied === false && (
                <div className="text-[11.5px] text-text">
                  Clipboard is blocked here — select the link above and copy it
                  manually.
                </div>
              )}
              <div className="text-[11.5px] text-muted">
                {copied ? "Copied. " : ""}Logs in one browser, then expires;
                unused links die after 24 h. Safe to paste in chat — previews
                can’t consume it. Share again for another.
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}

const NEW_TAB_ITEMS: { kind: NewTabKind; label: string; icon: React.ReactNode }[] = [
  { kind: "chat", label: "New agent", icon: <MessageSquare size={13} /> },
  { kind: "terminal", label: "Terminal", icon: <SquareTerminal size={13} /> },
  { kind: "files", label: "Files", icon: <FolderTree size={13} /> },
  { kind: "browser", label: "Browser", icon: <Globe size={13} /> },
  { kind: "messenger", label: "Messenger", icon: <Send size={13} /> },
];

/**
 * The strip's + button: drops a menu of tab types instead of opening a chat
 * straight away. Mirrors the theme picker's click-away behavior. The menu is
 * fixed-positioned from the button's rect — the tablist it lives in scrolls
 * horizontally (overflow-x: auto), which would clip an absolute child.
 */
function NewTabMenu({ onNew }: { onNew: (kind: NewTabKind) => void }) {
  const [menuAt, setMenuAt] = useState<{ left: number; top: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuAt) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setMenuAt(null);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [menuAt]);

  const toggle = () => {
    if (menuAt) {
      setMenuAt(null);
      return;
    }
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;
    // Keep the menu (w-44 = 176px) inside the viewport's right edge.
    setMenuAt({
      left: Math.min(rect.left, window.innerWidth - 184),
      top: rect.bottom + 4,
    });
  };

  return (
    <div ref={rootRef} className="relative flex">
      <IconButton title="New tab" pressed={menuAt !== null} onClick={toggle}>
        <Plus size={14} />
      </IconButton>
      {menuAt && (
        <div
          style={{ left: menuAt.left, top: menuAt.top }}
          className="fixed z-50 flex w-44 flex-col rounded-lg border border-line bg-card py-1 shadow-xl shadow-black/40"
        >
          {NEW_TAB_ITEMS.map((item) => (
            <button
              key={item.kind}
              type="button"
              onClick={() => {
                setMenuAt(null);
                onNew(item.kind);
              }}
              className="flex w-full cursor-pointer items-center gap-2.5 px-3 py-1.5 text-left text-[12.5px] text-text transition-colors hover:bg-hover"
            >
              <span className="shrink-0 text-muted">{item.icon}</span>
              <span className="min-w-0 flex-1 truncate">{item.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** Once-per-window icons opening (or focusing) their singleton tab. History
 *  and activity sit on the top-right strip; settings on the top-left. */
export function HistoryButton({ onOpen }: { onOpen: () => void }) {
  return (
    <IconButton title="History" onClick={onOpen}>
      <Clock size={13} />
    </IconButton>
  );
}

export function ActivityButton({ onOpen }: { onOpen: () => void }) {
  return (
    <IconButton title="Agent activity" onClick={onOpen}>
      <Activity size={13} />
    </IconButton>
  );
}

export function SettingsButton({ onOpen }: { onOpen: () => void }) {
  return (
    <IconButton title="Settings" onClick={onOpen}>
      <Settings size={13} />
    </IconButton>
  );
}

function IconButton({
  title,
  onClick,
  pressed,
  children,
}: {
  title: string;
  onClick: () => void;
  pressed?: boolean;
  children: React.ReactNode;
}) {
  return (
    <Tooltip label={title}>
      <button
        type="button"
        aria-label={title}
        onClick={onClick}
        className={`flex size-6 cursor-pointer items-center justify-center rounded-md transition-colors hover:bg-hover ${
          pressed ? "bg-hover text-text" : "text-muted hover:text-text"
        }`}
      >
        {children}
      </button>
    </Tooltip>
  );
}

function Tab({
  tab,
  active,
  closable,
  dragging,
  indicator,
  onActivate,
  onClose,
  onRename,
  onShare,
  onDragStart,
  onDragOverTab,
  onDropTab,
  onDragEnd,
}: {
  tab: TabInfo;
  active: boolean;
  closable: boolean;
  /** This tab is the one currently being dragged (dim it). */
  dragging: boolean;
  /** Paint a drop line on this tab's left ("before") or right ("after") edge. */
  indicator: "before" | "after" | null;
  onActivate: () => void;
  onClose: () => void;
  onRename: (title: string) => void;
  onShare: () => void;
  onDragStart: () => void;
  /** Fired while a drag hovers this tab; before = cursor on the left half. */
  onDragOverTab: (before: boolean) => void;
  /** Fired when a tab is dropped on this tab; before = left half. */
  onDropTab: (before: boolean) => void;
  onDragEnd: () => void;
}) {
  // Double-click a tab to rename it: the label becomes an input seeded with the
  // current title. Enter / blur commits a non-empty trim; Escape reverts.
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useLayoutEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const startEdit = () => {
    setDraft(tab.title ?? "");
    setEditing(true);
  };

  const commit = () => {
    const next = draft.trim();
    if (next) onRename(next);
    setEditing(false);
  };

  return (
    <div
      role="tab"
      aria-selected={active}
      aria-label={tab.title || "New Agent"}
      draggable={!editing}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", ""); // Firefox needs payload
        onDragStart();
      }}
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        const rect = e.currentTarget.getBoundingClientRect();
        onDragOverTab(e.clientX < rect.left + rect.width / 2);
      }}
      onDrop={(e) => {
        // Commit the move next to this tab, and keep the strip's background
        // drop from ALSO appending it to the end of the pane.
        e.preventDefault();
        e.stopPropagation();
        const rect = e.currentTarget.getBoundingClientRect();
        onDropTab(e.clientX < rect.left + rect.width / 2);
      }}
      onDragEnd={onDragEnd}
      onClick={onActivate}
      onDoubleClick={startEdit}
      title={tab.title || "New Agent"}
      // min-w keeps a readable label floor when the strip is crowded — the
      // tablist scrolls horizontally past that point instead of crushing
      // every title down to one character.
      className={`group flex min-w-[88px] max-w-[180px] cursor-pointer items-center gap-1.5 border-r border-line/40 px-3 text-[12px] ${
        active ? "bg-canvas text-bright" : "text-muted hover:text-text"
      } ${dragging ? "opacity-40" : ""} ${
        // The drop line is an inset shadow, not an absolutely-positioned child:
        // a `position: relative` flex item breaks the native drag image here.
        indicator === "before"
          ? "shadow-[inset_2px_0_0_0_var(--color-bright)]"
          : indicator === "after"
            ? "shadow-[inset_-2px_0_0_0_var(--color-bright)]"
            : ""
      }`}
    >
      {tab.busy ? (
        <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
      ) : tab.kind === "surface" ? (
        <AppWindow size={11} className="shrink-0 text-faint" />
      ) : tab.kind === "run" ? (
        <Orbit size={11} className="shrink-0 text-faint" />
      ) : tab.kind === "terminal" ? (
        <SquareTerminal size={11} className="shrink-0 text-faint" />
      ) : tab.kind === "activity" ? (
        <Activity size={11} className="shrink-0 text-faint" />
      ) : tab.kind === "messenger" ? (
        <Send size={11} className="shrink-0 text-faint" />
      ) : tab.kind === "settings" ? (
        <Settings size={11} className="shrink-0 text-faint" />
      ) : (
        <MessageSquare size={11} className="shrink-0 text-faint" />
      )}
      {editing ? (
        <input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onClick={(e) => e.stopPropagation()}
          onDoubleClick={(e) => e.stopPropagation()}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              setEditing(false);
            }
          }}
          className="min-w-0 flex-1 bg-transparent text-bright outline-none"
        />
      ) : (
        <span className="min-w-0 flex-1 truncate">{tab.title || "New Agent"}</span>
      )}
      {tab.shareScope && !editing && (
        <button
          type="button"
          title="Share"
          onClick={(e) => {
            e.stopPropagation();
            onShare();
          }}
          className={`shrink-0 cursor-pointer rounded p-0.5 text-faint transition-opacity hover:bg-hover hover:text-text ${
            active ? "" : "opacity-0 group-hover:opacity-100"
          }`}
        >
          <Share2 size={11} />
        </button>
      )}
      {closable && !editing && (
        <button
          type="button"
          title="Close"
          onClick={(e) => {
            e.stopPropagation();
            onClose();
          }}
          className={`shrink-0 cursor-pointer rounded p-0.5 text-faint transition-opacity hover:bg-hover hover:text-text ${
            active ? "" : "opacity-0 group-hover:opacity-100"
          }`}
        >
          <X size={11} />
        </button>
      )}
    </div>
  );
}

