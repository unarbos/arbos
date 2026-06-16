import { useEffect, type RefObject } from "react";

/**
 * Grow a textarea to fit its content whenever `value` changes, optionally
 * capped (the composer) or unbounded (the edit card, which should show the
 * whole message).
 *
 * `remeasureKey` forces a re-measure even when `value` is unchanged: a hidden
 * tab (display:none) reads scrollHeight as 0, so its textarea height is stale
 * when it becomes visible again — passing the tab's `active` flag here
 * recomputes the height on show, so an empty composer doesn't collapse and let
 * the placeholder overlap the controls below it.
 */
export function useAutosize(
  ref: RefObject<HTMLTextAreaElement | null>,
  value: string,
  maxPx?: number,
  remeasureKey?: unknown,
): void {
  useEffect(() => {
    const ta = ref.current;
    if (!ta) return;
    // A hidden element has scrollHeight 0; skip then so we don't pin a wrong
    // height — the next visible render re-runs this and measures correctly.
    if (ta.offsetParent === null && ta.getClientRects().length === 0) return;
    ta.style.height = "auto";
    ta.style.height = `${maxPx != null ? Math.min(ta.scrollHeight, maxPx) : ta.scrollHeight}px`;
  }, [ref, value, maxPx, remeasureKey]);
}
