/**
 * Typography preferences: UI and code font families chosen in Settings,
 * persisted locally and applied as CSS variable overrides on :root.
 *
 * Desktop Chromium exposes the full installed list via `queryLocalFonts`
 * (permission-gated); everywhere else — notably iOS Safari — users pick
 * from a curated stack of fonts commonly present on the device.
 */
import { readLocalStorage, readStoredStrings, writeLocalStorage } from "~/lib/storage";

const UI_FONT_KEY = "pecan:font-ui";
const CODE_FONT_KEY = "pecan:font-code";
const UI_CUSTOM_FONTS_KEY = "pecan:font-ui-custom";
const CODE_CUSTOM_FONTS_KEY = "pecan:font-code-custom";
const UI_SIZE_KEY = "pecan:text-size-ui";
const CODE_SIZE_KEY = "pecan:text-size-code";

export const SYSTEM_UI = "system-ui";
export const SYSTEM_CODE = "system-mono";

export type TextSize = "small" | "default" | "large";

const TEXT_SIZE_SCALES: Record<TextSize, number> = {
  small: 0.9,
  default: 1,
  large: 1.12,
};

/** Common device-installed proportional fonts (fallback when local scan is unavailable). */
export const CURATED_UI_FONTS = [
  "SF Pro Text",
  "Helvetica Neue",
  "Georgia",
  "Iowan Old Style",
  "Times New Roman",
  "Arial",
  "Verdana",
  "Segoe UI",
  "Roboto",
  "Inter",
];

/** Common device-installed monospace fonts. */
export const CURATED_CODE_FONTS = [
  "SF Mono",
  "Menlo",
  "Monaco",
  "Courier New",
  "Consolas",
  "Cascadia Code",
  "JetBrains Mono",
  "Fira Code",
  "IBM Plex Mono",
];

/** Default stacks from index.css; chosen fonts are prepended to these. */
const UI_FALLBACK =
  '-apple-system, BlinkMacSystemFont, "SF Pro Text", Inter, "Segoe UI", Roboto, sans-serif';
const CODE_FALLBACK = 'ui-monospace, "SF Mono", Menlo, Consolas, monospace';

function normalizeFont(font: string): string | null {
  const value = font.trim();
  return value.length > 0 && value.length <= 80 && /^[\p{L}\p{N} .,'_-]+$/u.test(value)
    ? value
    : null;
}

function familyStack(font: string, fallback: string): string {
  return `"${normalizeFont(font) ?? "system-ui"}", ${fallback}`;
}

/** Returns locally installed font families, or [] when unsupported/denied. */
export async function loadInstalledFonts(): Promise<string[]> {
  const root = globalThis as { queryLocalFonts?: () => Promise<Array<{ family: string }>> };
  if (typeof root.queryLocalFonts !== "function") return [];
  let fonts: string[] = [];
  try {
    fonts = [...new Set((await root.queryLocalFonts()).map((font) => font.family))].sort((a, b) =>
      a.localeCompare(b),
    );
  } catch {
    // Permission denied or an unsupported browser; the curated fallback list is used instead.
  }
  return fonts;
}

/** Applies stored font choices and size preferences as :root variable overrides. */
function applyFonts(): void {
  const ui = readLocalStorage(UI_FONT_KEY);
  const code = readLocalStorage(CODE_FONT_KEY);
  if (ui && ui !== SYSTEM_UI) {
    document.documentElement.style.setProperty("--font-sans", familyStack(ui, UI_FALLBACK));
  } else {
    document.documentElement.style.removeProperty("--font-sans");
  }
  if (code && code !== SYSTEM_CODE) {
    document.documentElement.style.setProperty("--font-mono", familyStack(code, CODE_FALLBACK));
  } else {
    document.documentElement.style.removeProperty("--font-mono");
  }
  document.documentElement.style.setProperty(
    "--ui-font-scale",
    String(TEXT_SIZE_SCALES[readUiTextSize()]),
  );
  document.documentElement.style.setProperty(
    "--code-font-scale",
    String(TEXT_SIZE_SCALES[readCodeTextSize()]),
  );
}

export function readUiFont(): string {
  return readLocalStorage(UI_FONT_KEY) ?? SYSTEM_UI;
}

export function readCodeFont(): string {
  return readLocalStorage(CODE_FONT_KEY) ?? SYSTEM_CODE;
}

export function saveUiFont(font: string): void {
  const normalized = normalizeFont(font);
  if (!normalized) return;
  writeLocalStorage(UI_FONT_KEY, normalized);
  applyFonts();
}

export function saveCodeFont(font: string): void {
  const normalized = normalizeFont(font);
  if (!normalized) return;
  writeLocalStorage(CODE_FONT_KEY, normalized);
  applyFonts();
}

function readCustomFonts(key: string): string[] {
  return [...new Set(readStoredStrings(key).flatMap((font) => {
    const normalized = normalizeFont(font);
    return normalized ? [normalized] : [];
  }))];
}

function addCustomFont(key: string, font: string): string[] {
  const normalized = normalizeFont(font);
  if (!normalized) return readCustomFonts(key);
  const fonts = [...new Set([...readCustomFonts(key), normalized])];
  writeLocalStorage(key, JSON.stringify(fonts));
  return fonts;
}

export const readCustomUiFonts = () => readCustomFonts(UI_CUSTOM_FONTS_KEY);
export const readCustomCodeFonts = () => readCustomFonts(CODE_CUSTOM_FONTS_KEY);
export const addCustomUiFont = (font: string) => addCustomFont(UI_CUSTOM_FONTS_KEY, font);
export const addCustomCodeFont = (font: string) => addCustomFont(CODE_CUSTOM_FONTS_KEY, font);

function readTextSize(key: string): TextSize {
  const value = readLocalStorage(key);
  return value === "small" || value === "large" ? value : "default";
}

export const readUiTextSize = () => readTextSize(UI_SIZE_KEY);
export const readCodeTextSize = () => readTextSize(CODE_SIZE_KEY);

function saveTextSize(key: string, size: TextSize): void {
  writeLocalStorage(key, size);
  applyFonts();
}

export const saveUiTextSize = (size: TextSize) => saveTextSize(UI_SIZE_KEY, size);
export const saveCodeTextSize = (size: TextSize) => saveTextSize(CODE_SIZE_KEY, size);

applyFonts();
