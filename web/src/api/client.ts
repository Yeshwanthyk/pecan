/**
 * The only module allowed to touch `fetch`. Every response crosses a TypeBox
 * boundary before it reaches the app, so malformed server data fails loudly
 * here instead of leaking `undefined` into components.
 */
import {
  Array,
  Boolean,
  Null,
  Number as TNumber,
  Object,
  String as TString,
  Union,
  type Static,
  type TSchema,
} from "@sinclair/typebox";
import { Value } from "@sinclair/typebox/value";

import {
  readLocalStorage,
  readSessionStorage,
  writeLocalStorage,
  writeSessionStorage,
} from "~/lib/storage";

import * as types from "./types";

export type TitleModelPreset = "luna-low" | "luna-medium" | "sol-low";

const TITLE_MODEL_PRESET_KEY = "pecan:title-model-preset";
const SHIP_TOKEN_KEY = "pecan:ship-token";
const TITLE_MODEL_PRESETS = new Set<TitleModelPreset>([
  "luna-low",
  "luna-medium",
  "sol-low",
]);

export function readTitleModelPreset(): TitleModelPreset {
  const stored = readLocalStorage(TITLE_MODEL_PRESET_KEY);
  return stored && TITLE_MODEL_PRESETS.has(stored as TitleModelPreset)
    ? (stored as TitleModelPreset)
    : "luna-low";
}

export function saveTitleModelPreset(preset: TitleModelPreset) {
  writeLocalStorage(TITLE_MODEL_PRESET_KEY, preset);
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    /** Stable server reason, e.g. `unpaired`. */
    readonly code: string | null = null,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Fired when the server says this browser is not (or no longer) paired. */
export const UNPAIRED_EVENT = "pecan:unpaired";

/** Schema for endpoints whose bodies we ignore. */
const Ack = Object({});

async function request<T extends TSchema>(
  method: "GET" | "POST" | "PUT" | "DELETE",
  path: string,
  schema: T,
  body?: unknown,
  capability?: "ship",
  idempotencyKey?: string,
): Promise<Static<T>> {
  const shipToken = capability === "ship" ? readShipToken() : null;
  const response = await fetch(path, {
    method,
    headers: {
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(shipToken ? { "x-pecan-ship-token": shipToken } : {}),
      ...(idempotencyKey ? { "idempotency-key": idempotencyKey } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) as string }),
  });
  if (!response.ok) {
    const problem: unknown = await response.json().catch(() => null);
    const field = (key: string) =>
      typeof problem === "object" && problem !== null && key in problem
        ? String((problem as Record<string, unknown>)[key])
        : null;
    const code = field("code");
    if (response.status === 401 && code === "unpaired") {
      window.dispatchEvent(new CustomEvent(UNPAIRED_EVENT));
    }
    throw new ApiError(
      field("error") ?? response.statusText,
      response.status,
      code,
    );
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

/** Random request key. `crypto.randomUUID` needs a secure context, which a
 * phone on plain LAN http does not have; `getRandomValues` works everywhere. */
function newIdempotencyKey(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return globalThis.Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

const RETRY_DELAYS_MS = [400, 1200, 3000];

/**
 * A POST whose side effect must happen exactly once even when a flaky mobile
 * link drops the response: every attempt carries the same Idempotency-Key, so
 * the server replays the first result instead of running it again.
 * Retries only when the request may not have arrived (network failure) or the
 * first attempt is still running server-side.
 */
async function postOnce<T extends TSchema>(
  path: string,
  schema: T,
  body: unknown,
  key = newIdempotencyKey(),
  attempt = 0,
): Promise<Static<T>> {
  try {
    return await request("POST", path, schema, body, undefined, key);
  } catch (error) {
    const inFlight =
      attempt > 0 &&
      error instanceof ApiError &&
      error.status === 409 &&
      /Idempotency-Key/.test(error.message);
    const retryable = error instanceof TypeError || inFlight;
    const delay = RETRY_DELAYS_MS[attempt];
    if (!retryable || delay === undefined) throw error;
    await new Promise((resolve) => setTimeout(resolve, delay));
    return postOnce(path, schema, body, key, attempt + 1);
  }
}

/** Captures the per-launch Ship capability, then removes it from the visible URL. */
export function captureShipToken() {
  const url = new URL(location.href);
  const token = url.searchParams.get("ship-token");
  if (!token) return;
  writeSessionStorage(SHIP_TOKEN_KEY, token);
  url.searchParams.delete("ship-token");
  history.replaceState(
    history.state,
    "",
    `${url.pathname}${url.search}${url.hash}`,
  );
}

/** Takes a one-time `?pair=` code off the URL (so reloads and shared links
 * never carry it) and returns it for redemption. */
export function takePairCode(): string | null {
  const url = new URL(location.href);
  const code = url.searchParams.get("pair");
  if (!code) return null;
  url.searchParams.delete("pair");
  history.replaceState(
    history.state,
    "",
    `${url.pathname}${url.search}${url.hash}`,
  );
  return code;
}

function readShipToken(): string | null {
  return readSessionStorage(SHIP_TOKEN_KEY);
}

export const api = {
  bootstrap: () => request("GET", "/api/bootstrap", types.Bootstrap),
  /** VAPID `applicationServerKey` and whether this device is subscribed. */
  pushKey: () =>
    request(
      "GET",
      "/api/push/key",
      Object({ publicKey: TString(), subscribed: Boolean() }),
    ),
  pushSubscribe: (endpoint: string) =>
    request(
      "PUT",
      "/api/push/subscription",
      Object({ subscribed: Boolean() }),
      { endpoint },
    ),
  pushUnsubscribe: () =>
    request("DELETE", "/api/push/subscription", Object({ removed: Boolean() })),
  /** Pushes a test notification to this device, even while it is open. */
  pushTest: () =>
    request(
      "POST",
      "/api/push/test",
      Object({ sent: TNumber(), removed: TNumber(), failed: TNumber() }),
    ),
  /** Redeems a one-time code from `pecan pair`; the server sets the device cookie. */
  pair: (code: string) =>
    request("POST", "/api/pair", types.Paired, { code: code.trim() }),
  health: (refresh = false) =>
    request(
      "GET",
      refresh ? "/api/health?refresh=true" : "/api/health",
      types.Health,
    ),
  sessions: (query: {
    project?: string;
    limit?: number;
    kind?: "normal" | "subagent";
  }) => {
    const params = new URLSearchParams();
    if (query.project) params.set("project", query.project);
    if (query.kind) params.set("kind", query.kind);
    params.set("limit", String(query.limit ?? 400));
    return request("GET", `/api/sessions?${params}`, types.SessionPage);
  },
  thread: (id: string) =>
    request("GET", `/api/session/${id}`, types.ThreadView),
  gitBranch: (id: string) =>
    request(
      "GET",
      `/api/session/${id}/git`,
      Object({ branch: Union([Null(), TString()]) }),
    ),
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
  settle: async (id: string) =>
    request("POST", `/api/session/${id}/settle`, Ack),
  reopen: async (id: string) =>
    request("DELETE", `/api/session/${id}/settle`, Ack),
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
  /** Folder picker: known Pi folders plus child directories of `path`. */
  folders: (path: string) =>
    request(
      "GET",
      `/api/folders?${new URLSearchParams({ path })}`,
      types.FolderPick,
    ),
  addProject: (cwd: string) => request("POST", "/api/projects", Ack, { cwd }),
  removeProject: (cwd: string) =>
    request("DELETE", "/api/projects", Ack, { cwd }),
  attachAgent: (id: string) =>
    request("POST", `/api/session/${id}/agent-attach`, types.AgentSnapshot),
  newSession: (cwd: string) =>
    postOnce("/api/session/new", Object({ id: TString() }), { cwd }),
  send: (id: string, text: string, mode: "send" | "steer" | "queue") =>
    postOnce(
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
    postOnce(
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
    postOnce(`/api/session/${id}/respond`, Object({ responded: Boolean() }), {
      requestId,
      ...answer,
    }),
  asks: (id: string) =>
    request(
      "GET",
      `/api/session/${id}/asks`,
      Object({ asks: Array(types.PendingAsk) }),
    ),
};
