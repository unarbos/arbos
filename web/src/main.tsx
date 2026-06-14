import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import { ShareChat } from "./ShareChat";
import { fetchMe, type Me } from "./lib/api";
import { initTheme } from "./lib/theme";
import "./index.css";

initTheme();

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
  if (!me) return null; // a blink while /api/me resolves
  if (me.kind === "share" && me.session) {
    return <ShareChat session={me.session} perm={me.perm ?? "read"} />;
  }
  return <App />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
);
