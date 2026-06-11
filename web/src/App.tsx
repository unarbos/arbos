import { useCallback, useRef, useState } from "react";

import { ActivityPanel } from "./components/ActivityPanel";
import { ChatTab } from "./components/ChatTab";
import { TopBar, type TabInfo } from "./components/TopBar";
import type { SessionSummary } from "./lib/api";

interface TabState extends TabInfo {
  /** Session to resume (from history); null opens fresh. */
  resumeId: string | null;
  /** Bound session id once the seam assigns/confirms one. */
  sessionId: string | null;
}

const TAB_TITLE_MAX = 40;

function tabTitle(text: string): string {
  const line = text.split("\n")[0].trim();
  return line.length > TAB_TITLE_MAX ? line.slice(0, TAB_TITLE_MAX) + "…" : line;
}

/**
 * The agent panel: a Cursor-style tab strip over independent chat tabs. Every
 * tab is its own seam connection, so agents run concurrently; switching tabs
 * is pure UI. History opens past sessions into new tabs.
 */
export default function App() {
  const nextKey = useRef(2);
  const [tabs, setTabs] = useState<TabState[]>([
    { key: 1, title: null, busy: false, resumeId: null, sessionId: null },
  ]);
  const [activeKey, setActiveKey] = useState(1);
  const [activityOpen, setActivityOpen] = useState(false);

  const patchTab = useCallback((key: number, patch: Partial<TabState>) => {
    setTabs((ts) => ts.map((t) => (t.key === key ? { ...t, ...patch } : t)));
  }, []);

  const newTab = useCallback((resume?: SessionSummary) => {
    const key = nextKey.current++;
    setTabs((ts) => [
      ...ts,
      {
        key,
        title: resume ? resume.title || resume.id : null,
        busy: false,
        resumeId: resume?.id ?? null,
        sessionId: resume?.id ?? null,
      },
    ]);
    setActiveKey(key);
  }, []);

  const closeTab = useCallback(
    (key: number) => {
      setTabs((ts) => {
        const rest = ts.filter((t) => t.key !== key);
        if (rest.length === 0) {
          const fresh = {
            key: nextKey.current++,
            title: null,
            busy: false,
            resumeId: null,
            sessionId: null,
          };
          setActiveKey(fresh.key);
          return [fresh];
        }
        setActiveKey((active) =>
          active === key ? rest[rest.length - 1].key : active,
        );
        return rest;
      });
    },
    [],
  );

  const openSession = useCallback(
    (s: SessionSummary) => {
      // Already open in a tab? Focus it instead of double-binding the session.
      const existing = tabs.find((t) => t.sessionId === s.id);
      if (existing) {
        setActiveKey(existing.key);
        return;
      }
      newTab(s);
    },
    [tabs, newTab],
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
      <TopBar
        tabs={tabs}
        activeKey={activeKey}
        onActivate={setActiveKey}
        onClose={closeTab}
        onNew={() => newTab()}
        onOpenSession={openSession}
        activityOpen={activityOpen}
        onToggleActivity={() => setActivityOpen((v) => !v)}
      />
      {tabs.map((tab) => (
        <ChatTab
          key={tab.key}
          active={tab.key === activeKey}
          resumeId={tab.resumeId}
          handle={{
            onBusy: (busy) => patchTab(tab.key, { busy }),
            onTitle: (t) => patchTab(tab.key, { title: tabTitle(t) }),
            onSession: (id) => patchTab(tab.key, { sessionId: id }),
          }}
        />
      ))}
      {activityOpen && (
        <ActivityPanel
          onOpenChat={(chat) => {
            setActivityOpen(false);
            openSession({ id: chat, title: "", updated_at: 0 });
          }}
          onClose={() => setActivityOpen(false)}
        />
      )}
    </div>
  );
}
