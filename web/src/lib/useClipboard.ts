import { useCallback, useState } from "react";

/** Tri-valued copy state: "idle" before any attempt, "ok" after a successful
 *  write, "blocked" when the clipboard API refused (e.g. a plain-http origin) —
 *  so a blocked copy is said out loud rather than failing silently. */
export type CopyState = "idle" | "ok" | "blocked";

/** Clipboard-copy with a tri-valued result the UI can render. One home for the
 *  write/try-catch that the share affordances all need. */
export function useClipboard(): {
  state: CopyState;
  copy: (text: string) => Promise<void>;
  reset: () => void;
} {
  const [state, setState] = useState<CopyState>("idle");
  const copy = useCallback(async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setState("ok");
    } catch {
      setState("blocked");
    }
  }, []);
  const reset = useCallback(() => setState("idle"), []);
  return { state, copy, reset };
}
