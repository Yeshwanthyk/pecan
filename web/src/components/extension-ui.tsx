import { CircleDotIcon } from "lucide-react";
import type { TaskGroup, TaskItem, WorkflowRun } from "~/api/types";
import { Badge } from "~/components/ui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "~/components/ui/collapsible";
import { cn } from "~/lib/utils";
import type { ExtensionWidget, SubagentActivity } from "~/store";

const SUBAGENT_WIDGET = "pi-subagents/activity/v1";

export function ExtensionWidgetRenderer({
  widget,
  activity,
}: {
  widget: ExtensionWidget;
  activity: SubagentActivity | undefined;
}) {
  if (widget.key === SUBAGENT_WIDGET && activity) {
    return <ActivityList activity={activity} />;
  }
  return <GenericWidget widget={widget} />;
}

export function ActivityList({ activity }: { activity: SubagentActivity }) {
  if (activity.children.length === 0) return null;
  return (
    <section aria-label="Subagent activity" className="rounded-lg border bg-card px-3 py-2">
      <div className="mb-1.5 flex items-center gap-2 text-[11px] font-medium text-muted-foreground">
        Agents <Badge variant="secondary">{activity.children.length}</Badge>
      </div>
      <div className="flex flex-col gap-1">
        {activity.children.map((child) => (
          <div className="flex min-w-0 items-center gap-2 text-[12px]" key={child.id ?? child.title}>
            <CircleDotIcon className="size-3 shrink-0 text-success" />
            <span className="min-w-0 truncate font-medium">{child.title}</span>
            {child.backend || child.model ? (
              <span className="ms-auto shrink-0 text-[10px] text-muted-foreground">
                {[child.backend, child.model].filter(Boolean).join(" · ")}
              </span>
            ) : null}
          </div>
        ))}
      </div>
    </section>
  );
}

export function TaskListPanel({ groups }: { groups: TaskGroup[] }) {
  const tasks = groups.flatMap((group) => group.tasks);
  if (tasks.length === 0) return null;
  return (
    <Collapsible defaultOpen>
      <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-[13px] font-medium text-muted-foreground hover:bg-accent">
        Tasks <Badge variant="secondary">{tasks.length}</Badge>
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

export function WorkflowListPanel({ runs }: { runs: WorkflowRun[] }) {
  if (runs.length === 0) return null;
  return (
    <Collapsible defaultOpen>
      <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-[13px] font-medium text-muted-foreground hover:bg-accent">
        Workflows <Badge variant="secondary">{runs.length}</Badge>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="flex flex-col gap-2 px-3 py-1.5">
          {runs.map((run) => <WorkflowRunRow key={run.runId} run={run} />)}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function WorkflowRunRow({ run }: { run: WorkflowRun }) {
  const status = run.status ?? "unknown";
  return (
    <div className="rounded-md border bg-card px-2.5 py-2 text-sm">
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="font-medium">{run.name ?? run.runId}</span>
        <span className={cn("text-xs", workflowStatusClass(status))}>{status}</span>
        {run.background ? <span className="text-[11px] text-muted-foreground">background</span> : null}
        {run.currentPhase ? <span className="text-xs text-muted-foreground">· {run.currentPhase}</span> : null}
      </div>
      {run.description ? <p className="mt-1 text-xs text-muted-foreground">{run.description}</p> : null}
      {run.agents.length > 0 ? (
        <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[11px] text-muted-foreground">
          {run.agents.map((agent, index) => (
            <span key={`${agent.label ?? "agent"}-${index}`}>
              {agent.label ?? `agent-${index + 1}`}: {agent.state ?? "unknown"}
              {agent.model ? ` · ${agent.model}` : ""}
              {agent.completedOperations !== undefined ? ` · ${agent.completedOperations} ops` : ""}
            </span>
          ))}
        </div>
      ) : null}
      {run.phases.length > 0 ? (
        <div className="mt-1 text-[11px] text-muted-foreground">
          Phases: {run.phases.map((phase) => phase.title).join(" · ")}
        </div>
      ) : null}
      {run.resultArtifact || run.transcriptArtifact ? (
        <div className="mt-1 font-mono text-[10px] text-muted-foreground">
          Artifacts: {[run.resultArtifact, run.transcriptArtifact].filter(Boolean).join(" · ")}
        </div>
      ) : null}
      {run.error ? <p className="mt-1 text-xs text-destructive">{run.error}</p> : null}
    </div>
  );
}

function workflowStatusClass(status: string): string {
  if (status === "completed") return "text-success";
  if (status === "running") return "text-warning";
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
