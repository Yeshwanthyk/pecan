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
