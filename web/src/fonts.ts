/**
 * Typography preferences: UI and code font families chosen in Settings,
 * persisted locally and applied as CSS variable overrides on :root.
 *
 * Desktop Chromium exposes the full installed list via `queryLocalFonts`
 * (permission-gated); everywhere else — notably iOS Safari — users pick
 * from a curated stack of fonts commonly present on the device.
 */

const UI_FONT_KEY = "pecan:font-ui";
const CODE_FONT_KEY = "pecan:font-code";

export const SYSTEM_UI = "system-ui";
export const SYSTEM_CODE = "system-mono";

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

function familyStack(font: string, fallback: string): string {
  return `"${font.replaceAll('"', "")}", ${fallback}`;
}

/** Returns locally installed font families, or [] when unsupported/denied. */
export async function loadInstalledFonts(): Promise<string[]> {
  const root = globalThis as { queryLocalFonts?: () => Promise<Array<{ family: string }>> };
  if (typeof root.queryLocalFonts !== "function") return [];
  try {
    const fonts = await root.queryLocalFonts();
    return [...new Set(fonts.map((font) => font.family))].sort((a, b) => a.localeCompare(b));
  } catch {
    return [];
  }
}

/** Applies stored font choices as :root variable overrides. */
export function applyFonts(): void {
  try {
    const ui = localStorage.getItem(UI_FONT_KEY);
    const code = localStorage.getItem(CODE_FONT_KEY);
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
  } catch {
    /* private mode */
  }
}

export function readUiFont(): string {
  try {
    return localStorage.getItem(UI_FONT_KEY) ?? SYSTEM_UI;
  } catch {
    return SYSTEM_UI;
  }
}

export function readCodeFont(): string {
  try {
    return localStorage.getItem(CODE_FONT_KEY) ?? SYSTEM_CODE;
  } catch {
    return SYSTEM_CODE;
  }
}

export function saveUiFont(font: string): void {
  try {
    localStorage.setItem(UI_FONT_KEY, font);
  } catch {
    /* private mode */
  }
  applyFonts();
}

export function saveCodeFont(font: string): void {
  try {
    localStorage.setItem(CODE_FONT_KEY, font);
  } catch {
    /* private mode */
  }
  applyFonts();
}

applyFonts();
