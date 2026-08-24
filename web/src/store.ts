/** App-wide state: data cache + live connection status. */
import { create } from "zustand";

import type { AgentSnapshot, Bootstrap, SessionRow, ThreadView } from "./api/types";

export type ThemeName = "earl-grey-light" | "one-dark";
export type SendMode = "steer" | "queue";

export type RunningSubagent = {
  title: string;
  startedAt: number;
  lastActivityAt: number;
};

export type SubagentActivity = {
  revision: number;
  children: RunningSubagent[];
};

export type PendingAsk = {
  id: string;
  method: "select" | "confirm" | "input";
  title?: string;
  options?: Array<{ label?: string; value?: string }>;
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
  pendingAsks: Record<string, PendingAsk[]>;
  subagentActivity: Record<string, SubagentActivity>;
  setBootstrap: (data: Bootstrap) => void;
  setSessions: (rows: SessionRow[]) => void;
  setThread: (thread: ThreadView | null) => void;
  setAgent: (agent: AgentSnapshot | null) => void;
  setStreaming: (streaming: boolean) => void;
  setConnected: (connected: boolean) => void;
  setTheme: (theme: ThemeName) => void;
  setSendMode: (mode: SendMode) => void;
  toggleTheme: () => void;
  toggleSettledFold: () => void;
  addPendingAsk: (sessionId: string, ask: PendingAsk) => void;
  removePendingAsk: (sessionId: string, requestId: string) => void;
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
  pendingAsks: {},
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
