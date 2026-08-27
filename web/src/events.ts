/** Live updates: one EventSource fanning debounced store refreshes. */
import { api } from "~/api/client";
import { useApp, type DialogOption, type PendingAsk, type SubagentActivity } from "~/store";

const SUBAGENT_ACTIVITY_WIDGET = "pi-subagents/activity/v1";

export function listenErrors() {
  window.addEventListener("pecan:error", (event) => {
    const detail = (event as CustomEvent<string>).detail;
    if (detail) console.warn("pecan:", detail); // eslint-disable-line no-console -- last-resort diagnostics for otherwise-swallowed API errors
  });
}

let source: EventSource | null = null;
let retryMs = 500;

/** Accumulated assistant text for the in-flight message (module-local so
 * deltas do not re-render the app per token). */
let draftId: string | null = null;
let draftText = "";
let draftFlushTimer: ReturnType<typeof setTimeout> | null = null;

function resetDraft(id: string | null) {
  draftId = id;
  draftText = "";
  if (draftFlushTimer !== null) {
    clearTimeout(draftFlushTimer);
    draftFlushTimer = null;
  }
  useApp.getState().setStreamingDraft(null);
}

/** Pushes draft updates to the store at most every ~120ms. */
function queueDraftFlush() {
  if (draftFlushTimer !== null || draftId === null) return;
  draftFlushTimer = setTimeout(() => {
    draftFlushTimer = null;
    useApp.getState().setStreamingDraft({ id: draftId ?? "", text: draftText });
  }, 120);
}

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
        const eventType =
          typeof envelope.event === "object" && envelope.event !== null
            ? (envelope.event as { type?: string }).type
            : undefined;
        if (envelope.id) {
          if (eventType === "message_start") {
            resetDraft(envelope.id);
          } else if (
            eventType === "agent_start" ||
            eventType === "turn_start" ||
            eventType === "abort"
          ) {
            resetDraft(null);
          } else if (eventType === "message_update") {
            const delta =
              typeof envelope.event === "object" && envelope.event !== null
                ? (envelope.event as { assistantMessageEvent?: { type?: string; delta?: string } })
                    .assistantMessageEvent
                : undefined;
            if (delta?.type === "text_delta" && typeof delta.delta === "string") {
              if (draftId !== envelope.id) resetDraft(envelope.id);
              draftText += delta.delta;
              queueDraftFlush();
            }
          } else if (eventType === "message_end" || eventType === "agent_settled") {
            resetDraft(null);
          }
        }
        const inner =
          typeof envelope.event === "object" && envelope.event !== null
            ? (envelope.event as {
                type?: string;
                method?: string;
                title?: string;
                message?: string;
                placeholder?: string;
                prefill?: string;
                options?: unknown;
                statusKey?: string;
                statusText?: string;
                widgetKey?: string;
                widgetLines?: unknown;
                widgetPlacement?: "aboveEditor" | "belowEditor";
                notifyType?: "info" | "warning" | "error";
                text?: string;
              })
            : undefined;
        const requestId = envelope.event && typeof envelope.event === "object"
          ? (envelope.event as { id?: string }).id
          : undefined;
        if (envelope.id && inner?.type === "extension_ui_request" && inner.method === "notify") {
          if (typeof inner.message === "string") {
            useApp.getState().addExtensionNotice(envelope.id, {
              id: requestId ?? `${Date.now()}`,
              message: inner.message,
              notifyType: inner.notifyType ?? "info",
            });
          }
        }
        if (envelope.id && inner?.type === "extension_ui_request" && inner.method === "setStatus") {
          if (typeof inner.statusKey === "string") {
            useApp.getState().setExtensionStatus(envelope.id, inner.statusKey, inner.statusText);
          }
        }
        if (envelope.id && inner?.type === "extension_ui_request" && inner.method === "setTitle") {
          if (typeof inner.title === "string") useApp.getState().setExtensionTitle(envelope.id, inner.title);
        }
        if (envelope.id && inner?.type === "extension_ui_request" && inner.method === "set_editor_text") {
          if (typeof inner.text === "string") useApp.getState().setExtensionEditorText(envelope.id, inner.text);
        }
        if (
          envelope.id &&
          inner?.type === "extension_ui_request" &&
          inner.method === "setWidget" &&
          typeof inner.widgetKey === "string"
        ) {
          useApp
            .getState()
            .setExtensionWidget(
              envelope.id,
              inner.widgetKey,
              parseWidgetLines(inner.widgetLines),
              inner.widgetPlacement,
            );
        }
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
            inner.method === "input" ||
            inner.method === "editor")
        ) {

          if (requestId) {
            useApp.getState().addPendingAsk(envelope.id, {
              id: requestId,
              method: inner.method,
              title: inner.title,
              message: inner.message,
              placeholder: inner.placeholder,
              prefill: inner.prefill,
              options: inner.options as Array<DialogOption> | undefined,
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

function parseWidgetLines(widgetLines: unknown): string[] | undefined {
  if (widgetLines === undefined) return undefined;
  if (!Array.isArray(widgetLines)) return [];
  return widgetLines
    .filter((line): line is string => typeof line === "string")
    .map((line) => line.slice(0, 2000))
    .slice(0, 64);
}

function parseSubagentActivity(widgetLines: unknown): SubagentActivity | null {
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
        id?: unknown;
        status?: unknown;
        title?: unknown;
        backend?: unknown;
        model?: unknown;
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
        id: typeof row.id === "string" ? row.id : undefined,
        title: row.title,
        status: "running" as const,
        backend: typeof row.backend === "string" ? row.backend : undefined,
        model: typeof row.model === "string" ? row.model : undefined,
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
  await syncPendingAsks(id);
}

/** Replaces SSE-delivered asks with the server's authoritative live set. */
export async function syncPendingAsks(id: string) {
  try {
    const { asks } = await api.asks(id);
    if (currentThreadId() !== id) return;
    useApp
      .getState()
      .setPendingAsks(
        id,
        asks.map((ask) => ({
          id: ask.id,
          method: ask.method as PendingAsk["method"],
          title: ask.title ?? undefined,
          message: ask.message ?? undefined,
          placeholder: ask.placeholder ?? undefined,
          prefill: ask.prefill ?? undefined,
          options: ask.options as Array<DialogOption> | undefined,
        })),
      );
  } catch {
    /* transient; SSE events still deliver asks live */
  }
}
