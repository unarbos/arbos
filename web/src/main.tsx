import { Component, StrictMode, useEffect, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import { ShareChat } from "./ShareChat";
import { fetchMe, type Me } from "./lib/api";
import { setHostName } from "./lib/identity";
import { initTheme } from "./lib/theme";
import "./index.css";

initTheme();

/**
 * Last-resort guard: a render/effect error anywhere below would otherwise blow
 * away the whole tree and leave a blank page with no way back. Catch it and
 * show the message (with a reload), so a crash is recoverable and legible
 * instead of a silent white screen.
 */
class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="flex h-dvh flex-col items-center justify-center gap-3 bg-canvas p-6 text-text">
        <div className="text-[13px] font-semibold text-bright">Something went wrong</div>
        <pre className="max-h-[50vh] max-w-2xl overflow-auto rounded-md border border-line bg-panel p-3 text-[11px] text-muted whitespace-pre-wrap">
          {String(this.state.error?.stack || this.state.error)}
        </pre>
        <button
          type="button"
          onClick={() => window.location.reload()}
          className="rounded-md bg-btn px-3 py-1.5 text-[12.5px] font-semibold text-canvas hover:bg-bright"
        >
          Reload
        </button>
      </div>
    );
  }
}

// Which app to mount is a question of *who you are*, not what URL you typed: a
// share link redeems into a scoped session cookie (server-side) and lands here
// at "/", so we ask the gateway. A session-scoped principal gets the focused
// share view (one chat); everyone else gets the full workspace.
function Root() {
  const [me, setMe] = useState<Me | null>(null);
  useEffect(() => {
    fetchMe()
      .then(setMe)
      .catch(() => setMe({ kind: "local" }));
  }, []);
  // A guest learns their own name from /api/me; store it the same way the host's
  // name is stored so the shared chat labels the guest's own messages live.
  useEffect(() => {
    if (me?.kind === "share" && me.session && me.name) {
      setHostName(me.session, me.name);
    }
  }, [me]);
  if (!me) return null; // a blink while /api/me resolves
  if (me.kind === "share" && me.session) {
    return <ShareChat session={me.session} perm={me.perm ?? "read"} matrix={me.matrix} />;
  }
  return <App />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <Root />
    </ErrorBoundary>
  </StrictMode>,
);
