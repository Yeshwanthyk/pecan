/**
 * Global "sent prompts" ring, most recent first, for ArrowUp recall in an
 * empty composer. Not per-thread: a prompt sent anywhere is worth recalling
 * anywhere.
 */
import { writeLocalStorage, readStoredStrings } from "~/lib/storage";

const HISTORY_KEY = "pecan:prompt-history";
const MAX_ENTRIES = 50;

export function readPromptHistory(): string[] {
  return readStoredStrings(HISTORY_KEY);
}

/** Records a sent prompt at the front of the ring, deduping and capping at 50. */
export function recordPrompt(text: string): void {
  const trimmed = text.trim();
  if (!trimmed) return;
  const next = [trimmed, ...readPromptHistory().filter((entry) => entry !== trimmed)].slice(0, MAX_ENTRIES);
  writeLocalStorage(HISTORY_KEY, JSON.stringify(next));
}
