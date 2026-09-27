/**
 * Per-thread composer draft persistence (text only). Backed by localStorage
 * so a draft survives a reload or a trip away from the thread; a bounded
 * index keeps the total key count in check across many threads.
 */
import { readLocalStorage, removeLocalStorage, writeLocalStorage, readStoredStrings } from "~/lib/storage";

const DRAFT_PREFIX = "pecan:draft:";
const INDEX_KEY = "pecan:draft-index";
/// Most-recently-touched threads keep their draft; older ones are pruned.
const MAX_DRAFTS = 50;

function draftKey(sessionId: string): string {
  return `${DRAFT_PREFIX}${sessionId}`;
}

function readIndex(): string[] {
  return readStoredStrings(INDEX_KEY);
}

export function readDraft(sessionId: string): string {
  return readLocalStorage(draftKey(sessionId)) ?? "";
}

/** Saves `text` as the draft for `sessionId`, or clears it when empty. Prunes to the 50 most recently touched threads. */
export function writeDraft(sessionId: string, text: string): void {
  if (!text) {
    clearDraft(sessionId);
    return;
  }
  writeLocalStorage(draftKey(sessionId), text);
  const index = [sessionId, ...readIndex().filter((id) => id !== sessionId)];
  const kept = index.slice(0, MAX_DRAFTS);
  for (const stale of index.slice(MAX_DRAFTS)) removeLocalStorage(draftKey(stale));
  writeLocalStorage(INDEX_KEY, JSON.stringify(kept));
}

export function clearDraft(sessionId: string): void {
  removeLocalStorage(draftKey(sessionId));
  const kept = readIndex().filter((id) => id !== sessionId);
  writeLocalStorage(INDEX_KEY, JSON.stringify(kept));
}
