import { useEffect, useState } from "react";

/**
 * Whether the document is visible — the gate for background polling. Effects
 * keyed on this pause their intervals while the window is hidden (minimized,
 * other space, backgrounded PWA) and re-run — refreshing immediately — the
 * moment it comes back.
 */
export function useDocumentVisible(): boolean {
  const [visible, setVisible] = useState(document.visibilityState === "visible");
  useEffect(() => {
    const on = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return visible;
}
