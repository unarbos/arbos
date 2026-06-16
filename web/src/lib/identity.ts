// The host's self-asserted display name for a shared chat, set at share time
// and applied to the host's own messages so guests see a name instead of "via
// another window". Kept per-session in localStorage (not a global setting): it
// is contextual to inviting people to a specific chat, and survives reloads
// without any backend state. Guests get their name a different way (the scoped
// share cookie, stamped server-side); this is only for the trusted host.

const KEY_PREFIX = "arbos:hostName:";

/** Trim, drop control chars, and cap to 32 runes — mirrors the server's
 *  sanitizeGuestName so a host name behaves like a guest name. */
export function sanitizeDisplayName(s: string): string {
  const cleaned = [...s]
    .filter((ch) => {
      const c = ch.codePointAt(0) ?? 0;
      return c >= 0x20 && c !== 0x7f;
    })
    .join("")
    .trim();
  return [...cleaned].slice(0, 32).join("");
}

export function hostName(sessionId: string | null | undefined): string {
  if (!sessionId) return "";
  try {
    return localStorage.getItem(KEY_PREFIX + sessionId) ?? "";
  } catch {
    return "";
  }
}

const EVENT = "arbos:hostName";

export function setHostName(sessionId: string, name: string): void {
  const clean = sanitizeDisplayName(name);
  try {
    if (clean) localStorage.setItem(KEY_PREFIX + sessionId, clean);
    else localStorage.removeItem(KEY_PREFIX + sessionId);
  } catch {
    // localStorage unavailable (private mode / blocked) — naming just no-ops.
  }
  // The storage event does not fire in the same document, so notify in-tab
  // listeners (the open chat) directly to relabel without a reload.
  try {
    window.dispatchEvent(new CustomEvent(EVENT, { detail: { sessionId, name: clean } }));
  } catch {
    // No window (SSR/tests) — nothing to notify.
  }
}

/** Subscribe to host-name changes for one session. Returns an unsubscribe fn. */
export function onHostNameChange(
  sessionId: string,
  fn: (name: string) => void,
): () => void {
  const handler = (e: Event) => {
    const detail = (e as CustomEvent<{ sessionId: string; name: string }>).detail;
    if (detail && detail.sessionId === sessionId) fn(detail.name);
  };
  window.addEventListener(EVENT, handler);
  return () => window.removeEventListener(EVENT, handler);
}
