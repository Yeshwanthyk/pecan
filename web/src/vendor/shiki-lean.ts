/**
 * Minimal Shiki surface for Pierre Diffs.
 *
 * Pecan intentionally renders workspace diffs as plain text. Importing Shiki's
 * default entrypoint would make Vite emit every bundled grammar and theme,
 * adding roughly 12 MB of optional assets to the embedded frontend.
 */
import {
  codeToHtml,
  createCssVariablesTheme,
  createHighlighterCore,
  getTokenStyleObject,
  stringifyTokenStyle,
} from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

export const bundledLanguages = {};
export const createHighlighter = createHighlighterCore;

export function createOnigurumaEngine(): never {
  throw new Error("Pecan's lean diff renderer does not include Shiki's WASM engine");
}

export {
  codeToHtml,
  createCssVariablesTheme,
  createJavaScriptRegexEngine,
  getTokenStyleObject,
  stringifyTokenStyle,
};
