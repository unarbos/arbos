import { lazy, type ComponentType, type LazyExoticComponent } from "react";

/** Set when a failed chunk load has already triggered a reload, so a chunk
 *  that is genuinely unreachable (offline, server down) cannot reload-loop. */
const RELOAD_KEY = "arbos-chunk-reloaded";

/** The post-reload fallback: the chunk is still unreachable, so say why the
 *  panel is empty instead of letting the rejection unmount the whole app
 *  (there is no error boundary above the tab bodies). */
function ChunkError() {
  return (
    <div className="flex flex-1 items-center justify-center px-4 py-4 text-[12.5px] text-muted">
      This panel failed to load — check the connection and reload the page.
    </div>
  );
}

/**
 * Code-split with stale-page healing. When the server hot-swaps to a new
 * build (arbos upgrade, the self-restart watcher), a page loaded before the
 * swap names content-hashed chunks that no longer exist, so the FIRST lazy
 * panel it opens fails to import — historically a silent unmount of the
 * entire app. One reload fetches the current index.html (served no-store)
 * whose chunk names match again; a failure that survives the reload renders
 * an inline error instead of reloading forever.
 *
 * T mirrors React's own lazy() signature exactly (the component, not its
 * props, constrained the way React constrains it) — inferring props through
 * ComponentType<P> trips TS inference at call sites.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function lazyPanel<T extends ComponentType<any>>(
  load: () => Promise<{ default: T }>,
): LazyExoticComponent<T> {
  const healed = async (): Promise<{ default: T }> => {
    try {
      const m = await load();
      sessionStorage.removeItem(RELOAD_KEY);
      return m;
    } catch (err) {
      if (sessionStorage.getItem(RELOAD_KEY) === null) {
        sessionStorage.setItem(RELOAD_KEY, "1");
        location.reload();
        // Hold the Suspense fallback up while the reload lands.
        return new Promise(() => {});
      }
      console.error("arbos: panel chunk failed to load", err);
      // Safe stand-in for any panel: it takes no props and ignores whatever
      // React passes, so substituting it for T cannot misbehave at runtime.
      return { default: ChunkError as unknown as T };
    }
  };
  return lazy(healed);
}
