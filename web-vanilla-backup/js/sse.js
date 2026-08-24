/** Reconnecting SSE listener for /api/events. */

/**
 * Connects and dispatches parsed events to `handlers` by event name.
 * Returns a close() function; reconnects automatically with backoff.
 */
export function connectEvents(handlers) {
  let closed = false;
  let retryMs = 500;
  let source = null;

  const open = () => {
    source = new EventSource("/api/events");
    source.addEventListener("index-changed", (e) => {
      retryMs = 500;
      handlers["index-changed"]?.(safeParse(e.data));
    });
    source.addEventListener("agent", (e) => {
      retryMs = 500;
      handlers.agent?.(safeParse(e.data));
    });
    source.onerror = () => {
      source.close();
      if (!closed) {
        setTimeout(open, Math.min(retryMs, 8000));
        retryMs *= 2;
      }
    };
  };
  open();

  return () => {
    closed = true;
    source?.close();
  };
}

function safeParse(text) {
  try {
    return JSON.parse(text);
  } catch {
    return {};
  }
}
