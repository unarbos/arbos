import { useSyncExternalStore } from "react";

/* ------------------------------------------------------------------ */
/* Settings store — the same tiny external-store shape as theme.ts, so  */
/* any component can read or flip a preference. The whole object        */
/* persists as one JSON blob in localStorage and unknown/missing keys   */
/* fall back to defaults, so the shape can grow without migrations.     */
/* ------------------------------------------------------------------ */

const STORAGE_KEY = "arbos.settings";

export interface Settings {
  /** Fire a system notification when an agent finishes while unfocused. */
  systemNotifications: boolean;
  /** Play a short chime when an agent finishes responding. */
  completionSound: boolean;
  /** Read the agent's final reply aloud when it finishes responding. */
  speakResponses: boolean;
  /** Switch a People tab and its chat together: raising one raises its
   *  partner in the adjacent pane, so the chat stays alongside its agentic
   *  half. */
  linkPeopleChat: boolean;
}

const DEFAULTS: Settings = {
  systemNotifications: false,
  completionSound: false,
  speakResponses: false,
  linkPeopleChat: true,
};

function load(): Settings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULTS;
    return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<Settings>) };
  } catch {
    return DEFAULTS;
  }
}

let current: Settings = load();
const listeners = new Set<() => void>();

/** Non-reactive read, for event handlers and one-shot effects. */
export function getSettings(): Settings {
  return current;
}

export function setSetting<K extends keyof Settings>(
  key: K,
  value: Settings[K],
): void {
  current = { ...current, [key]: value };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    // Ignore storage failures (private mode, quota) — the setting still applies.
  }
  listeners.forEach((fn) => fn());
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function getSnapshot(): Settings {
  return current;
}

/** Reactive accessor for the settings. */
export function useSettings(): Settings {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
