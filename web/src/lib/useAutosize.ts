import { useEffect, type RefObject } from "react";

/**
 * Grow a textarea to fit its content whenever `value` changes, optionally
 * capped (the composer) or unbounded (the edit card, which should show the
 * whole message).
 */
export function useAutosize(
  ref: RefObject<HTMLTextAreaElement | null>,
  value: string,
  maxPx?: number,
): void {
  useEffect(() => {
    const ta = ref.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${maxPx != null ? Math.min(ta.scrollHeight, maxPx) : ta.scrollHeight}px`;
  }, [ref, value, maxPx]);
}
