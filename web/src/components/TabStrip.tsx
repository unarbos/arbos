import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Activity,
  AppWindow,
  Clock,
  Columns2,
  Loader2,
  MessageSquare,
  Plus,
  Rows2,
  X,
} from "lucide-react";

import { fetchSessions, type SessionSummary } from "@/lib/api";
import type { SplitDir } from "@/lib/layout";
import { ThemePicker } from "./ThemePicker";

export interface TabInfo {
  key: number;
  title: string | null;
  busy: boolean;
  /** What the tab holds: an agent chat (default) or an opened surface. */
  kind?: "chat" | "surface";
}

/**
 * Drag wiring shared across strips (the drag state lives in App, so a tab can
 * be dragged from one pane's strip into the other's).
 */
export interface TabDrag {
  onStart: (key: number) => void;
  /** Fired while a drag hovers a tab; before = cursor on the left half. */
  onOverTab: (key: number, before: boolean) => void;
  onEnd: () => void;
  /** A tab dropped on the strip's empty background: append to this pane. */
  onDropStrip: () => void;
}

/**
 * Cursor's tab strip, one per pane: a tab per open agent and a + for a fresh
 * chat in this pane. Global actions (history, activity, split, theme) render
 * once, on the top-right strip, via `actions`.
 */
export function TabStrip({
  tabs,
  activeKey,
  canClose,
  onActivate,
  onClose,
  onRename,
  onNew,
  drag,
  actions,
}: {
  tabs: TabInfo[];
  activeKey: number;
  canClose: boolean;
  onActivate: (key: number) => void;
  onClose: (key: number) => void;
  onRename: (key: number, title: string) => void;
  onNew: () => void;
  drag: TabDrag;
  actions?: React.ReactNode;
}) {
  return (
    <div
      className="relative flex h-9 select-none items-stretch bg-bar"
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      }}
      onDrop={(e) => {
        e.preventDefault();
        drag.onDropStrip();
      }}
    >
      <div role="tablist" className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
        {tabs.map((tab) => (
          <Tab
            key={tab.key}
            tab={tab}
            active={tab.key === activeKey}
            closable={canClose}
            onActivate={() => onActivate(tab.key)}
            onClose={() => onClose(tab.key)}
            onRename={(title) => onRename(tab.key, title)}
            onDragStart={() => drag.onStart(tab.key)}
            onDragOverTab={(before) => drag.onOverTab(tab.key, before)}
            onDragEnd={drag.onEnd}
          />
        ))}
        <div className="flex items-center px-1.5">
          <IconButton title="New chat" onClick={onNew}>
            <Plus size={14} />
          </IconButton>
        </div>
      </div>
      {actions && <div className="flex items-center gap-0.5 px-2">{actions}</div>}
    </div>
  );
}

/**
 * The once-per-window action cluster: session history, agent activity, the
 * two split actions (each divides the FOCUSED pane, editor-style — splits
 * nest), and the theme picker. Lives on the top-right strip. A pane closes
 * by closing its last tab.
 */
export function GlobalActions({
  onOpenSession,
  activityOpen,
  onToggleActivity,
  onSplit,
}: {
  onOpenSession: (s: SessionSummary) => void;
  activityOpen: boolean;
  onToggleActivity: () => void;
  onSplit: (dir: SplitDir) => void;
}) {
  const [historyOpen, setHistoryOpen] = useState(false);

  return (
    <>
      <IconButton
        title="History"
        onClick={() => setHistoryOpen((v) => !v)}
        pressed={historyOpen}
      >
        <Clock size={13} />
      </IconButton>
      <IconButton
        title="Agent activity"
        onClick={onToggleActivity}
        pressed={activityOpen}
      >
        <Activity size={13} />
      </IconButton>
      <IconButton title="Split pane right" onClick={() => onSplit("right")}>
        <Columns2 size={13} />
      </IconButton>
      <IconButton title="Split pane down" onClick={() => onSplit("down")}>
        <Rows2 size={13} />
      </IconButton>
      <ThemePicker />
      {historyOpen && (
        <HistoryDropdown
          onPick={(s) => {
            setHistoryOpen(false);
            onOpenSession(s);
          }}
          onDismiss={() => setHistoryOpen(false)}
        />
      )}
    </>
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
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={`flex size-6 cursor-pointer items-center justify-center rounded-md transition-colors hover:bg-hover ${
        pressed ? "bg-hover text-text" : "text-muted hover:text-text"
      }`}
    >
      {children}
    </button>
  );
}

function Tab({
  tab,
  active,
  closable,
  onActivate,
  onClose,
  onRename,
  onDragStart,
  onDragOverTab,
  onDragEnd,
}: {
  tab: TabInfo;
  active: boolean;
  closable: boolean;
  onActivate: () => void;
  onClose: () => void;
  onRename: (title: string) => void;
  onDragStart: () => void;
  /** Fired while a drag hovers this tab; before = cursor on the left half. */
  onDragOverTab: (before: boolean) => void;
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
        // The hover already live-reordered next to this tab; don't let the
        // strip's background drop ALSO append it to the end of the pane.
        e.preventDefault();
        e.stopPropagation();
      }}
      onDragEnd={onDragEnd}
      onClick={onActivate}
      onDoubleClick={startEdit}
      className={`group flex min-w-0 max-w-[180px] cursor-pointer items-center gap-1.5 border-r border-line/40 px-3 text-[12px] ${
        active ? "bg-canvas text-bright" : "text-muted hover:text-text"
      }`}
    >
      {tab.busy ? (
        <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
      ) : tab.kind === "surface" ? (
        <AppWindow size={11} className="shrink-0 text-faint" />
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

/** Start of the local "today" for the Today / Older grouping. */
function startOfToday(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function HistoryDropdown({
  onPick,
  onDismiss,
}: {
  onPick: (s: SessionSummary) => void;
  onDismiss: () => void;
}) {
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [query, setQuery] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    fetchSessions().then(setSessions).catch(() => setSessions([]));
  }, []);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        onDismiss();
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [onDismiss]);

  const q = query.trim().toLowerCase();
  const visible = (sessions ?? []).filter(
    (s) => !q || (s.title || s.id).toLowerCase().includes(q),
  );
  const today = startOfToday();
  const groups: [string, SessionSummary[]][] = [
    ["Today", visible.filter((s) => s.updated_at >= today)],
    ["Older", visible.filter((s) => s.updated_at < today)],
  ];

  return (
    <div
      ref={rootRef}
      className="absolute right-2 top-9 z-20 flex max-h-96 w-72 flex-col overflow-hidden rounded-lg border border-line bg-card shadow-xl shadow-black/40"
    >
      <input
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onDismiss()}
        placeholder="Search Agents…"
        autoFocus
        className="border-b border-line/60 bg-transparent px-3 py-2 text-[12.5px] text-bright outline-none placeholder:text-faint"
      />
      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {sessions === null && (
          <div className="px-3 py-2 text-[12px] text-faint">Loading…</div>
        )}
        {sessions !== null && visible.length === 0 && (
          <div className="px-3 py-2 text-[12px] text-faint">No agents found</div>
        )}
        {groups.map(([label, items]) =>
          items.length === 0 ? null : (
            <div key={label}>
              <div className="px-3 pb-0.5 pt-1.5 text-[11px] text-faint">
                {label}
              </div>
              {items.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  onClick={() => onPick(s)}
                  className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-[12.5px] text-text transition-colors hover:bg-hover"
                >
                  <MessageSquare size={12} className="shrink-0 text-faint" />
                  <span className="min-w-0 flex-1 truncate">
                    {s.title || s.id}
                  </span>
                </button>
              ))}
            </div>
          ),
        )}
      </div>
    </div>
  );
}
