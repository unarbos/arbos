import { useSyncExternalStore } from "react";

import {
  DEFAULT_THEME_ID,
  THEMES,
  themeById,
  type Theme,
} from "./themes";

/* ------------------------------------------------------------------ */
/* Theme store — a tiny external store so any component can read or set */
/* the active theme. The active theme writes its palette onto the root  */
/* element's inline style, overriding index.css's `@theme` defaults.    */
/* The choice persists in localStorage and is reapplied on next load.   */
/* ------------------------------------------------------------------ */

const STORAGE_KEY = "arbos.theme";

function resolve(id: string | null): Theme {
  return (id && themeById(id)) || themeById(DEFAULT_THEME_ID) || THEMES[0];
}

function readStored(): string | null {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

let current: Theme = resolve(readStored());
const listeners = new Set<() => void>();

/** Paint a theme's palette onto :root so every `--color-*` token updates. */
function paint(theme: Theme): void {
  const root = document.documentElement;
  for (const [token, value] of Object.entries(theme.colors)) {
    root.style.setProperty(`--color-${token}`, value);
  }
  const scheme = theme.dark ? "dark" : "light";
  root.style.colorScheme = scheme;
  document.body.style.colorScheme = scheme;
}

/** Apply the persisted (or default) theme. Call once before first paint. */
export function initTheme(): void {
  paint(current);
}

export function setTheme(id: string): void {
  current = resolve(id);
  paint(current);
  try {
    localStorage.setItem(STORAGE_KEY, current.id);
  } catch {
    // Ignore storage failures (private mode, quota) — the theme still applies.
  }
  listeners.forEach((fn) => fn());
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function getSnapshot(): Theme {
  return current;
}

/** Reactive accessor for the active theme. */
export function useTheme(): Theme {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
