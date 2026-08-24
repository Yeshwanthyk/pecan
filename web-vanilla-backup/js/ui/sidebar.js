/** Sidebar: added projects (with add/remove), sessions by open time, settled fold. */

import { html } from "./html.js";

/**
 * Renders the sidebar into `el`.
 * @param {HTMLElement} el
 * @param {import("../main.js").AppState} state
 * @param {{onProject, onSession, onLoadMore, onAddProject, onRemoveProject}} actions
 */
export function renderSidebar(el, state, actions) {
  const projects = html`
    <div class="section-head">
      <span class="side-label">Projects</span>
      <button
        class="icon-btn"
        data-add-project
        title="Add a project folder"
        aria-label="Add project"
      >+</button>
    </div>
    ${state.addingProject ? addProjectRow(state) : ""}
    ${state.projects.length === 0 && !state.addingProject
      ? html`<div class="side-empty">No projects yet — press + to add one.</div>`
      : ""}
    ${state.projects.map(
      (p) => html`
        <div class="project-row-wrap ${state.project === p.cwd ? "selected" : ""}">
          <button class="project-row" data-cwd="${p.cwd}" title="${p.cwd}">
            <span class="row-top">
              <span class="row-title">${p.name}</span>
              <span class="row-time">${p.sessionCount}</span>
            </span>
          </button>
          <button
            class="icon-btn row-action"
            data-remove-project="${p.cwd}"
            title="Remove ${p.name} from sidebar"
            aria-label="Remove project"
          >×</button>
        </div>
      `,
    )}
  `;

  const visible = state.sessions.filter((s) => !state.project || s.cwd === state.project);
  const active = visible.filter((s) => !s.settled);
  const settled = visible.filter((s) => s.settled);
  const shown = active.slice(0, state.limit);

  const sessions = html`
    <div class="section-head">
      <span class="side-label">Sessions</span>
      ${state.project
        ? html`<button class="icon-btn" data-clear-filter title="Show all projects">×</button>`
        : ""}
    </div>
    ${shown.map((s) => sessionRow(s, state))}
    ${settled.length > 0 ? settledFold(settled, state) : ""}
    ${active.length === 0 ? html`<div class="side-empty">No unsettled sessions</div>` : ""}
    ${active.length > state.limit || state.hasMore
      ? html`<button class="load-more" data-more>Load older sessions</button>`
      : ""}
  `;

  el.replaceChildren(html`
    <div class="wordmark">
      pecan
      <button class="theme-toggle" data-theme-toggle title="Switch theme">
        ${state.theme === "one-dark" ? "light" : "dark"}
      </button>
    </div>
    <nav>${projects}</nav>
    <nav>${sessions}</nav>
  `);

  // wire actions
  el.querySelector("[data-add-project]")?.addEventListener("click", () => {
    document.dispatchEvent(new CustomEvent("pecan:start-add-project"));
  });
  const input = el.querySelector("[data-project-input]");
  if (input) {
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") actions.onAddProject(input.value.trim());
      if (e.key === "Escape") document.dispatchEvent(new CustomEvent("pecan:cancel-add-project"));
    });
  }
  el.querySelectorAll(".project-row").forEach((btn) =>
    btn.addEventListener("click", () => actions.onProject(btn.dataset.cwd)),
  );
  el.querySelectorAll("[data-remove-project]").forEach((btn) =>
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      actions.onRemoveProject(btn.dataset.removeProject);
    }),
  );
  el.querySelector("[data-clear-filter]")?.addEventListener("click", () => {
    location.hash = "#/";
  });
  el.querySelectorAll(".session-row").forEach((btn) =>
    btn.addEventListener("click", () => actions.onSession(btn.dataset.id)),
  );
  el.querySelector("[data-more]")?.addEventListener("click", actions.onLoadMore);
  el.querySelector("[data-theme-toggle]")?.addEventListener("click", () => {
    document.dispatchEvent(new CustomEvent("pecan:toggle-theme"));
  });
}

function addProjectRow(state) {
  return html`
    <div class="add-project-row">
      <input
        data-project-input
        type="text"
        placeholder="/absolute/path/to/project"
        value="${state.projectDraft ?? ""}"
        spellcheck="false"
        autofocus
      />
    </div>
  `;
}

function sessionRow(s, state) {
  return html`
    <button class="session-row ${state.sessionId === s.id ? "selected" : ""}" data-id="${s.id}">
      <span class="row-top">
        <span class="badges">
          ${s.waitingAskuser ? '<span class="dot waiting" title="waiting for answer"></span>' : ""}
          ${s.kind === "subagent" ? '<span class="tag">sub</span>' : ""}
        </span>
        <span class="row-title">${s.preview ?? s.id.slice(0, 8)}</span>
        <span class="row-time">${timeLabel(s.openedAt)}</span>
      </span>
      <span class="row-sub">${projectName(s.cwd)} · ${relative(s.lastActivity)}</span>
    </button>
  `;
}

function settledFold(rows, state) {
  return html`
    <details class="settled-fold" ${localStorage.getItem("pecan:fold-open") === "1" ? "open" : ""}>
      <summary>Settled (${rows.length})</summary>
      ${rows.map((s) => sessionRow(s, state))}
    </details>
  `;
}

function projectName(cwd) {
  return cwd.split("/").filter(Boolean).pop() ?? cwd;
}

function timeLabel(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return hhmm(d);
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

function relative(iso) {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const secs = Math.max(0, (Date.now() - then) / 1000);
  if (secs < 90) return "now";
  if (secs < 3600) return `${Math.round(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.round(secs / 3600)}h ago`;
  return `${Math.round(secs / 86400)}d ago`;
}

function hhmm(d) {
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
