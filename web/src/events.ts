/** Live updates: one EventSource fanning debounced store refreshes. */
import { api } from "~/api/client";
import { installAttention, notifyAttention, type AttentionKind } from "~/lib/attention";
import { parseJson } from "~/lib/storage";
import {
  useApp,
  type DialogOption,
  type PendingAsk,
  type RunningSubagent,
  type SubagentActivity,
  type SubagentHandoff,
} from "~/store";

const SUBAGENT_ACTIVITY_WIDGET = "pi-subagents/activity/v1";

export function listenErrors() {
  window.addEventListener("pecan:error", (event) => {
    const detail = (event as CustomEvent<string>).detail;
    if (detail) useApp.getState().pushToast(detail);
  });
}

let source: EventSource | null = null;
let retryMs = 500;
let retryTimer: ReturnType<typeof setTimeout> | null = null;
/** Last event sequence seen; the server replays everything after it on
 * reconnect (or answers `reset` when the gap is too old). */
let lastSeq: number | null = null;
let hiddenAt: number | null = null;
/** A backgrounded phone may keep a dead socket that never errors; after this
 * long hidden, reconnect on return instead of trusting it. */
const STALE_HIDDEN_MS = 10_000;

function trackSeq(event: Event) {
  const seq = Number((event as MessageEvent).lastEventId);
  if (Number.isSafeInteger(seq) && seq > 0) lastSeq = seq;
}

function sessionTitle(id: string): string | undefined {
  const state = useApp.getState();
  if (state.thread?.summary.id === id) return state.thread.summary.title ?? undefined;
  return state.sessions.find((row) => row.id === id)?.title ?? undefined;
}

function attention(kind: AttentionKind, id: string) {
  notifyAttention(kind, id, sessionTitle(id));
}

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

let started = false;
let openStream: (() => void) | null = null;

/** Reopens the stream after this browser (re)pairs. */
export function resumeEvents() {
  if (source || retryTimer !== null) return;
  retryMs = 500;
  openStream?.();
}

export function connectEvents() {
  if (started) return; // StrictMode double-effect guard.
  started = true;
  installAttention();
  const open = () => {
    retryTimer = null;
    // An unpaired browser would only collect 401s; pairing resumes it.
    if (!useApp.getState().paired) return;
    source = new EventSource(lastSeq === null ? "/api/events" : `/api/events?after=${lastSeq}`);
    source.onopen = () => {
      retryMs = 500;
      useApp.getState().setConnected(true);
    };
    // First frame of every connection. `reset` means the server could not
    // replay the gap (restart or too old): refetch instead of trusting deltas.
    source.addEventListener("ready", (event) => {
      const ready = parseJson((event as MessageEvent<string>).data) as { head?: unknown; reset?: unknown };
      if (typeof ready?.head === "number") lastSeq ??= ready.head;
      if (ready?.reset === true) resync(ready.head);
    });
    source.addEventListener("reset", (event) => {
      const reset = parseJson((event as MessageEvent<string>).data) as { head?: unknown };
      resync(reset?.head);
    });
    source.addEventListener("index-changed", (event) => {
      trackSeq(event);
      retryMs = 500;
      useApp.getState().setConnected(true);
      refreshSessionsDebounced();
      refreshThreadDebounced();
    });
    source.addEventListener("agent", (event) => {
      trackSeq(event);
      useApp.getState().setConnected(true);
      const payload: unknown = parseJson((event as MessageEvent<string>).data);
      if (typeof payload !== "object" || payload === null) return;
      const envelope = payload as { type?: string; id?: string; event?: unknown };

      // Worker events arrive wrapped: {type:"agent-event", id, event:{…}}.
      if (envelope.type === "agent-event") {
        const eventType =
          typeof envelope.event === "object" && envelope.event !== null
            ? (envelope.event as { type?: string }).type
            : undefined;
        if (envelope.id && envelope.id === currentThreadId()) {
          // The open session's run state follows the worker lifecycle directly.
          if (eventType === "agent_start") useApp.getState().setStreaming(true);
          if (eventType === "agent_settled" || eventType === "worker_exit") {
            useApp.getState().setStreaming(false);
            window.dispatchEvent(new CustomEvent("pecan:agent-refresh", { detail: envelope.id }));
          }
        }
        if (envelope.id && eventType === "agent_settled") attention("turn-done", envelope.id);
        if (envelope.id && eventType === "worker_exit") {
          // The Pi process is gone: nothing is running and no dialog can be
          // answered any more.
          useApp.getState().setPendingAsks(envelope.id, []);
          if (draftId === envelope.id) resetDraft(null);
          refreshSessionsDebounced();
        }
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
            attention("needs-input", envelope.id);
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
      // EventSource hides the status: probe once so a revoked device lands on
      // the pair screen (the client flips `paired`) instead of retrying forever.
      void api.health().catch(() => undefined);
      retryTimer = setTimeout(open, Math.min(retryMs, 8000));
      retryMs *= 2;
    };
  };
  openStream = open;
  open();

  // Returning to the tab or regaining network: reconnect now rather than
  // waiting out the backoff, and replace a socket that may have died silently.
  const reconnectNow = (force: boolean) => {
    // Events may have arrived while the refetches they trigger were failing
    // offline; the replay cursor cannot recover those, so refetch as well.
    refreshSessionsDebounced();
    refreshThreadDebounced();
    if (source && !force) return;
    if (retryTimer !== null) clearTimeout(retryTimer);
    source?.close();
    source = null;
    retryMs = 500;
    open();
  };
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      hiddenAt = Date.now();
      return;
    }
    const stale = hiddenAt !== null && Date.now() - hiddenAt > STALE_HIDDEN_MS;
    hiddenAt = null;
    reconnectNow(stale);
  });
  window.addEventListener("online", () => reconnectNow(true));

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

const ESCAPE_CHARACTER = String.fromCharCode(27);
const BELL_CHARACTER = String.fromCharCode(7);
const ANSI_OSC_SEQUENCE = new RegExp(
  `${ESCAPE_CHARACTER}\\][^${BELL_CHARACTER}]*(?:${BELL_CHARACTER}|${ESCAPE_CHARACTER}\\\\)`,
  "g",
);
const ANSI_CSI_SEQUENCE = new RegExp(
  `${ESCAPE_CHARACTER}\\[[0-?]*[ -/]*[@-~]`,
  "g",
);

/** Extension widgets may contain terminal styling, but the browser surface is
 * semantic text. Strip escape sequences at the event boundary. */
function cleanWidgetLine(line: string): string {
  return line
    .replace(ANSI_OSC_SEQUENCE, "")
    .replace(ANSI_CSI_SEQUENCE, "")
    .replaceAll("\r", "");
}

function parseWidgetLines(widgetLines: unknown): string[] | undefined {
  if (widgetLines === undefined) return undefined;
  if (!Array.isArray(widgetLines)) return [];
  return widgetLines
    .filter((line): line is string => typeof line === "string")
    .map((line) => cleanWidgetLine(line).slice(0, 2000))
    .slice(0, 64);
}

const MAX_ACTIVITY_CHILDREN = 4;
const MAX_ACTIVITY_BYTES = 15 * 1024;

function parseSubagentActivity(widgetLines: unknown): SubagentActivity | null {
  if (widgetLines === undefined) return null;
  if (!Array.isArray(widgetLines) || widgetLines.length !== 1) return null;
  const line = widgetLines[0];
  if (typeof line !== "string" || line.length > MAX_ACTIVITY_BYTES) return null;
  const snapshot = record(parseJson(line));
  if (
    !snapshot ||
    snapshot.version !== 1 ||
    finite(snapshot.revision) === undefined ||
    !Array.isArray(snapshot.children)
  ) {
    return null;
  }
  const children = snapshot.children.flatMap((child) => {
    const row = parseActivityChild(child);
    return row ? [row] : [];
  });
  const terminal = parseActivityTerminal(snapshot.terminal);
  return {
    revision: snapshot.revision as number,
    children: children.slice(0, MAX_ACTIVITY_CHILDREN),
    ...(terminal ? { terminal } : {}),
  };
}

function parseActivityChild(value: unknown): RunningSubagent | null {
  const row = record(value);
  if (!row) return null;
  const title = boundedText(row.title, 160);
  const startedAt = finite(row.startedAt);
  const lastActivityAt = finite(row.lastActivityAt);
  if (
    (row.status !== "running" && row.status !== "queued") ||
    !title ||
    startedAt === undefined ||
    lastActivityAt === undefined
  ) {
    return null;
  }
  const tools = Array.isArray(row.tools) ? row.tools : [];
  const lastTool = record(tools[tools.length - 1]);
  const toolName = lastTool ? boundedText(lastTool.name, 120) : undefined;
  const usage = record(row.usage);
  return {
    id: boundedText(row.id, 64),
    title,
    status: row.status,
    backend: boundedText(row.backend, 32),
    model: boundedText(row.model, 120),
    reasoningEffort: boundedText(row.reasoningEffort, 16),
    tool: toolName
      ? { name: toolName, args: boundedText(lastTool?.args, 512), isError: lastTool?.isError === true }
      : undefined,
    output: boundedText(row.output, 4096),
    failure: boundedText(row.failure, 2048),
    queuedMessages: Array.isArray(row.queued) ? Math.min(row.queued.length, 4) : 0,
    tokens: usage ? finite(usage.tokens) : undefined,
    contextWindow: usage ? finite(usage.contextWindow) : undefined,
    startedAt,
    lastActivityAt,
  };
}

function parseActivityTerminal(value: unknown): SubagentHandoff | undefined {
  const row = record(value);
  if (!row || (row.status !== "done" && row.status !== "error")) return undefined;
  const id = boundedText(row.id, 64);
  const title = boundedText(row.title, 160);
  const settledAt = finite(row.settledAt);
  if (!id || !title || settledAt === undefined) return undefined;
  return {
    id,
    title,
    status: row.status,
    output: boundedText(row.output, 4096) ?? "",
    failure: boundedText(row.failure, 2048),
    settledAt,
  };
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function finite(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** Non-empty string within the protocol's documented length limit. */
function boundedText(value: unknown, max: number): string | undefined {
  return typeof value === "string" && value.length > 0 && value.length <= max ? value : undefined;
}

/** Drops streaming state that the missed events would have updated and
 * refetches the authoritative view. */
function resync(head: unknown) {
  if (typeof head === "number") lastSeq = head;
  resetDraft(null);
  useApp.getState().setStreaming(false);
  refreshSessionsDebounced();
  refreshThreadDebounced();
  const id = currentThreadId();
  if (id) window.dispatchEvent(new CustomEvent("pecan:agent-refresh", { detail: id }));
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

async function refreshSessions() {
  let page: Awaited<ReturnType<typeof api.sessions>> | null = null;
  try {
    page = await api.sessions({});
  } catch {
    // The event server may be mid-restart; the next SSE reconnect retries this.
  }
  if (page) useApp.getState().setSessions(page.sessions);
}

async function refreshThread() {
  const id = useApp.getState().thread?.summary.id;
  if (!id) return;
  let thread: Awaited<ReturnType<typeof api.thread>> | null = null;
  try {
    thread = await api.thread(id);
  } catch {
    // Transient; the next SSE-triggered refresh or reconnect retries this.
  }
  if (thread && currentThreadId() === id && thread.summary.id === id) {
    useApp.getState().setThread(thread);
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
