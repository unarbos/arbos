/**
 * The split-tree: the window is a binary tree of panes, exactly like an
 * editor's split groups. A leaf is a pane (a tab strip + the visible chat);
 * a split divides its rectangle between two subtrees at a ratio. Splitting a
 * pane replaces its leaf with a split node, closing a pane's last tab
 * collapses the split back to the sibling — recursion falls out for free.
 *
 * Everything here is pure tree math. The render side flattens the tree into
 * window-fraction rectangles (computePlaces) and absolutely positions strips,
 * chats, and dividers from them, so a tree restructure is a style change —
 * never a remount that would drop a chat's live connection.
 */

/** "right" = side-by-side panes; "down" = stacked. */
export type SplitDir = "right" | "down";

export type LayoutNode =
  | { kind: "pane"; id: number }
  | {
      kind: "split";
      id: number;
      dir: SplitDir;
      /** First subtree's share of the split axis (0.2–0.8). */
      ratio: number;
      a: LayoutNode;
      b: LayoutNode;
    };

/** A rectangle in window fractions (0..1 on both axes). */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface PanePlace {
  pane: number;
  rect: Rect;
}

export interface DividerPlace {
  split: number;
  dir: SplitDir;
  /** The split's own rectangle — ratio drags are relative to it. */
  rect: Rect;
  /** Where the seam falls: an x fraction for "right", a y fraction for "down". */
  at: number;
}

export interface Places {
  panes: PanePlace[];
  dividers: DividerPlace[];
}

/** Flatten the tree into absolute window-fraction rectangles. */
export function computePlaces(root: LayoutNode): Places {
  const out: Places = { panes: [], dividers: [] };
  walk(root, { x: 0, y: 0, w: 1, h: 1 }, out);
  return out;
}

function walk(node: LayoutNode, rect: Rect, out: Places): void {
  if (node.kind === "pane") {
    out.panes.push({ pane: node.id, rect });
    return;
  }
  if (node.dir === "right") {
    const w = rect.w * node.ratio;
    walk(node.a, { ...rect, w }, out);
    walk(node.b, { ...rect, x: rect.x + w, w: rect.w - w }, out);
    out.dividers.push({ split: node.id, dir: node.dir, rect, at: rect.x + w });
  } else {
    const h = rect.h * node.ratio;
    walk(node.a, { ...rect, h }, out);
    walk(node.b, { ...rect, y: rect.y + h, h: rect.h - h }, out);
    out.dividers.push({ split: node.id, dir: node.dir, rect, at: rect.y + h });
  }
}

/** Pane ids in tree order (left/top first). */
export function panesOf(node: LayoutNode): number[] {
  if (node.kind === "pane") return [node.id];
  return [...panesOf(node.a), ...panesOf(node.b)];
}

/**
 * Split the given pane: its leaf becomes a split node holding the original
 * pane and a brand-new one (after/below it, at 50/50). Ids are allocated by
 * the caller so the tree stays a pure value.
 */
export function splitPane(
  node: LayoutNode,
  pane: number,
  dir: SplitDir,
  splitId: number,
  newPane: number,
): LayoutNode {
  if (node.kind === "pane") {
    if (node.id !== pane) return node;
    return {
      kind: "split",
      id: splitId,
      dir,
      ratio: 0.5,
      a: node,
      b: { kind: "pane", id: newPane },
    };
  }
  const a = splitPane(node.a, pane, dir, splitId, newPane);
  const b = splitPane(node.b, pane, dir, splitId, newPane);
  if (a === node.a && b === node.b) return node;
  return { ...node, a, b };
}

/**
 * Remove a pane: its parent split collapses to the sibling subtree, which
 * inherits the full rectangle. Returns null only if the whole tree was that
 * single pane (the caller decides what an empty window means).
 */
export function removePane(node: LayoutNode, pane: number): LayoutNode | null {
  if (node.kind === "pane") return node.id === pane ? null : node;
  const a = removePane(node.a, pane);
  const b = removePane(node.b, pane);
  if (a === null) return b;
  if (b === null) return a;
  if (a === node.a && b === node.b) return node;
  return { ...node, a, b };
}

/** Re-balance one split (a divider drag). */
export function setSplitRatio(
  node: LayoutNode,
  split: number,
  ratio: number,
): LayoutNode {
  if (node.kind === "pane") return node;
  if (node.id === split) return { ...node, ratio };
  const a = setSplitRatio(node.a, split, ratio);
  const b = setSplitRatio(node.b, split, ratio);
  if (a === node.a && b === node.b) return node;
  return { ...node, a, b };
}
