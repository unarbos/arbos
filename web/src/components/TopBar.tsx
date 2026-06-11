import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Activity, Clock, Loader2, MessageSquare, Plus, X } from "lucide-react";

import { fetchSessions, type SessionSummary } from "@/lib/api";

export interface TabInfo {
  key: number;
  title: string | null;
  busy: boolean;
}

/**
 * Cursor's tab strip: one tab per open agent, a + for a fresh chat, and a
 * clock that drops down the session history (search, Today / Older groups).
 */
export function TopBar({
  tabs,
  activeKey,
  onActivate,
  onClose,
  onRename,
  onNew,
  onOpenSession,
  activityOpen,
  onToggleActivity,
}: {
  tabs: TabInfo[];
  activeKey: number;
  onActivate: (key: number) => void;
  onClose: (key: number) => void;
  onRename: (key: number, title: string) => void;
  onNew: () => void;
  onOpenSession: (s: SessionSummary) => void;
  activityOpen: boolean;
  onToggleActivity: () => void;
}) {
  const [historyOpen, setHistoryOpen] = useState(false);

  return (
    <div className="relative shrink-0 select-none bg-bar">
      <div className="flex h-9 items-stretch">
        <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
          {tabs.map((tab) => (
            <Tab
              key={tab.key}
              tab={tab}
              active={tab.key === activeKey}
              closable={tabs.length > 1}
              onActivate={() => onActivate(tab.key)}
              onClose={() => onClose(tab.key)}
            />
          ))}
        </div>
        <div className="flex items-center gap-0.5 px-2">
          <IconButton title="New chat" onClick={onNew}>
            <Plus size={14} />
          </IconButton>
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
        </div>
      </div>
      {historyOpen && (
        <HistoryDropdown
          onPick={(s) => {
            setHistoryOpen(false);
            onOpenSession(s);
          }}
          onDismiss={() => setHistoryOpen(false)}
        />
      )}
    </div>
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
      className={`flex size-6 cursor-pointer items-center justify-center rounded-md transition-colors hover:bg-white/[0.06] ${
        pressed ? "bg-white/[0.06] text-text" : "text-muted hover:text-text"
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
}: {
  tab: TabInfo;
  active: boolean;
  closable: boolean;
  onActivate: () => void;
  onClose: () => void;
}) {
  return (
    <div
      onClick={onActivate}
      className={`group flex min-w-0 max-w-[180px] cursor-pointer items-center gap-1.5 border-r border-line/40 px-3 text-[12px] ${
        active ? "bg-canvas text-bright" : "text-muted hover:text-text"
      }`}
    >
      {tab.busy ? (
        <Loader2 size={11} className="shrink-0 animate-spin text-faint" />
      ) : (
        <MessageSquare size={11} className="shrink-0 text-faint" />
      )}
      <span className="min-w-0 flex-1 truncate">{tab.title || "New Agent"}</span>
      {closable && (
        <button
          type="button"
          title="Close"
          onClick={(e) => {
            e.stopPropagation();
            onClose();
          }}
          className={`shrink-0 cursor-pointer rounded p-0.5 text-faint transition-opacity hover:bg-white/[0.08] hover:text-text ${
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
                  className="flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-[12.5px] text-text transition-colors hover:bg-white/[0.05]"
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
