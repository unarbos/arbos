import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import { ShareApp } from "./ShareApp";
import { initTheme } from "./lib/theme";
import "./index.css";

initTheme();

// A /s/<token> path is a scoped share link: boot the read-only share view
// instead of the live workspace. The token's grant is enforced server-side;
// this only chooses which app to mount.
const isShare = window.location.pathname.startsWith("/s/");

createRoot(document.getElementById("root")!).render(
  <StrictMode>{isShare ? <ShareApp /> : <App />}</StrictMode>,
);
