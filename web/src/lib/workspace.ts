import type { Layout, TabState } from "../App";
import type { LayoutNode } from "./layout";

/* ------------------------------------------------------------------ */
/* Workspace persistence — the whole window (split tree, tabs, panes)   */
/* survives a reload by mirroring the live layout into localStorage on   */
/* every change and rehydrating it on cold start. Without this a refresh */
/* drops every tab and panel back to one fresh chat.                     */
/*                                                                       */
/* Only the layout SHAPE is stored, never a chat's transcript: a chat    */
/* tab carries its bound session id, and on restore we resume that       */
/* session so its history streams back from the seam — the same path     */
/* the History view uses. Live, host-side handles (a shell's PTY, a job  */
/* terminal, a browser screencast) reconnect by their id if the host     */
/* still has them, and degrade to an empty panel if it doesn't.          */
/* ------------------------------------------------------------------ */

const STORAGE_KEY = "arbos.workspace";

/** Bump when the stored shape changes incompatibly: a mismatch is discarded
 *  rather than restored, so an old blob can never render a broken window
 *  against new code. There are no migrations — a clean start is the fallback. */
const SCHEMA_VERSION = 1;

/** The on-disk envelope: a version stamp wrapping the layout snapshot. */
interface Stored {
  version: number;
  layout: Layout;
}

interface Restored {
  layout: Layout;
  /** Next tab key — past every restored tab's key, so new tabs don't collide. */
  nextKey: number;
  /** Next pane/split id — past every id in the restored tree. */
  nextId: number;
}

/** Largest pane/split id anywhere in the tree (ids are shared across both). */
function maxNodeId(node: LayoutNode): number {
  if (node.kind === "pane") return node.id;
  return Math.max(node.id, maxNodeId(node.a), maxNodeId(node.b));
}

/**
 * A structurally sound split tree: every pane has a numeric id and every split
 * has a direction, a finite ratio, and two valid children. A blob that fails
 * this would later feed NaN rectangles to the renderer (rect.w * undefined),
 * so reject it here and start clean instead.
 */
function validNode(node: unknown): node is LayoutNode {
  if (!node || typeof node !== "object") return false;
  const n = node as Record<string, unknown>;
  if (n.kind === "pane") return typeof n.id === "number";
  if (n.kind === "split") {
    return (
      typeof n.id === "number" &&
      (n.dir === "right" || n.dir === "down") &&
      typeof n.ratio === "number" &&
      Number.isFinite(n.ratio) &&
      validNode(n.a) &&
      validNode(n.b)
    );
  }
  return false;
}

/**
 * A restored tab is a clean slate for anything in-flight: nothing is busy
 * after a reload (no turn is mid-stream), and a chat with a bound session
 * resumes it so its transcript reloads — a fresh, never-bound chat just
 * reopens its empty composer.
 */
function rehydrateTab(tab: TabState): TabState {
  const isChat = tab.kind === undefined || tab.kind === "chat";
  return {
    ...tab,
    busy: false,
    resumeId: isChat ? (tab.sessionId ?? tab.resumeId) : tab.resumeId,
  };
}

/** Read the persisted window, or null when there's nothing usable to restore. */
export function loadWorkspace(): Restored | null {
  let raw: string | null;
  try {
    raw = localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Stored;
    if (!parsed || typeof parsed !== "object" || parsed.version !== SCHEMA_VERSION) {
      return null;
    }
    const snap = parsed.layout;
    if (
      !snap ||
      typeof snap !== "object" ||
      !validNode(snap.root) ||
      !Array.isArray(snap.tabs) ||
      snap.tabs.length === 0 ||
      !snap.paneActive ||
      typeof snap.focusedPane !== "number"
    ) {
      return null;
    }
    const layout: Layout = {
      root: snap.root,
      tabs: snap.tabs.map(rehydrateTab),
      paneActive: snap.paneActive,
      focusedPane: snap.focusedPane,
      focusTick: 0,
    };
    const nextKey = Math.max(0, ...layout.tabs.map((t) => t.key)) + 1;
    const nextId = maxNodeId(layout.root) + 1;
    return { layout, nextKey, nextId };
  } catch {
    return null;
  }
}

/** Mirror the current window to storage; failures (quota, private mode) are
 *  swallowed — persistence is a convenience, never a correctness dependency. */
export function saveWorkspace(layout: Layout): void {
  try {
    const blob: Stored = { version: SCHEMA_VERSION, layout };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(blob));
  } catch {
    // Ignore storage failures — the live window is unaffected.
  }
}
