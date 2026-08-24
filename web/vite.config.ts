import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      { find: "~", replacement: fileURLToPath(new URL("./src", import.meta.url)) },
      {
        find: /^shiki$/,
        replacement: fileURLToPath(new URL("./src/vendor/shiki-lean.ts", import.meta.url)),
      },
      {
        find: /^shiki\/wasm$/,
        replacement: fileURLToPath(
          new URL("./src/vendor/shiki-wasm-disabled.ts", import.meta.url),
        ),
      },
      {
        find: /^@pierre\/theming\/themes$/,
        replacement: fileURLToPath(
          new URL("./src/vendor/pierre-themes-lean.ts", import.meta.url),
        ),
      },
    ],
  },
  server: {
    port: 5714,
    // Backend binds a routable interface for phone access; override with
    // PECAN_API_TARGET when it moves.
    proxy: {
      "/api": {
        target: process.env.PECAN_API_TARGET ?? "http://100.68.203.117:7614",
      },
    },
  },
  build: { outDir: "dist", sourcemap: false },
});
