import { useEffect, useState } from "react";
import {
  ChevronDownIcon,
  CircleCheckIcon,
  CircleDashedIcon,
  CircleDotIcon,
  CircleSlashIcon,
  CircleXIcon,
  ListTodoIcon,
  LoaderCircleIcon,
  WorkflowIcon,
} from "lucide-react";
import type { TaskGroup, TaskItem, WorkflowRun, WorkflowTask } from "~/api/types";
import { Badge } from "~/components/ui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "~/components/ui/collapsible";
import { cn } from "~/lib/utils";
import type { ExtensionWidget, SubagentActivity, SubagentHandoff } from "~/store";

const SUBAGENT_WIDGET = "pi-subagents/activity/v1";

export function ExtensionWidgetRenderer({
  widget,
  activity,
}: {
  widget: ExtensionWidget;
  activity: SubagentActivity | undefined;
}) {
  if (widget.key === SUBAGENT_WIDGET && activity) {
    return <ActivityHandoff activity={activity} />;
  }
  return <GenericWidget widget={widget} />;
}

/** How long a settled child's handoff stays visible after it leaves the list. */
const HANDOFF_VISIBLE_MS = 8_000;

/** Running children live in the agent tabs above the composer; this only
 * surfaces a just-finished child's handoff for a few seconds. */
function ActivityHandoff({ activity }: { activity: SubagentActivity }) {
  const handoff = useRecentHandoff(activity.terminal);
  return handoff ? <HandoffRow handoff={handoff} /> : null;
}

function HandoffRow({ handoff }: { handoff: SubagentHandoff }) {
  const failed = handoff.status === "error";
  const detail = failed ? (handoff.failure ?? handoff.output) : handoff.output;
  return (
    <div
      aria-live="polite"
      className="flex min-w-0 items-center gap-2 px-1 text-[12px] animate-toast-in"
    >
      {failed ? (
        <CircleXIcon className="size-3.5 shrink-0 text-destructive" />
      ) : (
        <CircleCheckIcon className="size-3.5 shrink-0 text-success" />
      )}
      <span className="shrink-0 font-medium">{handoff.title}</span>
      <span className={cn("min-w-0 truncate text-[11px]", failed ? "text-destructive" : "text-muted-foreground")}>
        {detail ? lastLine(detail) : failed ? "failed" : "done"}
      </span>
    </div>
  );
}

/** Returns the handoff while it is fresh, re-rendering once it expires. */
function useRecentHandoff(terminal: SubagentHandoff | undefined) {
  const [now, setNow] = useState(() => Date.now());
  const remaining = terminal ? terminal.settledAt + HANDOFF_VISIBLE_MS - now : 0;
  useEffect(() => {
    if (remaining <= 0) return;
    const timer = window.setTimeout(() => setNow(Date.now()), remaining);
    return () => window.clearTimeout(timer);
  }, [remaining]);
  useEffect(() => setNow(Date.now()), [terminal]);
  return terminal && remaining > 0 ? terminal : undefined;
}

function lastLine(text: string) {
  const lines = text.trim().split("\n");
  return lines[lines.length - 1]?.trim() ?? "";
}

export function TaskListPanel({ groups }: { groups: TaskGroup[] }) {
  const tasks = groups.flatMap((group) => group.tasks);
  if (tasks.length === 0) return null;
  const activeTasks = tasks.filter((task) => task.status === "in_progress");
  const activeTask = activeTasks.find((task) => task.status === "in_progress");
  const activeLabel = activeTask?.activeForm ?? activeTask?.subject;
  return (
    <Collapsible defaultOpen>
      <CollapsibleTrigger className="flex w-full min-w-0 items-center gap-2 rounded-lg px-2 py-1.5 text-[13px] font-medium text-muted-foreground hover:bg-accent">
        <ListTodoIcon className="size-3.5 shrink-0" />
        <span className="shrink-0">Tasks</span>
        <Badge variant="secondary">{tasks.length}</Badge>
        {activeLabel ? (
          <span className="ms-auto flex min-w-0 items-center gap-1 text-[11px] font-normal text-warning">
            <CircleDotIcon className="size-3 shrink-0 animate-pulse" />
            <span className="truncate" title={activeLabel}>{activeLabel}</span>
          </span>
        ) : null}
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="flex flex-col">
          {groups.flatMap((group) =>
            group.tasks.map((task) => <TaskRow key={`${group.sessionId}:${task.id}`} task={task} />),
          )}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Compact header status for the task currently marked `in_progress`. */
export function ActiveTaskStatus({ groups }: { groups: TaskGroup[] }) {
  const activeTasks = groups
    .flatMap((group) => group.tasks)
    .filter((task) => task.status === "in_progress");
  const activeTask = activeTasks.find((task) => task.status === "in_progress");
  if (!activeTask) return null;
  const label = activeTask.activeForm ?? activeTask.subject;
  return (
    <div
      aria-label={`Working on ${label}`}
      className="mt-1 flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-warning"
      title={label}
    >
      <CircleDotIcon className="size-3 shrink-0 animate-pulse" />
      <span className="shrink-0 font-medium">Working on</span>
      <span className="min-w-0 truncate">{label}</span>
      {activeTasks.length > 1 ? (
        <span className="shrink-0 tabular-nums text-muted-foreground">
          +{activeTasks.length - 1}
        </span>
      ) : null}
    </div>
  );
}

function TaskRow({ task }: { task: TaskItem }) {
  const executionStatus = task.execution && typeof task.execution === "object" && "status" in task.execution
    ? String((task.execution as { status?: unknown }).status ?? "unknown")
    : null;
  return (
    <div className="grid grid-cols-[5rem_minmax(0,1fr)] gap-x-3 gap-y-0.5 px-3 py-1.5 text-sm">
      <span
        className={cn(
          "text-xs tabular-nums",
          task.status === "completed" && "text-success",
          task.status === "in_progress" && "text-warning",
          !["completed", "in_progress"].includes(task.status) && "text-muted-foreground",
        )}
      >
        {task.status.replace("_", " ")}
      </span>
      <span className="min-w-0 font-medium">{task.subject}</span>
      {task.description || task.activeForm || task.owner || task.harness || task.blockedBy.length > 0 || task.blocks.length > 0 || executionStatus ? (
        <span className="col-start-2 text-[11px] text-muted-foreground">
          {[
            task.description,
            task.activeForm,
            task.owner ? `Owner ${task.owner}` : null,
            task.harness,
            task.blockedBy.length > 0 ? `Blocked by ${task.blockedBy.join(", ")}` : null,
            task.blocks.length > 0 ? `Blocks ${task.blocks.join(", ")}` : null,
            executionStatus ? `Execution ${executionStatus}` : null,
          ]
            .filter(Boolean)
            .join(" · ")}
        </span>
      ) : null}
    </div>
  );
}

export function WorkflowListPanel({ runs, parentId }: { runs: WorkflowRun[]; parentId: string }) {
  if (runs.length === 0) return null;
  const live = runs.filter((run) => !TERMINAL_WORKFLOW.has(run.status)).length;
  return (
    <Collapsible defaultOpen>
      <CollapsibleTrigger className="group flex min-h-11 w-full items-center gap-2 rounded-lg px-2 text-[13px] font-medium text-muted-foreground hover:bg-accent md:min-h-8">
        <WorkflowIcon className="size-3.5 shrink-0" />
        <span>Workflows</span>
        <Badge variant="secondary">{runs.length}</Badge>
        {live > 0 ? (
          <span className="ms-auto flex items-center gap-1 text-[11px] font-normal text-warning">
            <CircleDotIcon className="size-3 animate-pulse" />
            {live} active
          </span>
        ) : null}
        <ChevronDownIcon className={cn("size-3.5 shrink-0 transition-transform group-data-[panel-open]:rotate-180", live > 0 ? "" : "ms-auto")} />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="flex flex-col gap-2 px-1 py-1.5">
          {runs.map((run) => <WorkflowRunCard key={run.runId} parentId={parentId} run={run} />)}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

const TERMINAL_WORKFLOW = new Set(["completed", "failed", "cancelled"]);

const WORKFLOW_STATUS_LABEL: Record<string, string> = {
  pending_approval: "awaiting approval",
  running: "running",
  paused: "paused",
  completed: "completed",
  failed: "failed",
  cancelled: "cancelled",
};

function WorkflowRunCard({ run, parentId }: { run: WorkflowRun; parentId: string }) {
  const done = run.tasks.filter((task) => task.status === "completed").length;
  const total = run.tasks.length;
  const failed = run.status === "failed" || run.status === "cancelled";
  return (
    <section
      aria-label={`Workflow ${run.name ?? run.runId}`}
      className="rounded-lg border bg-card px-3 py-2.5 text-sm"
      data-status={run.status}
    >
      <div className="flex min-w-0 items-baseline gap-2">
        <span className="min-w-0 truncate font-medium" title={run.runId}>{run.name ?? run.runId}</span>
        <span className={cn("shrink-0 text-xs", workflowStatusClass(run.status))}>
          {WORKFLOW_STATUS_LABEL[run.status] ?? run.status}
        </span>
        {total > 0 ? (
          <span className="ms-auto shrink-0 text-[11px] tabular-nums text-muted-foreground">
            {done}/{total}
          </span>
        ) : null}
      </div>
      {run.description ? <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">{run.description}</p> : null}
      {total > 0 ? (
        <div aria-hidden className="mt-2 flex h-1 gap-0.5 overflow-hidden rounded-full">
          {run.tasks.map((task) => (
            <span className={cn("flex-1 transition-colors duration-500", taskBarClass(task.status))} key={task.id} />
          ))}
        </div>
      ) : null}
      {total > 0 ? (
        <ol className="mt-2 flex flex-col gap-1">
          {run.tasks.map((task) => <WorkflowTaskRow key={task.id} parentId={parentId} task={task} />)}
        </ol>
      ) : null}
      {run.outcome && failed ? <p className="mt-2 text-xs text-destructive">{run.outcome}</p> : null}
    </section>
  );
}

function WorkflowTaskRow({ task, parentId }: { task: WorkflowTask; parentId: string }) {
  const detail = task.error ?? task.result;
  const labelClass = cn("min-w-0 truncate", task.status === "skipped" && "text-muted-foreground line-through");
  return (
    <li className="flex min-w-0 flex-col text-[12px]" data-status={task.status}>
      <div className="flex min-w-0 items-center gap-2">
        <TaskStatusGlyph status={task.status} />
        {task.sessionId ? (
          <a
            className={cn(labelClass, "underline-offset-2 hover:underline focus-visible:underline")}
            href={`#/s/${encodeURIComponent(task.sessionId)}?parent=${encodeURIComponent(parentId)}`}
            title={`Open ${task.label} transcript`}
          >
            {task.label}
          </a>
        ) : (
          <span className={labelClass}>{task.label}</span>
        )}
        {task.attempt > 1 ? (
          <span className="shrink-0 text-[10px] text-muted-foreground">attempt {task.attempt}</span>
        ) : null}
        <span className="ms-auto shrink-0 text-[10px] text-muted-foreground">
          {[task.kind, task.status === "completed" || task.status === "running" ? null : task.status]
            .filter(Boolean)
            .join(" · ")}
        </span>
      </div>
      {detail ? (
        <p
          className={cn(
            "ms-5 line-clamp-2 text-[11px]",
            task.error ? "text-destructive" : "text-muted-foreground",
          )}
          title={detail}
        >
          {detail}
        </p>
      ) : null}
    </li>
  );
}

function TaskStatusGlyph({ status }: { status: string }) {
  if (status === "completed") return <CircleCheckIcon aria-label="completed" className="size-3.5 shrink-0 text-success" />;
  if (status === "failed") return <CircleXIcon aria-label="failed" className="size-3.5 shrink-0 text-destructive" />;
  if (status === "running") return <LoaderCircleIcon aria-label="running" className="size-3.5 shrink-0 animate-spin text-warning" />;
  if (status === "queued") return <CircleDotIcon aria-label="queued" className="size-3.5 shrink-0 animate-pulse text-warning" />;
  if (status === "cancelled" || status === "skipped") return <CircleSlashIcon aria-label={status} className="size-3.5 shrink-0 text-muted-foreground" />;
  return <CircleDashedIcon aria-label="pending" className="size-3.5 shrink-0 text-muted-foreground" />;
}

function taskBarClass(status: string): string {
  if (status === "completed") return "bg-success";
  if (status === "failed") return "bg-destructive";
  if (status === "running" || status === "queued") return "animate-pulse bg-warning";
  return "bg-muted";
}

function workflowStatusClass(status: string): string {
  if (status === "completed") return "text-success";
  if (status === "running" || status === "pending_approval") return "text-warning";
  if (status === "paused") return "text-muted-foreground";
  return "text-destructive";
}

function GenericWidget({ widget }: { widget: ExtensionWidget }) {
  if (widget.lines.length === 0) return null;
  return (
    <section aria-label={`Extension widget ${widget.key}`} className="rounded-lg border bg-card px-3 py-2">
      <div className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {widget.key}
      </div>
      <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-muted-foreground">
        {widget.lines.join("\n")}
      </pre>
    </section>
  );
}
