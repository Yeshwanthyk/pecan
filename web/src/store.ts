/** App-wide state: data cache + live connection status. */
import { create } from "zustand";

import type { AgentSnapshot, Bootstrap, SessionRow, ThreadView } from "./api/types";
import { readLocalStorage, writeLocalStorage } from "./lib/storage";

const THEME_KEY = "pecan:theme";

export type ThemeName = "earl-grey-light" | "one-dark";
type SendMode = "steer" | "queue";

/** One live child from the `pi-subagents/activity/v1` widget. */
export type RunningSubagent = {
  id?: string;
  title: string;
  status: "queued" | "running";
  backend?: string;
  model?: string;
  reasoningEffort?: string;
  /** Latest tool call, newest last in the widget. */
  tool?: { name: string; args?: string; isError: boolean };
  /** Tail of the child's streamed output. */
  output?: string;
  failure?: string;
  /** Steer/follow-up messages waiting for the child. */
  queuedMessages: number;
  tokens?: number;
  contextWindow?: number;
  startedAt: number;
  lastActivityAt: number;
};

/** The one child that just settled, shown briefly after it leaves `children`. */
export type SubagentHandoff = {
  id: string;
  title: string;
  status: "done" | "error";
  output: string;
  failure?: string;
  settledAt: number;
};

export type SubagentActivity = {
  revision: number;
  children: RunningSubagent[];
  terminal?: SubagentHandoff;
};

/** One select choice exactly as pi sent it: a bare string or a label/value pair. */
export type DialogOption = string | { label?: string; value?: string };

export type ExtensionNotice = {
  id: string;
  message: string;
  notifyType: "info" | "warning" | "error";
};

type ExtensionStatus = {
  key: string;
  text: string;
};

export type ExtensionWidget = {
  key: string;
  lines: string[];
  placement: "aboveEditor" | "belowEditor";
};

type ExtensionEditorText = {
  revision: number;
  text: string;
};

export type PendingAsk = {
  id: string;
  method: "select" | "confirm" | "input" | "editor";
  title?: string;
  /** Body text under the title of confirm dialogs. */
  message?: string;
  /** Placeholder hint for input dialogs. */
  placeholder?: string;
  /** Prefilled content for editor dialogs. */
  prefill?: string;
  options?: DialogOption[];
};

/** A transient message shown in the app-wide toaster. */
export type Toast = { id: number; message: string; tone: "error" | "info" };

type AppState = {
  toasts: Toast[];
  pushToast: (message: string, tone?: Toast["tone"]) => void;
  dismissToast: (id: number) => void;
  /** Project directory a new session is being started in, if any. */
  startingCwd: string | null;
  setStartingCwd: (cwd: string | null) => void;
  bootstrap: Bootstrap | null;
  /** `false` once the server refuses this browser as unpaired. */
  paired: boolean;
  setPaired: (paired: boolean) => void;
  sessions: SessionRow[];
  theme: ThemeName;
  connected: boolean;
  streaming: boolean;
  sendMode: SendMode;
  agent: AgentSnapshot | null;
  contextPercent: number | null;
  thread: ThreadView | null;
  settledFoldOpen: boolean;
  /** Partial assistant text currently streaming for one session. */
  streamingDraft: { id: string; text: string } | null;
  pendingAsks: Record<string, PendingAsk[]>;
  extensionNotices: Record<string, ExtensionNotice[]>;
  extensionStatuses: Record<string, Record<string, ExtensionStatus>>;
  extensionWidgets: Record<string, Record<string, ExtensionWidget>>;
  extensionTitles: Record<string, string>;
  extensionEditorText: Record<string, ExtensionEditorText>;
  subagentActivity: Record<string, SubagentActivity>;
  setBootstrap: (data: Bootstrap) => void;
  setSessions: (rows: SessionRow[]) => void;
  setThread: (thread: ThreadView | null) => void;
  setStreamingDraft: (draft: { id: string; text: string } | null) => void;
  setAgent: (agent: AgentSnapshot | null) => void;
  setStreaming: (streaming: boolean) => void;
  setConnected: (connected: boolean) => void;
  setTheme: (theme: ThemeName) => void;
  setSendMode: (mode: SendMode) => void;
  toggleTheme: () => void;
  toggleSettledFold: () => void;
  addPendingAsk: (sessionId: string, ask: PendingAsk) => void;
  removePendingAsk: (sessionId: string, requestId: string) => void;
  setPendingAsks: (sessionId: string, asks: PendingAsk[]) => void;
  addExtensionNotice: (sessionId: string, notice: ExtensionNotice) => void;
  removeExtensionNotice: (sessionId: string, noticeId: string) => void;
  setExtensionStatus: (sessionId: string, statusKey: string, statusText?: string) => void;
  setExtensionWidget: (
    sessionId: string,
    widgetKey: string,
    lines: string[] | undefined,
    placement?: "aboveEditor" | "belowEditor",
  ) => void;
  setExtensionTitle: (sessionId: string, title: string) => void;
  setExtensionEditorText: (sessionId: string, text: string) => void;
  setSubagentActivity: (sessionId: string, activity: SubagentActivity | null) => void;
};

function applyTheme(theme: ThemeName) {
  document.documentElement.classList.toggle("dark", theme === "one-dark");
  writeLocalStorage(THEME_KEY, theme);
}

const initialTheme = (): ThemeName =>
  readLocalStorage(THEME_KEY) === "one-dark" ? "one-dark" : "earl-grey-light";

applyTheme(initialTheme());

let toastSeq = 0;

export const useApp = create<AppState>((set) => ({
  toasts: [],
  pushToast: (message, tone = "error") =>
    set((current) => {
      // Collapse repeats of the newest message instead of stacking them.
      if (current.toasts.at(-1)?.message === message) return current;
      toastSeq += 1;
      return { toasts: [...current.toasts, { id: toastSeq, message, tone }].slice(-3) };
    }),
  dismissToast: (id) =>
    set((current) => ({ toasts: current.toasts.filter((toast) => toast.id !== id) })),
  startingCwd: null,
  setStartingCwd: (startingCwd) => set({ startingCwd }),
  bootstrap: null,
  paired: true,
  setPaired: (paired) => set({ paired }),
  sessions: [],
  theme: initialTheme(),
  connected: false,
  streaming: false,
  sendMode: "steer",
  agent: null,
  contextPercent: null,
  thread: null,
  settledFoldOpen: false,
  streamingDraft: null,
  setStreamingDraft: (draft) => set({ streamingDraft: draft }),
  pendingAsks: {},
  extensionNotices: {},
  extensionStatuses: {},
  extensionWidgets: {},
  extensionTitles: {},
  extensionEditorText: {},
  subagentActivity: {},
  setBootstrap: (bootstrap) => set({ bootstrap }),
  setSessions: (sessions) => set({ sessions }),
  setThread: (thread) => set({ thread }),
  setAgent: (agent) =>
    set({
      agent,
      streaming: agent?.state.isStreaming === true,
      contextPercent: agent?.stats?.contextUsage?.percent ?? null,
    }),
  setStreaming: (streaming) => set({ streaming }),
  setConnected: (connected) => set({ connected }),
  setTheme: (theme) => {
    applyTheme(theme);
    set({ theme });
  },
  setSendMode: (sendMode) => set({ sendMode }),
  toggleTheme: () =>
    set((current) => {
      const theme: ThemeName =
        current.theme === "one-dark" ? "earl-grey-light" : "one-dark";
      applyTheme(theme);
      return { theme };
    }),
  toggleSettledFold: () =>
    set((current) => ({ settledFoldOpen: !current.settledFoldOpen })),
  addPendingAsk: (sessionId, ask) =>
    set((current) => {
      const existing = current.pendingAsks[sessionId] ?? [];
      if (existing.some((candidate) => candidate.id === ask.id)) return current;
      return {
        pendingAsks: { ...current.pendingAsks, [sessionId]: [...existing, ask] },
      };
    }),
  removePendingAsk: (sessionId, requestId) =>
    set((current) => ({
      pendingAsks: {
        ...current.pendingAsks,
        [sessionId]: (current.pendingAsks[sessionId] ?? []).filter(
          (ask) => ask.id !== requestId,
        ),
      },
    })),
  setPendingAsks: (sessionId, asks) =>
    set((current) => ({
      pendingAsks: { ...current.pendingAsks, [sessionId]: asks },
    })),
  addExtensionNotice: (sessionId, notice) =>
    set((current) => ({
      extensionNotices: {
        ...current.extensionNotices,
        [sessionId]: [...(current.extensionNotices[sessionId] ?? []), notice].slice(-4),
      },
    })),
  removeExtensionNotice: (sessionId, noticeId) =>
    set((current) => ({
      extensionNotices: {
        ...current.extensionNotices,
        [sessionId]: (current.extensionNotices[sessionId] ?? []).filter((notice) => notice.id !== noticeId),
      },
    })),
  setExtensionStatus: (sessionId, statusKey, statusText) =>
    set((current) => {
      const statuses = { ...current.extensionStatuses[sessionId] };
      if (statusText === undefined) delete statuses[statusKey];
      else statuses[statusKey] = { key: statusKey, text: statusText };
      return { extensionStatuses: { ...current.extensionStatuses, [sessionId]: statuses } };
    }),
  setExtensionWidget: (sessionId, widgetKey, lines, placement = "belowEditor") =>
    set((current) => {
      const widgets = { ...current.extensionWidgets[sessionId] };
      if (lines === undefined) delete widgets[widgetKey];
      else widgets[widgetKey] = { key: widgetKey, lines, placement };
      return { extensionWidgets: { ...current.extensionWidgets, [sessionId]: widgets } };
    }),
  setExtensionTitle: (sessionId, title) =>
    set((current) => ({ extensionTitles: { ...current.extensionTitles, [sessionId]: title } })),
  setExtensionEditorText: (sessionId, text) =>
    set((current) => ({
      extensionEditorText: {
        ...current.extensionEditorText,
        [sessionId]: {
          revision: (current.extensionEditorText[sessionId]?.revision ?? 0) + 1,
          text,
        },
      },
    })),
  setSubagentActivity: (sessionId, activity) =>
    set((current) => {
      const previous = current.subagentActivity[sessionId];
      if (activity && previous && activity.revision <= previous.revision) return current;
      const next = { ...current.subagentActivity };
      if (activity) next[sessionId] = activity;
      else delete next[sessionId];
      return { subagentActivity: next };
    }),
}));
