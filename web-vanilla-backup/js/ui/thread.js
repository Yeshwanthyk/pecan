/** Thread rendering: folded older turns, tool-call details, ask_user cards. */

import { html } from "./html.js";

const OPEN_TURNS = 4; // most recent turns rendered expanded

/**
 * Renders a full thread view into `el`.
 * @param {HTMLElement} el
 * @param {{summary: object, settled: boolean, waitingAskuser: boolean,
 *          omitted: number, entries: object[], tasks: object[], workflows: object[]}} data
 * @param {{onSettle, onReopen}} actions
 */
export function renderThread(el, data, actions) {
  const s = data.summary;
  const turns = buildTurns(data.entries);
  const foldBefore = Math.max(0, turns.length - OPEN_TURNS);

  el.replaceChildren(html`
    <div class="thread-head">
      <span class="thread-title">${s.preview ?? s.id.slice(0, 8)}</span>
      <button class="ghost-btn danger" data-settle>${data.settled ? "Reopen" : "Settle"}</button>
    </div>
    <div class="thread-meta-row thread-meta">
      ${projectName(s.cwd)} · opened ${dateLabel(s.openedAt)} · ${s.model ?? "?"}
      ${data.waitingAskuser ? '· <span class="ask-state waiting">waiting for answer</span>' : ""}
      ${data.settled ? '· <span class="tag">settled</span>' : ""}
    </div>
    <div class="thread-entries">
      ${data.omitted > 0 ? html`<div class="omitted-note">${data.omitted} earlier entries omitted</div>` : ""}
      ${turns.map((turn, i) => renderTurn(turn, i < foldBefore))}
    </div>
    ${renderPanels(data)}
  `);

  el.querySelector("[data-settle]")?.addEventListener("click", () =>
    data.settled ? actions.onReopen(s.id) : actions.onSettle(s.id),
  );
}

function renderTurn(turn, folded) {
  if (!folded) {
    return html`<section class="turn">${turn.entries.map(renderEntry)}</section>`;
  }
  const label = turn.firstLine || "earlier prompt";
  return html`
    <details class="turn-fold">
      <summary><span class="row-time">${timeOf(turn)}</span> ${label}</summary>
      <div class="entry">${turn.entries.map(renderEntry)}</div>
    </details>
  `;
}

function renderEntry(dated) {
  const e = dated.entry;
  const time = dated.ts ? new Date(dated.ts).toLocaleTimeString([], { hour12: false }) : "";
  switch (e.kind) {
    case "user":
      return html`
        <div class="entry entry-user">
          <div class="who">you · <span class="model-tag">${time}</span></div>
          <div class="text">${e.text}</div>
        </div>
      `;
    case "assistant":
      return html`
        <div class="entry entry-assistant">
          <div class="who">
            pecan <span class="model-tag">${e.model ?? ""} · ${time}</span>
            ${(e.tools ?? []).map((t) => toolDetails(t))}
          </div>
          ${e.thinking ? html`<div class="thinking-block">${e.thinking}</div>` : ""}
          ${e.text ? html`<div class="text">${e.text}</div>` : ""}
        </div>
      `;
    case "askUser":
      return askCard(e);
    default:
      return "";
  }
}

/** Tool calls render as quiet toggle rows; the args stay one click away. */
function toolDetails(t) {
  const preview = (t.argsPreview ?? "").slice(0, 120);
  return html`
    <details class="tool">
      <summary><span class="row-title">${t.name}</span> <span class="row-sub">${preview}</span></summary>
      <pre class="args-pre">${t.argsPreview ?? ""}</pre>
    </details>
  `;
}

function askCard(e) {
  const questions = (e.questions && e.questions.questions) ?? [];
  const answered = e.answer != null;
  const answerText = answered ? extractText(e.answer) : null;
  return html`
    <div class="ask-card">
      <div class="ask-state ${answered ? "answered" : "waiting"}">
        ${answered ? "answered" : "waiting for your answer"}
      </div>
      ${questions.map(
        (q) => html`
          <div class="ask-q">${q.question}</div>
          ${(q.options ?? []).map((o) => html`<div class="ask-opt">${o.label}</div>`)}
        `,
      )}
      ${answered ? html`<div class="ask-q" style="font-weight:400;color:var(--ink-2)">→ ${answerText}</div>` : ""}
    </div>
  `;
}

function renderPanels(data) {
  const tasks = data.tasks ?? [];
  const flows = data.workflows ?? [];
  if (tasks.length === 0 && flows.length === 0) return "";
  return html`
    <div class="panels">
      ${tasks.length > 0
        ? html`
            <details class="panel" open>
              <summary>Tasks (${tasks.reduce((n, l) => n + l.tasks.length, 0)})</summary>
              ${tasks.flatMap((list) =>
                list.tasks.map(
                  (t) => html`
                    <div class="task-row">
                      <span class="task-status ${t.status}">${t.status.replace("_", " ")}</span>
                      <span>${t.subject}</span>
                    </div>
                  `,
                ),
              )}
            </details>
          `
        : ""}
      ${flows.length > 0
        ? html`
            <details class="panel">
              <summary>Workflows (${flows.length})</summary>
              ${flows.map(
                (wf) => html`
                  <div class="wf-row">
                    <span class="wf-name">${wf.name ?? wf.runId}</span>
                    <span class="wf-agents">
                      ${wf.agents.map((a) => `${a.label ?? "?"}:${a.state ?? "?"}`).join(" · ")}
                    </span>
                  </div>
                `,
              )}
            </details>
          `
        : ""}
    </div>
  `;
}

/** Groups flat entries into turns beginning at each user message. */
export function buildTurns(entries) {
  const turns = [];
  for (const dated of entries) {
    if (dated.entry.kind === "user" || turns.length === 0) {
      turns.push({ entries: [dated], firstLine: firstLineOf(dated), ts: dated.ts });
    } else {
      turns[turns.length - 1].entries.push(dated);
    }
  }
  return turns;
}

function firstLineOf(dated) {
  const e = dated.entry;
  if (e.kind !== "user") return "";
  return (e.text ?? "").split("\n")[0].slice(0, 90);
}

function extractText(content) {
  if (content == null) return "";
  if (typeof content === "string") return content.slice(0, 200);
  if (Array.isArray(content)) {
    return content
      .filter((b) => b.type === "text")
      .map((b) => b.text)
      .join(" ")
      .slice(0, 200);
  }
  return JSON.stringify(content).slice(0, 200);
}

function projectName(cwd) {
  return cwd.split("/").filter(Boolean).pop() ?? cwd;
}
function dateLabel(iso) {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleDateString();
}
function timeOf(turn) {
  if (!turn.ts) return "";
  return new Date(turn.ts).toLocaleTimeString([], { hour12: false });
}

