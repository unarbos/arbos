import { useEffect, useState } from "react";
import { MessageSquare, Search } from "lucide-react";

import { fetchSessions, type SessionSummary } from "@/lib/api";
import { useDocumentVisible } from "@/lib/useDocumentVisible";

const SESSIONS_POLL_MS = 5000;

/** Start of the local "today" for the Today / Older grouping. */
function startOfToday(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function age(ms: number): string {
  const sec = Math.max(0, (Date.now() - ms) / 1000);
  if (sec < 60) return "just now";
  if (sec < 3600) return `${Math.round(sec / 60)}m ago`;
  if (sec < 86400) return `${Math.round(sec / 3600)}h ago`;
  return `${Math.round(sec / 86400)}d ago`;
}

/**
 * Session history as a tab: every past agent, searchable, grouped Today /
 * Older. Picking one opens (or focuses) its chat tab — this view stays open,
 * a place you return to. The list re-fetches while the tab is visible so new
 * sessions appear as they're created.
 */
export function HistoryView({
  active,
  onOpenSession,
  sessions: provided,
}: {
  /** Visible in its pane — list polling pauses while hidden. */
  active: boolean;
  onOpenSession: (s: SessionSummary) => void;
  /** A fixed list to render instead of fetching the host's sessions. The
   *  share view passes just the one shared chat — a guest's history is exactly
   *  what they were granted, never a window onto the host's other agents. */
  sessions?: SessionSummary[];
}) {
  const [fetched, setFetched] = useState<SessionSummary[] | null>(null);
  const [query, setQuery] = useState("");
  const docVisible = useDocumentVisible();

  useEffect(() => {
    // A provided list is authoritative — never poll the host-wide endpoint
    // (it's 403 for a share guest anyway).
    if (provided || !active || !docVisible) return;
    let stop = false;
    const load = () => {
      fetchSessions()
        .then((s) => {
          if (!stop) setFetched(s);
        })
        .catch(() => {});
    };
    load();
    const id = window.setInterval(load, SESSIONS_POLL_MS);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, docVisible, provided]);

  const sessions = provided ?? fetched;

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
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-2xl px-4 py-4">
          <div className="mb-3 flex items-center gap-2 rounded-md border border-line/60 px-2.5 py-1.5">
            <Search size={12} className="shrink-0 text-faint" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search Agents…"
              className="min-w-0 flex-1 bg-transparent text-[12.5px] text-bright outline-none placeholder:text-faint"
            />
          </div>

          {sessions === null && <div className="text-faint">Loading…</div>}
          {sessions !== null && visible.length === 0 && (
            <div className="text-[12px] text-faint">No agents found</div>
          )}
          {groups.map(([label, items]) =>
            items.length === 0 ? null : (
              <div key={label} className="mb-4">
                <div className="mb-1.5 text-[11px] uppercase tracking-wider text-faint select-none">
                  {label}
                </div>
                <div className="space-y-0.5">
                  {items.map((s) => (
                    <button
                      key={s.id}
                      type="button"
                      onClick={() => onOpenSession(s)}
                      className="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12.5px] text-text transition-colors hover:bg-hover"
                    >
                      <MessageSquare size={12} className="shrink-0 text-faint" />
                      <span className="min-w-0 flex-1 truncate">
                        {s.title || s.id}
                      </span>
                      <span className="shrink-0 text-[11px] text-faint">
                        {age(s.updated_at)}
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            ),
          )}
        </div>
      </div>
    </div>
  );
}
