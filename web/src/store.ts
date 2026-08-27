/** App-wide state: data cache + live connection status. */
import { create } from "zustand";

import type { AgentSnapshot, Bootstrap, SessionRow, ThreadView } from "./api/types";

export type ThemeName = "earl-grey-light" | "one-dark";
export type SendMode = "steer" | "queue";

export type RunningSubagent = {
  id?: string;
  title: string;
  status?: "running" | "settled";
  backend?: string;
  model?: string;
  startedAt: number;
  lastActivityAt: number;
};

export type SubagentActivity = {
  revision: number;
  children: RunningSubagent[];
};

/** One select choice exactly as pi sent it: a bare string or a label/value pair. */
export type DialogOption = string | { label?: string; value?: string };

export type ExtensionNotice = {
  id: string;
  message: string;
  notifyType: "info" | "warning" | "error";
};

export type ExtensionStatus = {
  key: string;
  text: string;
};

export type ExtensionWidget = {
  key: string;
  lines: string[];
  placement: "aboveEditor" | "belowEditor";
};

export type ExtensionEditorText = {
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

type AppState = {
  bootstrap: Bootstrap | null;
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
  try {
    localStorage.setItem("pecan:theme", theme);
  } catch {
    /* private mode */
  }
}

const initialTheme = (): ThemeName => {
  try {
    return localStorage.getItem("pecan:theme") === "one-dark" ? "one-dark" : "earl-grey-light";
  } catch {
    return "earl-grey-light";
  }
};

applyTheme(initialTheme());

export const useApp = create<AppState>((set) => ({
  bootstrap: null,
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
