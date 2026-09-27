import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "~/app";
import { prefetchMarkdown } from "~/components/markdown";
import { ErrorBoundary } from "~/components/error-boundary";
import { listenErrors } from "~/events";
import { readSessionStorage, writeSessionStorage } from "~/lib/storage";
import "./index.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary label="Pecan">
      <App />
    </ErrorBoundary>
  </StrictMode>,
);

listenErrors();
prefetchMarkdown();
installChunkReloadGuard();

/**
 * A stale tab left open across a redeploy references chunk hashes the server
 * no longer has. Vite's `vite:preloadError` (and a bare dynamic-import
 * rejection from React.lazy, which surfaces as an unhandled rejection) both
 * mean "the module graph moved on" — reload once to pick up the new build.
 * Guarded to once per ~10s via sessionStorage so a genuinely broken chunk
 * doesn't reload-loop the tab.
 */
function installChunkReloadGuard() {
  const RELOAD_KEY = "pecan:chunk-reload-at";
  const COOLDOWN_MS = 10_000;
  const CHUNK_ERROR_PATTERN = /dynamically imported module|failed to fetch dynamically imported/i;

  function reloadOnce() {
    const last = Number(readSessionStorage(RELOAD_KEY) ?? "0");
    if (Date.now() - last < COOLDOWN_MS) return;
    writeSessionStorage(RELOAD_KEY, String(Date.now()));
    location.reload();
  }

  window.addEventListener("vite:preloadError", reloadOnce);
  window.addEventListener("unhandledrejection", (event) => {
    const message = event.reason instanceof Error ? event.reason.message : String(event.reason ?? "");
    if (CHUNK_ERROR_PATTERN.test(message)) reloadOnce();
  });
}
