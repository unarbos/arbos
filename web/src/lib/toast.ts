/**
 * Transient error toasts for user-initiated actions that have no transcript
 * to report into (new shell, new browser tab). A module-scope pub/sub keeps
 * the call sites free of prop drilling; the Toasts component renders them.
 */

type Listener = (msg: string) => void;

const listeners = new Set<Listener>();

export function onToast(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Surface a failed user action as a visible, auto-dismissing toast. */
export function toastError(msg: string): void {
  for (const fn of listeners) fn(msg);
}

export function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
