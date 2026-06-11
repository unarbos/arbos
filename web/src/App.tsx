import { useCallback, useRef, useState } from "react";

import { ActivityPanel } from "./components/ActivityPanel";
import { ChatTab } from "./components/ChatTab";
import { RunView, type RunRef } from "./components/RunView";
import { SurfaceView } from "./components/SurfaceView";
import { TerminalView } from "./components/TerminalView";
import { GlobalActions, TabStrip, type TabInfo } from "./components/TabStrip";
import { closeTerminal, createTerminal, type SessionSummary } from "./lib/api";
import {
  computePlaces,
  panesOf,
  removePane,
  setSplitRatio,
  splitPane,
  type DividerPlace,
  type LayoutNode,
  type SplitDir,
} from "./lib/layout";
import { surfaceTitle, type Surface } from "./lib/surface";
import { sameTerm, termTitle, type TermRef } from "./lib/term";

interface TabState extends TabInfo {
  /** Session to resume (from history); null opens fresh. */
  resumeId: string | null;
  /** Bound session id once the seam assigns/confirms one. */
  sessionId: string | null;
  /** User renamed this tab — the first-prompt auto-title won't override it. */
  titlePinned: boolean;
  /** The pane (split-tree leaf) this tab lives in. */
  pane: number;
  /** kind "surface": the opened artifact this tab renders instead of a chat. */
  surface?: Surface;
  /** kind "run": the sub-agent run this tab watches instead of a chat. */
  run?: RunRef;
  /** kind "terminal": the job or shell this tab is a window onto. */
  term?: TermRef;
}

/**
 * The whole window in one value, so every mutation can uphold the invariants
 * in one place (see normalize): every pane in the tree has at least one tab,
 * and focus points at a pane that exists.
 */
interface Layout {
  root: LayoutNode;
  tabs: TabState[];
  /** Preferred active tab per pane; falls back to the pane's last tab. */
  paneActive: Record<number, number>;
  /** The pane that owns the keyboard: ambient notices and new tabs land here. */
  focusedPane: number;
  /** Bumped on explicit activation so the focused chat refocuses its composer. */
  focusTick: number;
}

const TAB_TITLE_MAX = 40;
const RATIO_MIN = 0.2;
const RATIO_MAX = 0.8;
/** TabStrip height (h-9) — a pane is its strip plus the chat below it. */
const STRIP_PX = 36;

function tabTitle(text: string): string {
  const line = text.split("\n")[0].trim();
  return line.length > TAB_TITLE_MAX ? line.slice(0, TAB_TITLE_MAX) + "…" : line;
}

function freshTab(key: number, pane: number, resume?: SessionSummary): TabState {
  return {
    key,
    title: resume ? resume.title || resume.id : null,
    busy: false,
    resumeId: resume?.id ?? null,
    sessionId: resume?.id ?? null,
    titlePinned: false,
    pane,
  };
}

function surfaceTab(key: number, pane: number, surface: Surface): TabState {
  return {
    key,
    title: surfaceTitle(surface),
    busy: false,
    kind: "surface",
    surface,
    resumeId: null,
    sessionId: null,
    titlePinned: false,
    pane,
  };
}

function runTab(key: number, pane: number, run: RunRef): TabState {
  return {
    key,
    title: run.label,
    busy: false,
    kind: "run",
    run,
    resumeId: null,
    sessionId: null,
    titlePinned: false,
    pane,
  };
}

function termTab(key: number, pane: number, term: TermRef): TabState {
  return {
    key,
    title: termTitle(term),
    busy: false,
    kind: "terminal",
    term,
    resumeId: null,
    sessionId: null,
    titlePinned: false,
    pane,
  };
}

/**
 * Re-establish the layout invariants after any mutation: a pane left with no
 * tabs collapses out of the tree (its sibling inherits the space), and focus
 * snaps back to a real pane if its own vanished.
 */
function normalize(s: Layout): Layout {
  let root = s.root;
  for (const pane of panesOf(root)) {
    if (!s.tabs.some((t) => t.pane === pane)) {
      root = removePane(root, pane) ?? root;
    }
  }
  const panes = panesOf(root);
  const focusedPane = panes.includes(s.focusedPane) ? s.focusedPane : panes[0];
  if (root === s.root && focusedPane === s.focusedPane) return s;
  return { ...s, root, focusedPane };
}

/** The pane's visible tab: the preferred one if it's still here, else the last. */
function activeKeyIn(tabs: TabState[], pane: number, preferred: number | undefined): number {
  const own = tabs.filter((t) => t.pane === pane);
  if (preferred !== undefined && own.some((t) => t.key === preferred)) return preferred;
  return own.length > 0 ? own[own.length - 1].key : -1;
}

const pct = (f: number) => `${f * 100}%`;

/**
 * The agent panel: Cursor-style tab strips over independent chat tabs, in an
 * editor-style split tree — each split divides the FOCUSED pane right or
 * down, recursively, with a draggable divider per split. Every tab is its own
 * seam connection, so agents run concurrently; switching tabs and resizing
 * panes is pure UI. History opens past sessions into new tabs.
 *
 * Strips, chats, and dividers are all flat keyed children of one relative
 * container, absolutely positioned from the tree's rectangles. A chat never
 * changes parents when the tree restructures, so its connection survives
 * splits, merges, and cross-pane tab drags.
 */
export default function App() {
  const nextKey = useRef(2); // tab keys (tab 1 exists)
  const nextId = useRef(2); // pane / split ids (pane 1 exists)
  const [layout, setLayout] = useState<Layout>({
    root: { kind: "pane", id: 1 },
    tabs: [freshTab(1, 1)],
    paneActive: { 1: 1 },
    focusedPane: 1,
    focusTick: 0,
  });
  const [activityOpen, setActivityOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // Key of the tab being dragged; live-reorders (across panes too) as it
  // crosses neighbors' midpoints, so drop is just cleanup.
  const dragTab = useRef<number | null>(null);

  const patchTab = useCallback((key: number, patch: Partial<TabState>) => {
    setLayout((s) => ({
      ...s,
      tabs: s.tabs.map((t) => (t.key === key ? { ...t, ...patch } : t)),
    }));
  }, []);

  const newTab = useCallback((resume?: SessionSummary, pane?: number) => {
    setLayout((s) => {
      const into = pane ?? s.focusedPane;
      const tab = freshTab(nextKey.current++, into, resume);
      return normalize({
        ...s,
        tabs: [...s.tabs, tab],
        paneActive: { ...s.paneActive, [into]: tab.key },
        focusedPane: into,
        focusTick: s.focusTick + 1,
      });
    });
  }, []);

  const closeTab = useCallback((key: number) => {
    setLayout((s) => {
      // Closing a shell tab hangs up its PTY — the tab IS the shell's
      // lifetime. Job terminals are only windows; the job runs on.
      const closing = s.tabs.find((t) => t.key === key);
      if (closing?.kind === "terminal" && closing.term?.kind === "shell") {
        void closeTerminal(closing.term.id);
      }
      const rest = s.tabs.filter((t) => t.key !== key);
      if (rest.length === 0) {
        // The last chat anywhere: the window resets to one pane, one fresh tab.
        const pane = nextId.current++;
        const tab = freshTab(nextKey.current++, pane);
        return {
          root: { kind: "pane", id: pane },
          tabs: [tab],
          paneActive: { [pane]: tab.key },
          focusedPane: pane,
          focusTick: s.focusTick + 1,
        };
      }
      return normalize({ ...s, tabs: rest, focusTick: s.focusTick + 1 });
    });
  }, []);

  const activateTab = useCallback((key: number) => {
    setLayout((s) => {
      const tab = s.tabs.find((t) => t.key === key);
      if (!tab) return s;
      return {
        ...s,
        paneActive: { ...s.paneActive, [tab.pane]: key },
        focusedPane: tab.pane,
        focusTick: s.focusTick + 1,
      };
    });
  }, []);

  const renameTab = useCallback(
    (key: number, title: string) => {
      patchTab(key, { title, titlePinned: true });
    },
    [patchTab],
  );

  /** Drag-reorder: move fromKey before/after toKey, adopting its pane. The
   *  chats are keyed children of one container, so the move never unmounts a
   *  connection. */
  const moveTab = useCallback((fromKey: number, toKey: number, before: boolean) => {
    setLayout((s) => {
      const fromIdx = s.tabs.findIndex((t) => t.key === fromKey);
      if (fromIdx === -1) return s;
      const moved = s.tabs[fromIdx];
      const rest = s.tabs.filter((t) => t.key !== fromKey);
      const toIdx = rest.findIndex((t) => t.key === toKey);
      if (toIdx === -1) return s;
      const target = rest[toIdx];
      const insertAt = before ? toIdx : toIdx + 1;
      if (insertAt === fromIdx && moved.pane === target.pane) return s;
      rest.splice(
        insertAt,
        0,
        moved.pane === target.pane ? moved : { ...moved, pane: target.pane },
      );
      return normalize({
        ...s,
        tabs: rest,
        paneActive: { ...s.paneActive, [target.pane]: fromKey },
      });
    });
  }, []);

  /** A tab dropped on a strip's empty background lands at that pane's end. */
  const moveTabToPane = useCallback((key: number, pane: number) => {
    setLayout((s) => {
      const tab = s.tabs.find((t) => t.key === key);
      if (!tab) return s;
      const rest = s.tabs.filter((t) => t.key !== key);
      rest.push(tab.pane === pane ? tab : { ...tab, pane });
      return normalize({
        ...s,
        tabs: rest,
        paneActive: { ...s.paneActive, [pane]: key },
        focusedPane: pane,
        focusTick: s.focusTick + 1,
      });
    });
  }, []);

  /**
   * Editor-style split: the FOCUSED pane divides in two, the new pane opens a
   * fresh chat and takes focus. Splits nest — splitting an already-split
   * pane's half divides just that half. Panes close by closing their last
   * tab (normalize collapses the empty pane back into its sibling).
   */
  const splitFocused = useCallback((dir: SplitDir) => {
    setLayout((s) => {
      const splitId = nextId.current++;
      const paneId = nextId.current++;
      const tab = freshTab(nextKey.current++, paneId);
      return normalize({
        ...s,
        root: splitPane(s.root, s.focusedPane, dir, splitId, paneId),
        tabs: [...s.tabs, tab],
        paneActive: { ...s.paneActive, [paneId]: tab.key },
        focusedPane: paneId,
        focusTick: s.focusTick + 1,
      });
    });
  }, []);

  const setRatio = useCallback((split: number, ratio: number) => {
    setLayout((s) => ({ ...s, root: setSplitRatio(s.root, split, ratio) }));
  }, []);

  /**
   * Open (or refresh) a companion tab beside the chat that produced it —
   * the one mechanic every non-chat tab kind shares: re-use a matching open
   * tab (reference refreshed, raised to front); else the next pane over;
   * else a fresh split to the chat's right. The keyboard stays with the
   * chat — companions are something to look at, not type into (a shell the
   * user asks for takes focus separately, in newShellTab).
   */
  const openAside = useCallback(
    (
      fromKey: number,
      matches: (t: TabState) => boolean,
      refresh: (t: TabState) => TabState,
      make: (key: number, pane: number) => TabState,
    ) => {
      setLayout((s) => {
        const from = s.tabs.find((t) => t.key === fromKey);
        if (!from) return s;
        const existing = s.tabs.find(matches);
        if (existing) {
          return {
            ...s,
            tabs: s.tabs.map((t) => (t.key === existing.key ? refresh(t) : t)),
            paneActive: { ...s.paneActive, [existing.pane]: existing.key },
          };
        }
        const panes = panesOf(s.root);
        const beside = panes[panes.indexOf(from.pane) + 1];
        if (beside !== undefined) {
          const tab = make(nextKey.current++, beside);
          return {
            ...s,
            tabs: [...s.tabs, tab],
            paneActive: { ...s.paneActive, [beside]: tab.key },
          };
        }
        const splitId = nextId.current++;
        const paneId = nextId.current++;
        const tab = make(nextKey.current++, paneId);
        return {
          ...s,
          root: splitPane(s.root, from.pane, "right", splitId, paneId),
          tabs: [...s.tabs, tab],
          paneActive: { ...s.paneActive, [paneId]: tab.key },
        };
      });
    },
    [],
  );

  /** Show a surface (a shown file) beside the chat that produced it. */
  const openSurface = useCallback(
    (fromKey: number, surface: Surface) =>
      openAside(
        fromKey,
        (t) => t.kind === "surface" && t.surface?.path === surface.path,
        (t) => ({ ...t, surface, title: t.titlePinned ? t.title : surfaceTitle(surface) }),
        (key, pane) => surfaceTab(key, pane, surface),
      ),
    [openAside],
  );

  /**
   * Show a sub-agent run beside the chat that spawned it. The run tab polls
   * on its own, so it stays live however long the run takes — and survives
   * the parent chat's tab closing.
   */
  const openRun = useCallback(
    (fromKey: number, run: RunRef) =>
      openAside(
        fromKey,
        (t) => t.kind === "run" && t.run?.session === run.session,
        (t) => ({ ...t, run, title: t.titlePinned ? t.title : run.label }),
        (key, pane) => runTab(key, pane, run),
      ),
    [openAside],
  );

  /**
   * Show a terminal beside the chat: an agent command's live journal (a job
   * terminal, from its card or the background bar) or an interactive shell.
   * A job already open re-uses its tab — the same job seen from two cards is
   * one terminal.
   */
  const openTerminal = useCallback(
    (fromKey: number, term: TermRef) =>
      openAside(
        fromKey,
        (t) => t.kind === "terminal" && !!t.term && sameTerm(t.term, term),
        (t) => ({ ...t, term, title: t.titlePinned ? t.title : termTitle(term) }),
        (key, pane) => termTab(key, pane, term),
      ),
    [openAside],
  );

  /**
   * A fresh interactive shell, user-initiated: spawned on the host, opened
   * as a tab in the focused pane, and given the keyboard — unlike the
   * companion tabs, a shell the user asked for is something to type into.
   */
  const newShellTab = useCallback(() => {
    createTerminal()
      .then((info) => {
        setLayout((s) => {
          const into = s.focusedPane;
          const tab = termTab(nextKey.current++, into, {
            kind: "shell",
            id: info.id,
            cwd: info.cwd,
          });
          return normalize({
            ...s,
            tabs: [...s.tabs, tab],
            paneActive: { ...s.paneActive, [into]: tab.key },
            focusedPane: into,
            focusTick: s.focusTick + 1,
          });
        });
      })
      .catch(() => {});
  }, []);

  /** A job terminal's "shell here": continue the job's work by hand, in its
   * directory, in a live shell beside it. */
  const openShellBeside = useCallback(
    (fromKey: number, cwd?: string) => {
      createTerminal(cwd)
        .then((info) =>
          openTerminal(fromKey, { kind: "shell", id: info.id, cwd: info.cwd }),
        )
        .catch(() => {});
    },
    [openTerminal],
  );

  const focusPane = useCallback((pane: number) => {
    setLayout((s) => (s.focusedPane === pane ? s : { ...s, focusedPane: pane }));
  }, []);

  const openSession = useCallback(
    (summary: SessionSummary) => {
      // Already open in a tab? Focus it instead of double-binding the session.
      const existing = layout.tabs.find((t) => t.sessionId === summary.id);
      if (existing) {
        activateTab(existing.key);
        return;
      }
      newTab(summary);
    },
    [layout.tabs, activateTab, newTab],
  );

  const dragHandlers = (pane: number) => ({
    onStart: (key: number) => {
      dragTab.current = key;
    },
    onOverTab: (key: number, before: boolean) => {
      const from = dragTab.current;
      if (from !== null && from !== key) moveTab(from, key, before);
    },
    onEnd: () => {
      dragTab.current = null;
    },
    onDropStrip: () => {
      const from = dragTab.current;
      if (from !== null) moveTabToPane(from, pane);
      dragTab.current = null;
    },
  });

  const { tabs, root, paneActive, focusedPane, focusTick } = layout;
  const places = computePlaces(root);
  const paneRect = new Map(places.panes.map((p) => [p.pane, p.rect]));
  const activeKey = new Map(
    places.panes.map((p) => [p.pane, activeKeyIn(tabs, p.pane, paneActive[p.pane])]),
  );
  // Global actions live on whichever strip touches the window's top-right.
  const actionsPane = places.panes.reduce((best, p) =>
    p.rect.y < 0.001 && p.rect.x + p.rect.w > best.rect.x + best.rect.w ? p : best,
  ).pane;

  return (
    <div ref={rootRef} className="relative h-full overflow-hidden">
      {places.panes.map(({ pane, rect }) => (
        <div
          key={`strip-${pane}`}
          className="absolute"
          style={{
            left: pct(rect.x),
            top: pct(rect.y),
            width: pct(rect.w),
            height: STRIP_PX,
          }}
        >
          <TabStrip
            tabs={tabs.filter((t) => t.pane === pane)}
            activeKey={activeKey.get(pane) ?? -1}
            canClose={tabs.length > 1}
            onActivate={activateTab}
            onClose={closeTab}
            onRename={renameTab}
            onNew={() => newTab(undefined, pane)}
            drag={dragHandlers(pane)}
            actions={
              pane === actionsPane ? (
                <GlobalActions
                  onOpenSession={openSession}
                  activityOpen={activityOpen}
                  onToggleActivity={() => setActivityOpen((v) => !v)}
                  onSplit={splitFocused}
                  onNewTerminal={newShellTab}
                />
              ) : undefined
            }
          />
        </div>
      ))}

      {places.dividers.map((d) => (
        <Divider key={d.split} place={d} containerRef={rootRef} onRatio={setRatio} />
      ))}

      {tabs.map((tab) => {
        const rect = paneRect.get(tab.pane);
        const visible = rect !== undefined && tab.key === activeKey.get(tab.pane);
        return (
          <div
            key={tab.key}
            className={visible ? "absolute flex flex-col" : "hidden"}
            style={
              visible
                ? {
                    left: pct(rect.x),
                    top: `calc(${pct(rect.y)} + ${STRIP_PX}px)`,
                    width: pct(rect.w),
                    height: `calc(${pct(rect.h)} - ${STRIP_PX}px)`,
                  }
                : undefined
            }
            onPointerDownCapture={() => focusPane(tab.pane)}
          >
            {tab.kind === "surface" && tab.surface ? (
              <SurfaceView surface={tab.surface} active={visible} />
            ) : tab.kind === "terminal" && tab.term ? (
              <TerminalView
                term={tab.term}
                active={visible}
                onBusy={(busy) => patchTab(tab.key, { busy })}
                onOpenShell={(cwd) => openShellBeside(tab.key, cwd)}
              />
            ) : tab.kind === "run" && tab.run ? (
              <RunView
                run={tab.run}
                active={visible}
                onBusy={(busy) => patchTab(tab.key, { busy })}
              />
            ) : (
              <ChatTab
                active={visible}
                focused={visible && tab.pane === focusedPane}
                focusTick={focusTick}
                resumeId={tab.resumeId}
                handle={{
                  onBusy: (busy) => patchTab(tab.key, { busy }),
                  onTitle: (t) =>
                    setLayout((s) => ({
                      ...s,
                      tabs: s.tabs.map((x) =>
                        x.key === tab.key && !x.titlePinned
                          ? { ...x, title: tabTitle(t) }
                          : x,
                      ),
                    })),
                  onSession: (id) => patchTab(tab.key, { sessionId: id }),
                  onOpenSurface: (surface) => openSurface(tab.key, surface),
                  onOpenRun: (run) => openRun(tab.key, run),
                  onOpenTerminal: (term) => openTerminal(tab.key, term),
                }}
              />
            )}
          </div>
        );
      })}

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

/**
 * One split's seam: a 3px line over the boundary with a slightly wider grab
 * area. Dragging re-balances that split alone — the ratio is measured inside
 * the split's own rectangle, so nested splits resize independently.
 */
function Divider({
  place,
  containerRef,
  onRatio,
}: {
  place: DividerPlace;
  containerRef: React.RefObject<HTMLDivElement | null>;
  onRatio: (split: number, ratio: number) => void;
}) {
  const dragging = useRef(false);
  const vertical = place.dir === "right"; // a vertical seam between columns
  const line: React.CSSProperties = vertical
    ? {
        left: `calc(${pct(place.at)} - 1.5px)`,
        top: pct(place.rect.y),
        width: 3,
        height: pct(place.rect.h),
      }
    : {
        top: `calc(${pct(place.at)} - 1.5px)`,
        left: pct(place.rect.x),
        height: 3,
        width: pct(place.rect.w),
      };
  return (
    <div className="absolute z-10 bg-line" style={line}>
      <div
        className={`absolute touch-none transition-colors hover:bg-accent/50 active:bg-accent/70 ${
          vertical
            ? "-inset-x-1 inset-y-0 cursor-col-resize"
            : "inset-x-0 -inset-y-1 cursor-row-resize"
        }`}
        onPointerDown={(e) => {
          e.preventDefault();
          dragging.current = true;
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (!dragging.current || !containerRef.current) return;
          const c = containerRef.current.getBoundingClientRect();
          const frac = vertical
            ? ((e.clientX - c.left) / c.width - place.rect.x) / place.rect.w
            : ((e.clientY - c.top) / c.height - place.rect.y) / place.rect.h;
          onRatio(place.split, Math.min(RATIO_MAX, Math.max(RATIO_MIN, frac)));
        }}
        onPointerUp={() => {
          dragging.current = false;
        }}
      />
    </div>
  );
}
