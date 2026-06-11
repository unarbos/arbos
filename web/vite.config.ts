import { fileURLToPath, URL } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Dev flow: `arbos -web :8420` in one terminal, `npm run dev` here. The proxy
// forwards REST and the control-seam WebSocket to the gateway, so the SPA is
// origin-agnostic (it always talks to its own origin's /api).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    proxy: {
      "/api": {
        target: `http://127.0.0.1:${process.env.ARBOS_GW_PORT ?? "8420"}`,
        ws: true,
      },
    },
  },
  build: {
    outDir: "dist",
  },
});
