/**
 * Compact thread navigation for subagent runs.
 *
 * Pi's `session_info.parentId` is the previous JSONL entry id, not a parent
 * session id. The strip therefore uses the parent carried in the hash route
 * for exact navigation. The current scan also has no authoritative process
 * lifecycle, so this surface deliberately says "Recent agents" and shows
 * unsettled same-project candidates opened after the parent rather than
 * claiming that they are running.
 */
import { ArrowLeftIcon } from "lucide-react";
import { useMemo } from "react";

import type { SessionRow, SessionSummary } from "~/api/types";
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

  const candidates = useMemo(() => {
    const parentOpenedAt = parent ? Date.parse(parent.openedAt) : Number.NaN;
    return sessions
      .filter((row) => {
        if (row.kind !== "subagent" || row.cwd !== (parent?.cwd ?? current.cwd)) {
          return false;
        }
        if (row.settled && row.id !== current.id) return false;
        const childOpenedAt = Date.parse(row.openedAt);
        return (
          !Number.isFinite(parentOpenedAt) ||
          !Number.isFinite(childOpenedAt) ||
          childOpenedAt >= parentOpenedAt
        );
      });
  }, [current.cwd, current.id, parent, sessions]);

  const runningIds = useMemo(() => {
    const ids = new Set<string>();
    for (const live of activity?.children ?? []) {
      const match = candidates
        .filter((row) => agentName(row) === normalizeAgentName(live.title))
        .sort(
          (a, b) =>
            Math.abs(Date.parse(a.openedAt) - live.startedAt) -
            Math.abs(Date.parse(b.openedAt) - live.startedAt),
        )[0];
      if (match) ids.add(match.id);
    }
    return ids;
  }, [activity, candidates]);

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
          Main thread
        </a>
        <span className="shrink-0 px-1 text-xs text-muted-foreground">
          Recent agents
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
        "flex min-h-11 max-w-[17rem] shrink-0 items-center gap-2 rounded-md border px-2.5 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring md:min-h-8",
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
  return normalizeAgentName(row.agentName ?? row.preview ?? row.id.slice(0, 8));
}

function normalizeAgentName(value: string) {
  return value.replace(/^subagents:\s*/, "").trim();
}
