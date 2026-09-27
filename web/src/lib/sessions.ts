/** Session lifecycle actions shared by the sidebar, header, and empty state. */
import { api } from "~/api/client";
import { reportError } from "~/lib/errors";
import { useApp } from "~/store";

/**
 * Starts a Pi session in `cwd`, then opens it. `startingCwd` in the store
 * drives progress UI; failures surface as a toast. Returns the new id.
 */
export async function startSession(cwd: string): Promise<string | null> {
  const store = useApp.getState();
  if (store.startingCwd !== null) return null;
  store.setStartingCwd(cwd);
  try {
    const { id } = await api.newSession(cwd);
    location.hash = `#/s/${encodeURIComponent(id)}`;
    window.dispatchEvent(new CustomEvent("pecan:navigated"));
    window.dispatchEvent(new CustomEvent("pecan:refresh"));
    return id;
  } catch (error) {
    reportError(`Could not start a session: ${error instanceof Error ? error.message : String(error)}`);
    return null;
  } finally {
    useApp.getState().setStartingCwd(null);
  }
}
