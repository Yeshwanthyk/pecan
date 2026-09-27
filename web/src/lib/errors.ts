/** Surfaces an error message via the global "pecan:error" toast/event channel. */
export function reportError(message: string) {
  window.dispatchEvent(new CustomEvent("pecan:error", { detail: message }));
}
