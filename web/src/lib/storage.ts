/**
 * Thin wrappers around `localStorage` / `sessionStorage` that survive storage
 * being unavailable (private browsing, disabled cookies, locked-down embeds),
 * where browsers throw `SecurityError` even on *accessing* the global. Reads
 * fall back to `null` so callers apply their own default; writes are
 * best-effort since the caller's in-memory value still works for this page.
 */

type AreaName = "localStorage" | "sessionStorage";

/** Areas that threw `SecurityError`; not retried for the rest of the page. */
const blocked = new Set<AreaName>();

function withArea<T>(name: AreaName, fallback: T, use: (area: Storage) => T): T {
  if (blocked.has(name)) return fallback;
  try {
    return use(window[name]);
  } catch (error) {
    // SecurityError means storage is off for this origin: stop retrying.
    // QuotaExceededError and friends leave it enabled for later calls.
    if (error instanceof DOMException && error.name === "SecurityError") blocked.add(name);
    return fallback;
  }
}

export const readLocalStorage = (key: string): string | null =>
  withArea("localStorage", null, (area) => area.getItem(key));

export const writeLocalStorage = (key: string, value: string): void =>
  withArea("localStorage", undefined, (area) => area.setItem(key, value));

export const removeLocalStorage = (key: string): void =>
  withArea("localStorage", undefined, (area) => area.removeItem(key));

export const readSessionStorage = (key: string): string | null =>
  withArea("sessionStorage", null, (area) => area.getItem(key));

export const writeSessionStorage = (key: string, value: string): void =>
  withArea("sessionStorage", undefined, (area) => area.setItem(key, value));

/** Parses JSON text, or `undefined` when it is malformed (a corrupted cache
 * entry, a cut-off frame). The single place JSON syntax errors are absorbed;
 * callers validate the shape of whatever comes back. */
export function parseJson(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch (error) {
    if (error instanceof SyntaxError) return undefined;
    throw error;
  }
}

/** A stored JSON array of strings; anything else reads as empty. */
export function readStoredStrings(key: string): string[] {
  const raw = readLocalStorage(key);
  const parsed = raw ? parseJson(raw) : undefined;
  return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
}
