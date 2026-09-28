/**
 * Compact thread navigation for subagent runs.
 *
 * Pi's `session_info.parentId` is the previous JSONL entry id, not a parent
 * session id. Embedded servers resolve that relationship from the parent's
 * spawn name/prompt evidence; the global server uses a cheap recent-agent
 * fallback so it never parses historical transcripts just to render this UI.
 */
import { ArrowLeftIcon } from "lucide-react";
import { useMemo } from "react";

import type { SessionRow, SessionSummary } from "~/api/types";
import { agentTitle } from "~/lib/format";
import { cn } from "~/lib/utils";
import { useApp } from "~/store";

const MAX_VISIBLE_AGENTS = 8;

export function SubagentStrip({
  parentId,
  parent,
  current,
}: {
  parentId: string;
  parent?: SessionSummary;
  current: SessionSummary;
}) {
  const sessions = useApp((state) => state.sessions);
  const activity = useApp((state) => state.subagentActivity[parentId]);
  const isInsideChild = current.id !== parentId;
  const parentOpenedAt = parent ? Date.parse(parent.openedAt) : Number.NaN;

  // One session row per live child, chosen by title + closest start time.
  // `activity` is already scoped to this exact parentId (keyed by
  // `subagentActivity[parentId]`), so title matching here doesn't need a
  // cwd guard — a direct `subagent_spawn` can run in a different cwd than
  // its parent (the `pi` backend passes the caller's own cwd through
  // unchanged), and requiring cwd equality hid cross-cwd children entirely.
  // Claiming each matched row also keeps a reused title (a later spawn
  // round can reuse a name like "fix-idle-coldstart") from matching more
  // than one historical row at once, which used to render duplicate tabs
  // with the same label instead of one tab per live child.
  const runningRows = useMemo(() => {
    const claimed = new Set<string>();
    const matches: SessionRow[] = [];
    for (const live of activity?.children ?? []) {
      const title = agentTitle(live.title);
      const match = sessions
        .filter(
          (row) =>
            row.kind === "subagent" &&
            !claimed.has(row.id) &&
            agentName(row) === title &&
            (!Number.isFinite(parentOpenedAt) ||
              !Number.isFinite(Date.parse(row.openedAt)) ||
              Date.parse(row.openedAt) >= parentOpenedAt),
        )
        .sort(
          (a, b) =>
            Math.abs(Date.parse(a.openedAt) - live.startedAt) -
            Math.abs(Date.parse(b.openedAt) - live.startedAt),
        )[0];
      if (match) {
        claimed.add(match.id);
        matches.push(match);
      }
    }
    return matches;
  }, [activity, parentOpenedAt, sessions]);

  const runningIds = useMemo(
    () => new Set(runningRows.map((row) => row.id)),
    [runningRows],
  );

  const candidates = useMemo(() => {
    return sessions.filter((row) => {
      if (row.kind !== "subagent") return false;
      const exactMatch = row.parentSessionId === parentId;
      const fallbackMatch = row.id === current.id || runningIds.has(row.id);
      if (!exactMatch && !fallbackMatch) {
        return false;
      }
      return !row.settled || row.id === current.id;
    });
  }, [current.id, parentId, runningIds, sessions]);

  const children = useMemo(
    () =>
      [...candidates]
        .sort((a, b) => {
          if (a.id === current.id) return -1;
          if (b.id === current.id) return 1;
          const runningOrder = Number(runningIds.has(b.id)) - Number(runningIds.has(a.id));
          if (runningOrder !== 0) return runningOrder;
          return Date.parse(b.lastActivity) - Date.parse(a.lastActivity);
        })
        .slice(0, MAX_VISIBLE_AGENTS),
    [candidates, current.id, runningIds],
  );

  if (children.length === 0 && !isInsideChild) return null;

  return (
    <nav
      aria-label="Thread navigation"
      className="shrink-0 border-t bg-background px-3 md:px-6"
    >
      <div className="mx-auto flex max-w-[46rem] items-center gap-1.5 overflow-x-auto py-1.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        <a
          aria-current={isInsideChild ? undefined : "page"}
          className={cn(
            "flex min-h-11 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring md:min-h-8",
            isInsideChild
              ? "font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
              : "bg-accent font-semibold text-foreground",
          )}
          href={`#/s/${parentId}`}
          title={parent?.preview ?? "Open the main thread"}
        >
          {isInsideChild ? <ArrowLeftIcon aria-hidden className="size-3.5" /> : null}
          <span className="sm:hidden">Main</span>
          <span className="hidden sm:inline">Main thread</span>
        </a>
        <span className="hidden shrink-0 px-1 text-xs text-muted-foreground sm:inline">
          Agents
        </span>
        {children.map((child) => (
          <SubagentLink
            key={child.id}
            active={child.id === current.id}
            running={runningIds.has(child.id)}
            parentId={parentId}
            row={child}
          />
        ))}
      </div>
    </nav>
  );
}

function SubagentLink({
  row,
  active,
  parentId,
  running,
}: {
  row: SessionRow;
  active: boolean;
  parentId: string;
  running: boolean;
}) {
  const name = agentName(row);

  return (
    <a
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex min-h-11 max-w-[12rem] shrink-0 items-center gap-2 rounded-md border px-2.5 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring sm:max-w-[17rem] md:min-h-8",
        active
          ? "border-input bg-accent font-medium text-foreground"
          : "border-border bg-card text-muted-foreground hover:border-input hover:text-foreground",
      )}
      href={`#/s/${row.id}?parent=${encodeURIComponent(parentId)}`}
      title={`${name} — ${running ? "running" : "recent"}; open thread`}
    >
      <span
        aria-hidden
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          running ? "bg-success" : active ? "bg-foreground" : "bg-muted-foreground/70",
        )}
      />
      <span className="truncate">{name}</span>
      {running ? <span className="sr-only">Running</span> : null}
    </a>
  );
}

function agentName(row: SessionRow) {
  return agentTitle(row.agentName ?? row.preview ?? row.id.slice(0, 8));
}
