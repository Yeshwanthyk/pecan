/** Live updates: one EventSource fanning debounced store refreshes. */
import { api } from "~/api/client";
import { useApp } from "~/store";

const SUBAGENT_ACTIVITY_WIDGET = "pi-subagents/activity/v1";

export function listenErrors() {
  window.addEventListener("pecan:error", (event) => {
    const detail = (event as CustomEvent<string>).detail;
    if (detail) console.warn("pecan:", detail); // eslint-disable-line no-console -- last-resort diagnostics for otherwise-swallowed API errors
  });
}

let source: EventSource | null = null;
let retryMs = 500;

export function connectEvents() {
  if (source) return; // StrictMode double-effect guard.
  const open = () => {
    source = new EventSource("/api/events");
    source.onopen = () => {
      retryMs = 500;
      useApp.getState().setConnected(true);
    };
    source.addEventListener("index-changed", () => {
      retryMs = 500;
      useApp.getState().setConnected(true);
      refreshSessionsDebounced();
      refreshThreadDebounced();
    });
    source.addEventListener("agent", (event) => {
      useApp.getState().setConnected(true);
      const payload: unknown = JSON.parse((event as MessageEvent<string>).data);
      if (typeof payload !== "object" || payload === null) return;
      const envelope = payload as { type?: string; id?: string; event?: unknown };

      // Worker events arrive wrapped: {type:"agent-event", id, event:{…}}.
      if (envelope.type === "agent-event") {
        const inner =
          typeof envelope.event === "object" && envelope.event !== null
            ? (envelope.event as {
                type?: string;
                method?: string;
                title?: string;
                options?: Array<{ label?: string; value?: string }>;
                widgetKey?: string;
                widgetLines?: unknown;
              })
            : undefined;
        if (
          envelope.id &&
          inner?.type === "extension_ui_request" &&
          inner.method === "setWidget" &&
          inner.widgetKey === SUBAGENT_ACTIVITY_WIDGET
        ) {
          useApp
            .getState()
            .setSubagentActivity(envelope.id, parseSubagentActivity(inner.widgetLines));
        }
        if (
          envelope.id &&
          inner &&
          (inner.type === "extension_ui_request" || inner.type === "ui_request") &&
          (inner.method === "select" ||
            inner.method === "confirm" ||
            inner.method === "input")
        ) {
          const requestId = (envelope.event as { id?: string }).id;
          if (requestId) {
            useApp.getState().addPendingAsk(envelope.id, {
              id: requestId,
              method: inner.method,
              title: inner.title,
              options: inner.options,
            });
          }
        }
        return;
      }

      // Bare server notifications keep their own top-level type tag.
      if (envelope.type === "thread-changed") {
        if (envelope.id && currentThreadId() === envelope.id) {
          refreshSessionsDebounced();
          refreshThreadDebounced();
        }
      }
    });
    source.onerror = () => {
      source?.close();
      source = null;
      useApp.getState().setConnected(false);
      setTimeout(open, Math.min(retryMs, 8000));
      retryMs *= 2;
    };
  };
  open();

  // Composer actions (send/model/thinking changes) request agent refreshes.
  window.addEventListener("pecan:agent-refresh", (event) => {
    const id = (event as CustomEvent<string>).detail;
    if (!id || currentThreadId() !== id) return;
    void api
      .attachAgent(id)
      .then((agent) => {
        if (currentThreadId() === id && agent.sessionId === id) {
          useApp.getState().setAgent(agent);
        }
      })
      .catch(() => undefined);
  });
}

function parseSubagentActivity(
  widgetLines: unknown,
): { revision: number; children: Array<{ title: string; startedAt: number; lastActivityAt: number }> } | null {
  if (widgetLines === undefined) return null;
  if (!Array.isArray(widgetLines) || widgetLines.length !== 1) return null;
  const line = widgetLines[0];
  if (typeof line !== "string" || line.length > 15 * 1024) return null;
  try {
    const value: unknown = JSON.parse(line);
    if (typeof value !== "object" || value === null) return null;
    const snapshot = value as { version?: unknown; revision?: unknown; children?: unknown };
    if (
      snapshot.version !== 1 ||
      typeof snapshot.revision !== "number" ||
      !Number.isFinite(snapshot.revision) ||
      !Array.isArray(snapshot.children)
    ) {
      return null;
    }
    const children = snapshot.children.flatMap((child) => {
      if (typeof child !== "object" || child === null) return [];
      const row = child as {
        status?: unknown;
        title?: unknown;
        startedAt?: unknown;
        lastActivityAt?: unknown;
      };
      if (
        row.status !== "running" ||
        typeof row.title !== "string" ||
        row.title.length === 0 ||
        row.title.length > 160 ||
        typeof row.startedAt !== "number" ||
        !Number.isFinite(row.startedAt) ||
        typeof row.lastActivityAt !== "number" ||
        !Number.isFinite(row.lastActivityAt)
      ) {
        return [];
      }
      return [{
        title: row.title,
        startedAt: row.startedAt,
        lastActivityAt: row.lastActivityAt,
      }];
    });
    return { revision: snapshot.revision, children: children.slice(0, 4) };
  } catch {
    return null;
  }
}

function currentThreadId(): string | null {
  return useApp.getState().thread?.summary.id ?? null;
}

function debounce<Args extends unknown[]>(fn: (...args: Args) => void, ms: number) {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (...args: Args) => {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      fn(...args);
    }, ms);
  };
}

const refreshSessionsDebounced = debounce(() => void refreshSessions(), 200);

const refreshThreadDebounced = debounce(() => void refreshThread(), 250);

export async function refreshSessions() {
  try {
    const page = await api.sessions({});
    useApp.getState().setSessions(page.sessions);
  } catch {
    /* server restarting; SSE reconnect will retry */
  }
}

async function refreshThread() {
  const id = useApp.getState().thread?.summary.id;
  if (!id) return;
  try {
    const thread = await api.thread(id);
    if (currentThreadId() === id && thread.summary.id === id) {
      useApp.getState().setThread(thread);
    }
  } catch {
    /* transient */
  }
}
