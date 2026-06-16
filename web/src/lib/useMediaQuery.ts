import { useEffect, useState } from "react";

/**
 * Subscribe to a CSS media query and re-render when it flips. SSR-safe-ish:
 * defaults to `false` until the first effect runs. Used to collapse the
 * multi-pane workspace into a single full-width pane on phones.
 */
export function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() =>
    typeof window !== "undefined" && "matchMedia" in window
      ? window.matchMedia(query).matches
      : false,
  );

  useEffect(() => {
    if (typeof window === "undefined" || !("matchMedia" in window)) return;
    const mql = window.matchMedia(query);
    const onChange = () => setMatches(mql.matches);
    onChange();
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, [query]);

  return matches;
}
