/** Composer: message input with model/thinking/context meta and send modes. */

/**
 * Renders the composer into `el`.
 * @param {HTMLElement} el
 * @param {import("../main.js").AppState} state
 * @param {{onSend, onAbort, onAttach}} actions
 */
import { html } from "./html.js";

export function renderComposer(el, state, actions) {
  const streaming = state.streaming;
  const mode = state.sendMode;
  el.replaceChildren(html`
    <div class="composer-inner composer-wrap">
      ${state.modelPopover ? renderModelPopover(state) : ""}
      <textarea
        data-input
        rows="1"
        placeholder="${streaming ? "Steer or queue a message…" : "Message this thread…"}"
        ${!state.threadId && !streaming ? "" : ""}
      ></textarea>
      <div class="composer-meta">
        <button class="chip" data-models title="Switch model">${modelLabel(state)}</button>
        <button class="chip" data-thinking title="Cycle thinking level">
          thinking: ${state.thinking ?? "?"}
        </button>
        ${ctxMeter(state)}
        <span class="spacer"></span>
        ${streaming
          ? html`<span class="mode-hint">agent running —</span>
              <button class="chip" data-mode="steer" title="Deliver after current tool calls">
                steer${mode === "steer" ? " ✓" : ""}
              </button>
              <button class="chip" data-mode="queue" title="Wait for the agent to finish">
                queue${mode === "queue" ? " ✓" : ""}
              </button>
              <button class="abort-btn" data-abort>abort</button>`
          : html`<span class="mode-hint">↵ send · ⇧↵ newline</span>
              <button class="send-btn" data-send>Send</button>`}
      </div>
    </div>
  `);

  const textarea = el.querySelector("[data-input]");
  autosize(textarea);
  textarea.addEventListener("input", () => autosize(textarea));
  textarea.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit(el, state, actions);
    }
  });
  el.querySelector("[data-send]")?.addEventListener("click", () => submit(el, state, actions));
  el.querySelector("[data-abort]")?.addEventListener("click", actions.onAbort);
  el.querySelector("[data-models]")?.addEventListener("click", () => {
    if (!state.agent) actions.onAttach();
    document.dispatchEvent(new CustomEvent("pecan:toggle-model-popover"));
  });
  el.querySelector("[data-thinking]")?.addEventListener("click", () => {
    if (!state.agent) actions.onAttach();
    document.dispatchEvent(new CustomEvent("pecan:cycle-thinking"));
  });
  el.querySelectorAll("[data-mode]").forEach((btn) =>
    btn.addEventListener("click", () => {
      document.dispatchEvent(
        new CustomEvent("pecan:set-mode", { detail: btn.dataset.mode }),
      );
    }),
  );
  if (state.focusComposer) requestAnimationFrame(() => textarea.focus());
}

function submit(el, state, actions) {
  const textarea = el.querySelector("[data-input]");
  const text = textarea.value.trim();
  if (!text || !state.threadId) return;
  actions.onSend(text);
  textarea.value = "";
  autosize(textarea);
}

function modelLabel(state) {
  if (state.agent?.state?.model) {
    return `${state.agent.state.model.provider}/${shortModel(state.agent.state.model.id)}`;
  }
  return "model";
}

function shortModel(id) {
  return String(id).split("/").pop().split("-2025")[0].split("-2024")[0];
}

function ctxMeter(state) {
  const pct = state.contextPercent;
  if (pct == null) return "";
  const hot = pct >= 80 ? " hot" : "";
  return html`
    <span class="ctx-meter" title="context window used">
      <span class="ctx-bar"><span class="ctx-fill${hot}" style="width:${clamp(pct)}%"></span></span>
      ${Math.round(pct)}%
    </span>
  `;
}

function renderModelPopover(state) {
  const models = state.agent?.models ?? [];
  const current = state.agent?.state?.model?.id;
  return html`
    <div class="popover">
      ${models.length === 0 ? '<div class="popover-item">loading models…</div>' : ""}
      ${models.map(
        (m) => html`
          <button class="popover-item" data-pick-model="${m.id}" data-provider="${m.provider}">
            <span class="check">${m.id === current ? "✓" : ""}</span>
            <span class="row-title">${m.displayName ?? m.id}</span>
            <span class="row-time">${m.provider}</span>
          </button>
        `,
      )}
    </div>
  `;
}

function clamp(n) {
  return Math.max(0, Math.min(100, n));
}

function autosize(textarea) {
  textarea.style.height = "auto";
  textarea.style.height = `${Math.min(200, textarea.scrollHeight)}px`;
}

