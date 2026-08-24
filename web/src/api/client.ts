/**
 * The only module allowed to touch `fetch`. Every response crosses a TypeBox
 * boundary before it reaches the app, so malformed server data fails loudly
 * here instead of leaking `undefined` into components.
 */
import {
  Array,
  Boolean,
  Null,
  Object,
  String as TString,
  Union,
  type Static,
  type TSchema,
} from "@sinclair/typebox";
import { Value } from "@sinclair/typebox/value";

import * as types from "./types";

export type TitleModelPreset = "luna-low" | "luna-medium" | "sol-low";

const TITLE_MODEL_PRESET_KEY = "pecan:title-model-preset";
const AI_TITLE_GENERATION_KEY = "pecan:ai-title-generation";
const SHIP_TOKEN_KEY = "pecan:ship-token";
const TITLE_MODEL_PRESETS = new Set<TitleModelPreset>([
  "luna-low",
  "luna-medium",
  "sol-low",
]);

export function readTitleModelPreset(): TitleModelPreset {
  try {
    const stored = localStorage.getItem(TITLE_MODEL_PRESET_KEY);
    if (stored && TITLE_MODEL_PRESETS.has(stored as TitleModelPreset)) {
      return stored as TitleModelPreset;
    }
  } catch {
    // Browser privacy settings may make localStorage unavailable.
  }
  return "luna-low";
}

export function saveTitleModelPreset(preset: TitleModelPreset) {
  try {
    localStorage.setItem(TITLE_MODEL_PRESET_KEY, preset);
  } catch {
    // The in-memory selection still works when persistence is unavailable.
  }
}

/** AI title generation is opt-in so browsing sessions never spend model tokens. */
export function readAiTitleGenerationEnabled(): boolean {
  try {
    return localStorage.getItem(AI_TITLE_GENERATION_KEY) === "true";
  } catch {
    return false;
  }
}

export function saveAiTitleGenerationEnabled(enabled: boolean) {
  try {
    localStorage.setItem(AI_TITLE_GENERATION_KEY, String(enabled));
  } catch {
    // The current page still receives the settings event when persistence fails.
  }
  window.dispatchEvent(
    new CustomEvent("pecan:title-settings", { detail: { enabled } }),
  );
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Schema for endpoints whose bodies we ignore. */
const Ack = Object({});

async function request<T extends TSchema>(
  method: "GET" | "POST" | "DELETE",
  path: string,
  schema: T,
  body?: unknown,
  capability?: "ship",
): Promise<Static<T>> {
  const shipToken = capability === "ship" ? readShipToken() : null;
  const response = await fetch(path, {
    method,
    headers: {
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(shipToken ? { "x-pecan-ship-token": shipToken } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) as string }),
  });
  if (!response.ok) {
    const detail = await response
      .json()
      .then((data: unknown) =>
        typeof data === "object" && data !== null && "error" in data
          ? String((data as { error: unknown }).error)
          : response.statusText,
      )
      .catch(() => response.statusText);
    throw new ApiError(detail, response.status);
  }
  const data: unknown = await response.json();
  if (!Value.Check(schema, data)) {
    const first = [...Value.Errors(schema, data)][0];
    throw new ApiError(
      `API shape mismatch at ${path}: ${first?.message ?? "invalid"} at ${first?.path ?? "?"}`,
      response.status,
    );
  }
  return data as Static<T>;
}

/** Captures the per-launch Ship capability, then removes it from the visible URL. */
export function captureShipToken() {
  const url = new URL(location.href);
  const token = url.searchParams.get("ship-token");
  if (!token) return;
  try {
    sessionStorage.setItem(SHIP_TOKEN_KEY, token);
  } catch {
    // The token remains unavailable if browser session storage is disabled.
  }
  url.searchParams.delete("ship-token");
  history.replaceState(history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

function readShipToken(): string | null {
  try {
    return sessionStorage.getItem(SHIP_TOKEN_KEY);
  } catch {
    return null;
  }
}

export const api = {
  bootstrap: () => request("GET", "/api/bootstrap", types.Bootstrap),
  sessions: (query: { project?: string; limit?: number }) => {
    const params = new URLSearchParams();
    if (query.project) params.set("project", query.project);
    params.set("limit", String(query.limit ?? 400));
    return request("GET", `/api/sessions?${params}`, types.SessionPage);
  },
  thread: (id: string) => request("GET", `/api/session/${id}`, types.ThreadView),
  gitBranch: (id: string) =>
    request("GET", `/api/session/${id}/git`, Object({ branch: Union([Null(), TString()]) })),
  workspaceDiff: (id: string) =>
    request("GET", `/api/session/${id}/diff`, types.WorkspaceDiff),
  shipPlan: (id: string) =>
    request("GET", `/api/session/${id}/ship`, types.ShipPlan),
  ship: (
    id: string,
    input: {
      branch: string;
      baseBranch: string;
      commitMessage: string;
      pullRequestTitle: string;
      pullRequestBody: string;
      reviewToken: string;
    },
  ) =>
    request(
      "POST",
      `/api/session/${id}/ship`,
      types.ShipResult,
      { confirmed: true, ...input },
      "ship",
    ),
  settle: async (id: string) => request("POST", `/api/session/${id}/settle`, Ack),
  reopen: async (id: string) => request("DELETE", `/api/session/${id}/settle`, Ack),
  pin: async (id: string) => request("POST", `/api/session/${id}/pin`, Ack),
  unpin: async (id: string) => request("DELETE", `/api/session/${id}/pin`, Ack),
  regenerateTitle: (
    id: string,
    preset: TitleModelPreset = readTitleModelPreset(),
  ) =>
    request(
      "POST",
      `/api/session/${id}/title/regenerate`,
      Object({ title: TString() }),
      { preset },
    ),
  addProject: (cwd: string) => request("POST", "/api/projects", Ack, { cwd }),
  removeProject: (cwd: string) => request("DELETE", "/api/projects", Ack, { cwd }),
  setUiPlugin: (id: string, enabled: boolean) =>
    request("POST", `/api/ui-plugins/${encodeURIComponent(id)}`, types.UiPlugin, {
      enabled,
    }),
  attachAgent: (id: string) =>
    request("POST", `/api/session/${id}/agent-attach`, types.AgentSnapshot),
  newSession: (cwd: string) =>
    request("POST", "/api/session/new", Object({ id: TString() }), { cwd }),
  send: (id: string, text: string, mode: "send" | "steer" | "queue") =>
    request(
      "POST",
      `/api/session/${id}/message`,
      Object({ accepted: Boolean(), wasStreaming: Boolean() }),
      { text, mode },
    ),
  sendImages: (
    id: string,
    text: string,
    mode: "send" | "steer" | "queue",
    images: Array<{ data: string; mimeType: string }>,
  ) =>
    request(
      "POST",
      `/api/session/${id}/message`,
      Object({ accepted: Boolean(), wasStreaming: Boolean() }),
      images.length > 0 ? { text, mode, images } : { text, mode },
    ),
  abort: async (id: string) => request("POST", `/api/session/${id}/abort`, Ack),
  setModel: async (id: string, provider: string, modelId: string) =>
    request("POST", `/api/session/${id}/set-model`, Ack, { provider, modelId }),
  setThinking: async (id: string, level: string) =>
    request("POST", `/api/session/${id}/set-thinking`, Ack, { level }),
  respond: (
    id: string,
    requestId: string,
    answer: { cancelled?: boolean; confirmed?: boolean; value?: string },
  ) =>
    request(
      "POST",
      `/api/session/${id}/respond`,
      Object({ responded: Boolean() }),
      { requestId, ...answer },
    ),
  asks: (id: string) =>
    request("GET", `/api/session/${id}/asks`, Object({ asks: Array(types.PendingAsk) })),
};

/** Complete boundary a standalone or hosted session transport must satisfy. */
export type SessionTransport = typeof api;

/** Current local implementation; hosted adapters can satisfy the same contract. */
export const localSessionTransport: SessionTransport = api;
