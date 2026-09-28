/** "anthropic/claude-sonnet-4" → "claude-sonnet-4". */
export function shortModel(model: string) {
  return model.split("/").at(-1) ?? model;
}

/** Last non-empty path segment of a working directory, e.g. "/Users/x/pecan" → "pecan". */
export function projectName(cwd: string) {
  return cwd.split("/").findLast((segment) => segment.length > 0) ?? cwd;
}

/**
 * Strips the `<owner>: ` prefix pi-subagents puts on child session names
 * (`subagents: scout`, `workflow:wf-1: Build`), leaving the spawn title.
 * Mirrors `normalize_agent_name` on the server.
 */
export function agentTitle(name: string): string {
  const match = /^(\S+?): ([\s\S]*)$/.exec(name);
  return (match?.[2] ?? name).trim();
}

/** Compact activity time: "now", "12m", "14:05" today, "3d", then "9/14". */
export function timeLabel(iso: string) {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const seconds = Math.max(0, (Date.now() - date.getTime()) / 1000);
  if (seconds < 3600) {
    if (seconds < 90) return "now";
    return `${Math.round(seconds / 60)}m`;
  }
  if (date.toDateString() === new Date().toDateString()) {
    return date.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
  }
  if (seconds < 86_400 * 7) return `${Math.round(seconds / 86_400)}d`;
  return `${date.getMonth() + 1}/${date.getDate()}`;
}
