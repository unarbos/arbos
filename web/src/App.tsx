import { Suspense, useCallback, useEffect, useRef, useState } from "react";

import { ActivityHistoryView } from "./components/ActivityHistoryView";
import { ChatTab } from "./components/ChatTab";
import { RunView, type RunRef } from "./components/RunView";
import { lazyPanel } from "./lib/lazyPanel";

// Code-split the heavy companion-tab bodies out of the main bundle: xterm
// (TerminalView), the surface viewers (SurfaceView), and the settings panel
// load on first use instead of on every cold start. Chat is the start page,
// so its path stays eager.
const MessengerView = lazyPanel(() =>
  import("./components/MessengerView").then((m) => ({ default: m.MessengerView })),
);
const SettingsView = lazyPanel(() =>
  import("./components/SettingsView").then((m) => ({ default: m.SettingsView })),
);
import type { SettingsPage } from "./components/SettingsView";
const SurfaceView = lazyPanel(() =>
  import("./components/SurfaceView").then((m) => ({ default: m.SurfaceView })),
);
const PlanView = lazyPanel(() =>
  import("./components/PlanView").then((m) => ({ default: m.PlanView })),
);
const TerminalView = lazyPanel(() =>
  import("./components/TerminalView").then((m) => ({ default: m.TerminalView })),
);
import {
  ActivityButton,
  GlobalActions,
  HistoryButton,
  SettingsButton,
  TabStrip,
  type NewTabKind,
  type TabInfo,
} from "./components/TabStrip";
import {
  closeTerminal,
  createTerminal,
  createBrowserTab,
  fetchLLM,
  listBrowserTabs,
  type SessionSummary,
} from "./lib/api";
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
import { dirSurface, surfaceTitle, type Surface, type UICommand } from "./lib/surface";
import { sameTerm, termTitle, type TermRef } from "./lib/term";
import { errMsg, toastError } from "./lib/toast";
import { Toasts } from "./components/Toasts";

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
  /** kind "messenger": the connected bot this tab is the chat for
   *  (undefined until its token is entered). */
  messengerBot?: number;
  /** kind "plan": a node in the plan this tab shows the goal tree for. */
  planNode?: number;
  /** kind "settings": the page the panel opens on (undefined → General). */
  settingsPage?: SettingsPage;
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

function activityTab(key: number, pane: number): TabState {
  return {
    key,
    title: "Activity",
    busy: false,
    kind: "activity",
    resumeId: null,
    sessionId: null,
    titlePinned: false,
    pane,
  };
}

function planTab(key: number, pane: number, node: number): TabState {
  return {
    key,
    title: `Plan #${node}`,
    busy: false,
    kind: "plan",
    planNode: node,
    resumeId: null,
    sessionId: null,
    titlePinned: false,
    pane,
  };
}

function settingsTab(key: number, pane: number, page?: SettingsPage): TabState {
  return {
    key,
    title: "Settings",
    busy: false,
    kind: "settings",
    resumeId: null,
    sessionId: null,
    titlePinned: false,
    pane,
    settingsPage: page,
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
  const rootRef = useRef<HTMLDivElement>(null);
  // Key of the tab being dragged; the strip paints a drop line at the hovered
  // insertion point and the actual move (across panes too) commits on drop.
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
   * else a fresh split to the chat's right.
   *
   * The companion's pane becomes the focused one, so layout actions (a split,
   * a + new tab) land on the panel the user just got, not the chat it sprang
   * from — without it, splitting "the browser" silently split the chat
   * instead. The composer keyboard still stays with the chat: focusTick is
   * NOT bumped, so the chat's textarea keeps the DOM focus it already holds
   * (a shell the user asks for takes the keyboard separately, in newShellTab).
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
            focusedPane: existing.pane,
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
            focusedPane: beside,
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
          focusedPane: paneId,
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
      .catch((e: unknown) => toastError(`New terminal failed: ${errMsg(e)}`));
  }, []);

  /**
   * Open (or focus) a singleton tab in the focused pane: a place you return
   * to, not one you accumulate. An already-open match is raised and given
   * focus; otherwise a fresh one opens in the focused pane and takes the
   * keyboard. This is the user-initiated counterpart to openAside, and the
   * shared mechanic behind the files browser, activity, and history tabs.
   */
  const openSingleton = useCallback(
    (matches: (t: TabState) => boolean, make: (key: number, pane: number) => TabState) => {
      setLayout((s) => {
        const existing = s.tabs.find(matches);
        if (existing) {
          return {
            ...s,
            paneActive: { ...s.paneActive, [existing.pane]: existing.key },
            focusedPane: existing.pane,
            focusTick: s.focusTick + 1,
          };
        }
        const into = s.focusedPane;
        const tab = make(nextKey.current++, into);
        return normalize({
          ...s,
          tabs: [...s.tabs, tab],
          paneActive: { ...s.paneActive, [into]: tab.key },
          focusTick: s.focusTick + 1,
        });
      });
    },
    [],
  );

  /**
   * The file browser, user-initiated: one tab browsing the workspace root.
   * An already-open browser is focused, wherever its navigation has taken
   * it — "the files" is a place you return to, not a tab you accumulate.
   * (More browsers can still exist: the agent's show on a directory opens
   * its own, deduped by path like any surface.)
   */
  const openFilesTab = useCallback(
    () =>
      openSingleton(
        (t) => t.kind === "surface" && t.surface?.kind === "dir",
        (key, pane) => surfaceTab(key, pane, dirSurface(".")),
      ),
    [openSingleton],
  );

  /**
   * The live browser, user-initiated: focus the active tab's panel, creating
   * a fresh browser tab when none are open. Each browser tab has its own
   * stream and its own panel (deduped per stream), so this and the agent's
   * auto-opened panels are windows onto the same shared Chrome's tab table.
   */
  const openBrowserTab = useCallback(() => {
    listBrowserTabs()
      .then(async (tabs) => {
        const target =
          tabs.find((t) => t.active) ?? tabs[0] ?? (await createBrowserTab());
        openSingleton(
          (t) => t.kind === "surface" && t.surface?.path === target.stream,
          (key, pane) =>
            surfaceTab(key, pane, {
              kind: "screencast",
              path: target.stream,
              title: `Browser ${target.id}`,
            }),
        );
      })
      .catch((e: unknown) =>
        // A dead endpoint or downed Chrome must not be a silent no-op.
        toastError(`Browser failed to open: ${errMsg(e)}`),
      );
  }, [openSingleton]);

  /** The merged activity + history view as a singleton tab — past sessions
   *  beside every standing obligation and live run, across all chats. Both
   *  the history (clock) and activity buttons open this one panel. */
  const openActivityTab = useCallback(
    () => openSingleton((t) => t.kind === "activity", activityTab),
    [openSingleton],
  );
  const openHistoryTab = openActivityTab;

  /** The plan detail view for a node, as a tab — clicking a standing
   *  obligation opens (or focuses) its plan's goal tree and code. Deduped by
   *  node so re-clicking the same task raises its tab instead of stacking. */
  const openPlanTab = useCallback(
    (node: number) =>
      openSingleton(
        (t) => t.kind === "plan" && t.planNode === node,
        (key, pane) => planTab(key, pane, node),
      ),
    [openSingleton],
  );

  /** A fresh messenger tab — each one is its own Telegram connector: enter
   *  a bot token (or reattach to a connected bot) and the tab becomes that
   *  bot's conversation. Open as many as you have bots. */
  const newMessengerTab = useCallback((pane?: number) => {
    setLayout((s) => {
      const into = pane ?? s.focusedPane;
      const tab: TabState = {
        key: nextKey.current++,
        title: "Messenger",
        busy: false,
        kind: "messenger",
        resumeId: null,
        sessionId: null,
        titlePinned: false,
        pane: into,
      };
      return normalize({
        ...s,
        tabs: [...s.tabs, tab],
        paneActive: { ...s.paneActive, [into]: tab.key },
        focusedPane: into,
        focusTick: s.focusTick + 1,
      });
    });
  }, []);

  /** The settings panel as a singleton tab, opened from the top-left gear. */
  const openSettingsTab = useCallback(
    () => openSingleton((t) => t.kind === "settings", settingsTab),
    [openSingleton],
  );

  /** Onboarding: the same singleton settings tab, but landed on the Provider
   *  page — where a first run with no configured provider needs to go. */
  const openProviderSetup = useCallback(
    () =>
      openSingleton(
        (t) => t.kind === "settings",
        (key, pane) => settingsTab(key, pane, "provider"),
      ),
    [openSingleton],
  );

  // First run with no provider configured can't talk to a model at all, so
  // there's nothing to do in a chat yet — take the user straight to the
  // Provider settings instead of a dead composer. A missing /api/llm (no
  // config dir, so nothing to configure here) is left alone.
  useEffect(() => {
    fetchLLM()
      .then((info) => {
        if (!info.key_set) openProviderSetup();
      })
      .catch(() => {});
  }, [openProviderSetup]);

  /** A job terminal's "shell here": continue the job's work by hand, in its
   * directory, in a live shell beside it. */
  const openShellBeside = useCallback(
    (fromKey: number, cwd?: string) => {
      createTerminal(cwd)
        .then((info) =>
          openTerminal(fromKey, { kind: "shell", id: info.id, cwd: info.cwd }),
        )
        .catch((e: unknown) => toastError(`New terminal failed: ${errMsg(e)}`));
    },
    [openTerminal],
  );

  /**
   * Execute a UI command from the agent's `ui` tool. Open spawns a panel:
   * a terminal (fresh shell beside the chat that asked — the companion
   * mechanic, the chat keeps the keyboard) or one of the singleton views.
   * Close targets companion tabs only (never a chat — closing the
   * conversation out from under the user would be hostile); focus may raise
   * any tab. "all" closes every companion panel. Matching is a
   * case-insensitive substring of a tab's title or its surface path, so the
   * agent can name a panel the way it sees it ("Browser", a filename).
   */
  const controlUI = useCallback(
    (fromKey: number, cmd: UICommand) => {
      if (cmd.action === "open") {
        switch (cmd.panel) {
          case "chat":
            newTab();
            return;
          case "terminal":
            openShellBeside(fromKey, cmd.cwd);
            return;
          case "browser":
            openBrowserTab();
            return;
          case "files":
            openFilesTab();
            return;
          case "activity":
            openActivityTab();
            return;
          case "history":
            openHistoryTab();
            return;
          case "settings":
            openSettingsTab();
            return;
          default: {
            const exhaustive: never = cmd.panel;
            return exhaustive;
          }
        }
      }
      const q = cmd.target.toLowerCase();
      const hit = (t: TabState) =>
        (t.title ?? "").toLowerCase().includes(q) ||
        (t.surface?.path ?? "").toLowerCase().includes(q);
      if (cmd.action === "close") {
        // Companion tabs only — never a conversation. A chat tab's kind is
        // UNDEFINED (freshTab sets no kind; "chat" is the default), so the
        // guard must treat missing-kind as chat too.
        const targets = layout.tabs.filter(
          (t) => t.kind !== undefined && t.kind !== "chat" && (cmd.target === "all" || hit(t)),
        );
        targets.forEach((t) => closeTab(t.key));
        return;
      }
      const focusTarget =
        cmd.target === "all" ? undefined : layout.tabs.find(hit);
      if (focusTarget) activateTab(focusTarget.key);
    },
    [
      layout.tabs,
      closeTab,
      activateTab,
      newTab,
      openShellBeside,
      openBrowserTab,
      openFilesTab,
      openActivityTab,
      openHistoryTab,
      openSettingsTab,
    ],
  );

  const focusPane = useCallback((pane: number) => {
    setLayout((s) => (s.focusedPane === pane ? s : { ...s, focusedPane: pane }));
  }, []);

  /**
   * The strip's + menu picked a tab type. Non-chat kinds open in the focused
   * pane, so focus the strip's pane first — the queued updates apply in order.
   */
  const newTabOfKind = useCallback(
    (kind: NewTabKind, pane: number) => {
      switch (kind) {
        case "chat":
          newTab(undefined, pane);
          break;
        case "terminal":
          focusPane(pane);
          newShellTab();
          break;
        case "files":
          focusPane(pane);
          openFilesTab();
          break;
        case "browser":
          focusPane(pane);
          openBrowserTab();
          break;
        case "messenger":
          newMessengerTab(pane);
          break;
        default: {
          const exhaustive: never = kind;
          throw new Error(`unknown tab kind: ${String(exhaustive)}`);
        }
      }
    },
    [
      newTab,
      focusPane,
      newShellTab,
      openFilesTab,
      openBrowserTab,
      newMessengerTab,
    ],
  );

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
    onDropTab: (key: number, before: boolean) => {
      const from = dragTab.current;
      if (from !== null && from !== key) moveTab(from, key, before);
      dragTab.current = null;
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
  // Global actions live on whichever strip touches the window's top-right;
  // the settings gear on whichever touches the top-left.
  const actionsPane = places.panes.reduce((best, p) =>
    p.rect.y < 0.001 && p.rect.x + p.rect.w > best.rect.x + best.rect.w ? p : best,
  ).pane;
  const leadingPane = places.panes.reduce((best, p) =>
    p.rect.y < 0.001 && p.rect.x < best.rect.x ? p : best,
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
            onNew={(kind) => newTabOfKind(kind, pane)}
            drag={dragHandlers(pane)}
            leading={
              pane === leadingPane ? (
                <SettingsButton onOpen={openSettingsTab} />
              ) : undefined
            }
            actions={
              pane === actionsPane ? (
                <>
                  <HistoryButton onOpen={openHistoryTab} />
                  <ActivityButton onOpen={openActivityTab} />
                  <GlobalActions onSplit={splitFocused} />
                </>
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
            <Suspense
              fallback={<div className="px-4 py-4 text-faint">Loading…</div>}
            >
            {tab.kind === "surface" && tab.surface ? (
              <SurfaceView
                surface={tab.surface}
                active={visible}
                onNavigate={(s) =>
                  patchTab(tab.key, {
                    surface: s,
                    ...(tab.titlePinned ? {} : { title: surfaceTitle(s) }),
                  })
                }
                onOpenFile={(s) => openSurface(tab.key, s)}
                onCloseSelf={() => closeTab(tab.key)}
              />
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
            ) : tab.kind === "activity" ? (
              <ActivityHistoryView
                active={visible}
                onOpenSession={openSession}
                onOpenChat={(chat) =>
                  openSession({ id: chat, title: "", updated_at: 0 })
                }
                onOpenPlan={openPlanTab}
                onBusy={(busy) => patchTab(tab.key, { busy })}
              />
            ) : tab.kind === "plan" && tab.planNode !== undefined ? (
              <PlanView
                node={tab.planNode}
                active={visible}
                onOpenChat={(chat) =>
                  openSession({ id: chat, title: "", updated_at: 0 })
                }
              />
            ) : tab.kind === "messenger" ? (
              <MessengerView
                botId={tab.messengerBot}
                onBot={(id, title) =>
                  patchTab(tab.key, {
                    messengerBot: id,
                    ...(tab.titlePinned ? {} : { title: title ?? "Messenger" }),
                  })
                }
                renderChat={(sessionId) => (
                  // The bridged session rendered as a REAL agent chat — the
                  // same component, surfaces, runs, and composer as any chat
                  // tab; the bridge mirrors this session to Telegram.
                  <ChatTab
                    key={sessionId}
                    active={visible}
                    focused={visible && tab.pane === focusedPane}
                    focusTick={focusTick}
                    resumeId={sessionId}
                    handle={{
                      onBusy: (busy) => patchTab(tab.key, { busy }),
                      // The tab is named for its connector, not the first
                      // prompt — @bot is the identity that matters here.
                      onTitle: () => {},
                      onSession: (id) => patchTab(tab.key, { sessionId: id }),
                      onOpenSurface: (surface) => openSurface(tab.key, surface),
                      onControlUI: (cmd) => controlUI(tab.key, cmd),
                      onOpenRun: (run) => openRun(tab.key, run),
                      onOpenTerminal: (term) => openTerminal(tab.key, term),
                    }}
                  />
                )}
              />
            ) : tab.kind === "settings" ? (
              <SettingsView initialPage={tab.settingsPage} />
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
                  onControlUI: (cmd) => controlUI(tab.key, cmd),
                  onOpenRun: (run) => openRun(tab.key, run),
                  onOpenTerminal: (term) => openTerminal(tab.key, term),
                }}
              />
            )}
            </Suspense>
          </div>
        );
      })}
      <Toasts />
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
