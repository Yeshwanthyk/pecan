/** pecan app shell: state, routing, SSE wiring. */

import * as api from "./api.js";
import { connectEvents } from "./sse.js";
import { renderSidebar } from "./ui/sidebar.js";
import { renderThread } from "./ui/thread.js";
import { renderComposer } from "./ui/composer.js";

/** @typedef {{
 *   projects: Array<{cwd: string, name: string, sessionCount: number}>,
 *   sessions: Array<object>,
 *   project: string|null,
 *   sessionId: string|null,
 *   thread: object|null,
 *   limit: number,
 *   hasMore: boolean,
 *   theme: string,
 *   streaming: boolean,
 *   sendMode: string,
 *   agent: object|null,
 *   contextPercent: number|null,
 *   thinking: string|null,
 *   modelPopover: boolean,
 *   focusComposer: boolean,
 * }} AppState */

const state = /** @type {AppState} */ ({
  projects: [],
  sessions: [],
  project: null,
  sessionId: null,
  thread: null,
  limit: 80,
  hasMore: false,
  theme: localStorage.getItem("pecan:theme") ?? "earl-grey-light",
  streaming: false,
  sendMode: "steer",
  agent: null,
  contextPercent: null,
  thinking: null,
  modelPopover: false,
  focusComposer: false,
  addingProject: false,
  projectDraft: "",
});

const sidebarEl = document.getElementById("sidebar");
const mainEl = document.getElementById("main");

// ------------------------------------------------------------ rendering ----

function render() {
  document.documentElement.dataset.theme = state.theme;
  const sidebar = document.createElement("div");
  renderSidebar(sidebar, state, {
    onProject: (cwd) => {
      state.project = state.project === cwd ? null : cwd;
      state.limit = 80;
      location.hash = state.project ? `#/p/${encodeURIComponent(state.project)}` : "#/";
    },
    onSession: (id) => {
      location.hash = `#/s/${id}`;
    },
    onLoadMore: () => {
      state.limit += 120;
      void loadSessions();
    },
    onAddProject: (cwd) => void addProject(cwd),
    onRemoveProject: (cwd) => void removeProject(cwd),
  });
  sidebarEl.replaceChildren(...sidebar.childNodes);

  if (!state.sessionId) {
    mainEl.replaceChildren(emptyMain());
    return;
  }
  if (state.sessionId && !state.thread) {
    mainEl.replaceChildren(loadingMain());
    return;
  }

  const scroll = document.createElement("div");
  scroll.className = "thread-scroll";
  const inner = document.createElement("div");
  inner.className = "thread-inner";
  renderThread(inner, state.thread, {
    onSettle: (id) => void api.post(`/api/session/${id}/settle`),
    onReopen: (id) => void api.del(`/api/session/${id}/settle`),
  });
  scroll.appendChild(inner);

  const composer = document.createElement("div");
  composer.className = "composer";
  renderComposer(composer, state, {
    onSend: (text) => void sendMessage(text),
    onAbort: () => void abortAgent(),
    onAttach: () => void attachAgent(),
  });

  mainEl.replaceChildren(scroll, composer);
}

function emptyMain() {
  const div = document.createElement("div");
  div.className = "empty-state";
  div.innerHTML =
    '<p class="empty-title">Select a session from the sidebar</p>' +
    '<p class="empty-sub">Settle threads you are done with; reopen them any time.</p>';
  return div;
}

function loadingMain() {
  const div = document.createElement("div");
  div.className = "empty-state";
  div.innerHTML = '<p class="empty-title">Loading thread…</p>';
  return div;
}

// ------------------------------------------------------------- data flow ----

async function loadBootstrap() {
  const data = await api.get("/api/bootstrap");
  state.projects = data.projects;
  applySessionList(data.sessions);
  render();
}

async function loadSessions() {
  const params = new URLSearchParams({ limit: "400" });
  const data = await api.get(`/api/sessions?${params}`);
  applySessionList(data.sessions);
  render();
}

function applySessionList(rows) {
  state.sessions = rows.map((row) => ({ ...row }));
}

async function openThread(id) {
  state.sessionId = id;
  state.thread = null;
  state.agent = null;
  state.contextPercent = null;
  state.thinking = null;
  state.streaming = false;
  state.modelPopover = false;
  state.focusComposer = true;
  render();
  try {
    const data = await api.get(`/api/session/${id}`);
    state.thread = data;
    render();
    // Attach lazily so the composer shows live agent state.
    void attachAgent();
  } catch (error) {
    state.sessionId = null;
    showError(String(error));
  }
}

async function attachAgent() {
  const id = state.sessionId;
  if (!id || state.agent) return;
  try {
    const agent = await api.post(`/api/session/${id}/agent-attach`);
    state.agent = agent;
    state.thinking = agent.state?.thinkingLevel ?? null;
    state.streaming = agent.state?.isStreaming === true;
    state.contextPercent = agent.stats?.contextUsage?.percent ?? null;
    render();
  } catch {
    /* worker unavailable; composer still works for reads */
  }
}

let pollTimer = null;

function startPolling() {
  clearInterval(pollTimer);
  pollTimer = setInterval(() => {
    if (!state.sessionId || !state.agent) return;
    void refreshAgentState();
  }, 2500);
}

async function refreshAgentState() {
  const id = state.sessionId;
  try {
    const agent = await api.get(`/api/session/${id}/agent-attach`);
    state.agent = agent;
    state.thinking = agent.state?.thinkingLevel ?? state.thinking;
    state.streaming = agent.state?.isStreaming === true;
    state.contextPercent = agent.stats?.contextUsage?.percent ?? null;
    render();
  } catch {
    state.streaming = false;
  }
}

async function sendMessage(text) {
  const id = state.sessionId;
  if (!id) return;
  const mode = state.streaming ? state.sendMode : "send";
  try {
    await api.post(`/api/session/${id}/message`, { text, mode });
    state.streaming = mode !== "queue";
  } catch (error) {
    showError(String(error.message ?? error));
  }
}

async function abortAgent() {
  const id = state.sessionId;
  if (!id) return;
  await api.post(`/api/session/${id}/abort`);
  state.streaming = false;
  render();
}

async function addProject(cwd) {
  if (!cwd || !cwd.startsWith("/")) {
    showError("Project path must be absolute");
    return;
  }
  try {
    await api.post("/api/projects", { cwd });
    state.addingProject = false;
    state.projectDraft = "";
    await loadSessions();
    const found = state.projects.some((p) => p.cwd === cwd);
    if (!found) await loadBootstrap();
    render();
  } catch (error) {
    showError(String(error.message ?? error));
  }
}

async function removeProject(cwd) {
  try {
    await api.del("/api/projects", { cwd });
    if (state.project === cwd) {
      state.project = null;
      location.hash = "#/";
    }
    await Promise.all([loadBootstrap(), loadSessions()]);
  } catch (error) {
    showError(String(error.message ?? error));
  }
}

function showError(message) {
  const bar = document.createElement("div");
  bar.className = "omitted-note";
  bar.style.color = "var(--err)";
  bar.textContent = message;
  mainEl.prepend(bar);
  setTimeout(() => bar.remove(), 6000);
}

// ------------------------------------------------------------------ sse ----

connectEvents({
  "index-changed": async () => {
    await loadSessions();
  },
  agent: (payload) => {
    switch (payload.type) {
      case "thread-changed":
        if (state.sessionId === payload.id) void openThread(payload.id);
        break;
      case "agent-event": {
        if (state.sessionId !== payload.id) break;
        const kind = payload.event?.type;
        if (kind === "agent_start") {
          state.streaming = true;
          render();
        } else if (kind === "agent_settled" || kind === "agent_end") {
          state.streaming = false;
          void refreshAgentState();
        } else if (kind === "queue_update") {
          const steer = payload.event.steering?.length ?? 0;
          const followUp = payload.event.followUp?.length ?? 0;
          if (steer === 0 && followUp === 0) state.streaming = state.streaming;
          else state.sendMode = followUp > steer ? "queue" : "steer";
          render();
        } else if (kind === "message_update") {
          state.streaming = true;
        }
        break;
      }
      default:
        break;
    }
  },
});

// ---------------------------------------------------------------- events ----

document.addEventListener("pecan:start-add-project", () => {
  state.addingProject = true;
  render();
});

document.addEventListener("pecan:cancel-add-project", () => {
  state.addingProject = false;
  render();
});

document.addEventListener("pecan:toggle-theme", () => {
  state.theme = state.theme === "one-dark" ? "earl-grey-light" : "one-dark";
  localStorage.setItem("pecan:theme", state.theme);
  render();
});

document.addEventListener("pecan:toggle-model-popover", () => {
  state.modelPopover = !state.modelPopover;
  render();
});

document.addEventListener("click", (e) => {
  const pick = e.target.closest("[data-pick-model]");
  if (pick) {
    void pickModel(pick.dataset.pickModel, pick.dataset.provider);
    e.stopPropagation();
    return;
  }
  if (state.modelPopover && !e.target.closest(".composer-wrap")) {
    state.modelPopover = false;
    render();
  }
});

document.addEventListener("pecan:set-mode", (e) => {
  state.sendMode = e.detail;
  render();
});

document.addEventListener("pecan:cycle-thinking", () => {
  const levels = ["off", "minimal", "low", "medium", "high"];
  const idx = levels.indexOf(state.thinking ?? "medium");
  void setThinking(levels[(idx + 1) % levels.length]);
});

window.addEventListener("hashchange", route);

// ---------------------------------------------------------------- routing ----

async function route() {
  const hash = location.hash;
  if (hash.startsWith("#/s/")) {
    state.project = null;
    await openThread(hash.slice(4));
  } else if (hash.startsWith("#/p/")) {
    state.sessionId = null;
    state.thread = null;
    state.project = decodeURIComponent(hash.slice(4));
    state.limit = 80;
    render();
  }
}

async function pickModel(modelId, provider) {
  const id = state.sessionId;
  if (!id || !state.agent) return;
  try {
    await fetch(`/api/session/${id}/agent-attach`, { method: "POST" });
    // Model switching rides the same worker via a dedicated command below.
    await api.post(`/api/session/${id}/set-model`, { provider, modelId });
    await refreshAgentState();
  } catch (error) {
    showError(String(error.message ?? error));
  }
  state.modelPopover = false;
  render();
}

async function setThinking(level) {
  const id = state.sessionId;
  if (!id || !state.agent) return;
  try {
    await api.post(`/api/session/${id}/set-thinking`, { level });
    state.thinking = level;
    render();
  } catch (error) {
    showError(String(error.message ?? error));
  }
}

// ------------------------------------------------------------------ boot ----

void (async function boot() {
  document.getElementById("app").ariaBusy = "false";
  await loadBootstrap();
  startPolling();
  await route();
})();
