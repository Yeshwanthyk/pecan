/**
 * Thread view: scrollable transcript with turn folding, user bubbles,
 * collapsible thinking + deterministic tool activity, ask_user cards and
 * workspace-change summaries. Pins to bottom while the reader is already near
 * it so streaming never yanks the viewport.
 */
import {
  ActivityIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleAlertIcon,
} from "lucide-react";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import type { DatedEntry, ThreadEntry, ThreadView, ToolCall } from "~/api/types";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "~/components/ui/collapsible";
import { Spinner } from "~/components/ui/spinner";
import { ChatMarkdown, DiffBlock } from "~/components/chat-markdown";
import { CopyButton } from "~/components/copy-button";
import { TaskListPanel, WorkflowListPanel } from "~/components/extension-ui";
import { useApp } from "~/store";
import { cn } from "~/lib/utils";

const OPEN_TURNS = 3;
const TOOL_CATEGORY_ORDER: ToolCategory[] = [
  "inspect",
  "change",
  "check",
  "research",
  "agent",
  "task",
  "runtime",
  "other",
];
const TOOL_CATEGORY_LABELS: Record<ToolCategory, string> = {
  inspect: "Inspect",
  change: "Changes",
  check: "Checks",
  research: "Research",
  agent: "Agents",
  task: "Task plan",
  runtime: "Runtime",
  other: "Other",
};

export function Thread({ data }: { data: ThreadView }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);

  // Keep newest content in view only when the reader hasn't scrolled up.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !pinnedRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [data]);

  const turns = useMemo(() => buildTurns(data.entries), [data.entries]);
  const foldCount = Math.max(0, turns.length - OPEN_TURNS);
  const openTurns = turns.slice(foldCount);

  return (
    <div
      className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
      onScroll={(event) => {
        const el = event.currentTarget;
        pinnedRef.current =
          el.scrollHeight - el.scrollTop - el.clientHeight < 140;
      }}
      ref={scrollRef}
    >
      <div className="mx-auto w-full max-w-[46rem] px-4 pt-4 pb-6 md:px-6">
        {foldCount > 0 ? <FoldedTurns turns={turns.slice(0, foldCount)} /> : null}
        {openTurns.map((turn) => (
          <TurnBlock key={turn.key} turn={turn} />
        ))}
        <StreamingDraft sessionId={data.summary.id} />
        <Panels data={data} />
        <div className="h-2" />
      </div>
    </div>
  );
}

/** Live partial assistant message assembled from streaming deltas. */
function StreamingDraft({ sessionId }: { sessionId: string }) {
  const draft = useApp((state) =>
    state.streamingDraft?.id === sessionId ? state.streamingDraft : null,
  );
  const model = useApp((state) =>
    state.agent?.sessionId === sessionId
      ? (state.agent.state.model?.displayName ?? state.agent.state.model?.id)
      : null,
  );
  if (!draft || draft.text.length === 0) return null;
  return (
    <div className="py-2">
      <div className="mb-1 flex h-5 items-center gap-2 text-[11px] font-medium text-muted-foreground/80">
        pecan
        {model ? <span className="font-normal">{shortModel(model)}</span> : null}
        <Spinner className="size-3" />
      </div>
      <div className="chat-md prose prose-sm break-words">
        <ChatMarkdown text={draft.text} />
      </div>
    </div>
  );
}

function FoldedTurns({ turns }: { turns: Turn[] }) {
  const [open, setOpen] = useState(false);
  if (open) {
    return (
      <>
        <button
          className="-ml-1.5 mb-3 flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
          onClick={() => setOpen(false)}
          type="button"
        >
          <ChevronDownIcon className="size-3.5" /> Hide earlier turns
        </button>
        {turns.map((turn) => (
          <TurnBlock key={turn.key} turn={turn} />
        ))}
      </>
    );
  }
  const last = turns.at(-1);
  return (
    <button
      className="-ml-1.5 mb-4 flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-xs font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
      onClick={() => setOpen(true)}
      type="button"
    >
      <ChevronRightIcon className="size-3.5 shrink-0" />
      Earlier turns ({turns.length})
      {last?.firstLine ? (
        <span className="min-w-0 truncate opacity-70">· {last.firstLine}</span>
      ) : null}
    </button>
  );
}

function TurnBlock({ turn }: { turn: Turn }) {
  return (
    <section className="py-1.5">
      <ToolRunBlock entries={turn.entries} />
    </section>
  );
}

/**
 * Groups consecutive assistant entries that carry only tools into one
 * collapsed row, so streaming turns read quietly instead of repeating the
 * "pecan · model" header per entry. Thinking-bearing entries stay visible.
 */
const ToolRunBlock = memo(function ToolRunBlock({
  entries,
}: {
  entries: DatedEntry[];
}) {
  const blocks: Array<
    | { kind: "run"; tools: ToolCall[] }
    | { kind: "entry"; dated: DatedEntry }
  > = [];
  for (const dated of entries) {
    const entry = dated.entry;
    const isRunPart =
      entry.kind === "assistant" &&
      !entry.text &&
      !entry.thinking &&
      (entry.tools?.length ?? 0) > 0;
    if (!isRunPart) {
      blocks.push({ kind: "entry", dated });
      continue;
    }
    const last = blocks.at(-1);
    if (last?.kind === "run") {
      last.tools.push(...(entry.tools ?? []));
    } else {
      blocks.push({ kind: "run", tools: [...(entry.tools ?? [])] });
    }
  }

  return (
    <>
      {blocks.map((block, index) =>
        block.kind === "run" ? (
          <ToolRun key={runKey(index, block.tools)} tools={block.tools} />
        ) : (
          <EntryRow key={entryKey(block.dated)} dated={block.dated} />
        ),
      )}
    </>
  );
});

function runKey(index: number, tools: ToolCall[]) {
  return `run-${index}-${tools[0]?.toolCallId ?? ""}`;
}

/** One tool run, collapsed until the reader asks for its categorized actions. */
function ToolRun({ tools }: { tools: ToolCall[] }) {
  return <ToolActivity tools={tools} />;
}

type ToolCategory = ToolCall["category"];
type ToolGroup = { category: ToolCategory; tools: ToolCall[] };

function ToolActivity({ tools }: { tools: ToolCall[] }) {
  const groups = groupTools(tools);
  const firstSummary = tools[0]?.summary;
  const title =
    tools.length === 1
      ? (firstSummary ?? "Tool activity")
      : firstSummary
        ? `${firstSummary} + ${tools.length - 1} more actions`
        : "Tool activity";
  return (
    <Collapsible className="my-1 min-w-0" defaultOpen={false}>
      <CollapsibleTrigger
        aria-label={`Show ${tools.length} tool ${tools.length === 1 ? "action" : "actions"}`}
        className="group flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md px-1.5 text-left text-[12px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-8"
      >
        <ActivityIcon className="size-3.5 shrink-0 opacity-70" />
        <span className="min-w-0 flex-1 truncate font-medium">{title}</span>
        <span className="shrink-0 tabular-nums text-[11px] opacity-70">
          {tools.length} {tools.length === 1 ? "action" : "actions"}
        </span>
        <ChevronRightIcon className="size-3.5 shrink-0 transition-transform group-data-[panel-open]:rotate-90" />
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-0.5 border-t ps-1 sm:ps-4">
          {groups.map((group) => (
            <ToolActivityGroup group={group} key={group.category} />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function groupTools(tools: ToolCall[]): ToolGroup[] {
  const grouped = new Map<ToolCategory, ToolCall[]>();
  for (const tool of tools) {
    const current = grouped.get(tool.category);
    if (current) current.push(tool);
    else grouped.set(tool.category, [tool]);
  }
  return TOOL_CATEGORY_ORDER.flatMap((category) => {
    const categoryTools = grouped.get(category);
    return categoryTools ? [{ category, tools: categoryTools }] : [];
  });
}

function ToolActivityGroup({ group }: { group: ToolGroup }) {
  const isTaskPlan = group.category === "task";
  return (
    <Collapsible defaultOpen={!isTaskPlan}>
      <CollapsibleTrigger className="group flex min-h-11 w-full items-center gap-2 px-1.5 text-[11px] font-medium text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-8">
        <ChevronRightIcon className="size-3 shrink-0 transition-transform group-data-[panel-open]:rotate-90" />
        <span>{TOOL_CATEGORY_LABELS[group.category]}</span>
        <span className="ms-auto tabular-nums opacity-70">{group.tools.length}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="pb-1">
          {group.tools.map((tool, index) => (
            <ToolAction key={`${tool.toolCallId}-${index}`} tool={tool} />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function ToolAction({ tool }: { tool: ToolCall }) {
  const body = previewBody(tool.argsPreview);
  const diff = looksLikeDiff(body);
  return (
    <Collapsible>
      <CollapsibleTrigger
        className="group flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md px-1.5 text-left text-[12px] text-foreground/85 outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring md:min-h-8"
        title={tool.summary}
      >
        <ChevronRightIcon className="size-3 shrink-0 text-muted-foreground transition-transform group-data-[panel-open]:rotate-90" />
        <span className="min-w-0 flex-1 truncate">{tool.summary}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="pb-2 ps-5 pe-1 sm:ps-6">
          {tool.targetCount > 0 ? (
            <div className="mb-1 text-[11px] text-muted-foreground">
              Targets ({tool.targetCount}): {tool.targets.join(", ") || "not retained"}
            </div>
          ) : null}
          {diff ? <DiffBlock code={body} /> : null}
          <ToolBody showArguments={!diff} tool={tool} />
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Memoized so unchanged entries skip re-render on SSE refreshes. */
const EntryRow = memo(function EntryRow({ dated }: { dated: DatedEntry }) {
  const entry = dated.entry;
  if (entry.kind === "user") return <UserMessage entry={entry} />;
  if (entry.kind === "assistant") return <AssistantMessage entry={entry} />;
  if (entry.kind === "toolError") return <ToolErrorNote text={entry.text} />;
  return <AskUserCard entry={entry} />;
});

/** Quiet inline note for failed tool results — never styled as a user prompt. */
function ToolErrorNote({ text }: { text: string }) {
  return (
    <div className="my-1.5 flex items-start gap-2 rounded-lg border border-destructive/25 bg-destructive/[0.04] px-3 py-2">
      <CircleAlertIcon className="mt-0.5 size-3.5 shrink-0 text-destructive" />
      <div className="min-w-0 flex-1 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-destructive/90">
        {text}
      </div>
    </div>
  );
}

const USER_COLLAPSE_LINES = 22;

type UserEntry = Extract<ThreadEntry, { kind: "user" }>;

function UserMessage({ entry }: { entry: UserEntry }) {
  const { text, images } = entry;
  const lines = useMemo(() => text.split("\n"), [text]);
  const overflows = lines.length > USER_COLLAPSE_LINES;
  const [expanded, setExpanded] = useState(false);
  const shown = expanded || !overflows ? text : lines.slice(0, USER_COLLAPSE_LINES).join("\n");
  return (
    <div className="flex justify-end py-2">
      <div className="chat-md prose prose-sm relative min-w-0 max-w-[85%] break-words rounded-xl rounded-br-sm border border-bubble-border bg-bubble px-3.5 py-2">
        {text ? (
          <div className={cn(!expanded && overflows && "relative overflow-hidden")}>
            <ChatMarkdown text={shown} />
            {!expanded && overflows ? (
              <div className="pointer-events-none absolute inset-x-0 bottom-0 h-16 bg-gradient-to-t from-bubble to-transparent" />
            ) : null}
          </div>
        ) : null}
        {images.length > 0 ? <UserImages images={images} hasText={Boolean(text)} /> : null}
        {overflows ? (
          <button
            className="mt-1 text-[12px] font-medium text-muted-foreground hover:text-foreground"
            onClick={() => setExpanded((value) => !value)}
            type="button"
          >
            {expanded ? "Show less" : `Show ${lines.length - USER_COLLAPSE_LINES} more lines`}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function UserImages({ images, hasText }: { images: UserEntry["images"]; hasText: boolean }) {
  return (
    <div className={cn("not-prose flex flex-wrap justify-end gap-1.5", hasText && "mt-2")}>
      {images.map((image, index) => {
        const source = `data:${image.mimeType};base64,${image.data}`;
        return (
          <a
            aria-label={`Open attached image ${index + 1}`}
            className="block overflow-hidden rounded-md outline outline-black/10 focus-visible:ring-2 focus-visible:ring-ring dark:outline-white/10"
            href={source}
            key={`${image.mimeType}-${index}`}
            rel="noreferrer"
            target="_blank"
          >
            <img
              alt={`Attached image ${index + 1}`}
              className="h-20 w-16 bg-muted object-cover object-top sm:h-24 sm:w-20"
              decoding="async"
              loading="lazy"
              src={source}
            />
          </a>
        );
      })}
    </div>
  );
}

type AssistantEntry = Extract<ThreadEntry, { kind: "assistant" }>;

function AssistantMessage({ entry }: { entry: AssistantEntry }) {
  const tools = entry.tools ?? [];
  return (
    <div className="py-2">
      <div className="mb-1 flex h-5 items-center gap-2 text-[11px] font-medium text-muted-foreground/80">
        pecan
        {entry.model ? (
          <span className="font-normal">{shortModel(entry.model)}</span>
        ) : null}
        {!entry.text && tools.length === 0 && !entry.thinking ? (
          <Spinner className="size-3" />
        ) : null}
      </div>
      {entry.thinking ? <ThinkingFold text={entry.thinking} /> : null}
      {entry.text ? (
        <div className="group/output relative">
          <CopyButton
            className="absolute top-0 right-0 opacity-0 group-hover/output:opacity-100 focus-visible:opacity-100"
            label="Copy response"
            text={entry.text}
          />
          <div className="chat-md prose prose-sm break-words pe-7">
            <ChatMarkdown text={entry.text} />
          </div>
        </div>
      ) : null}
      {tools.length > 0 ? <ToolActivity tools={tools} /> : null}
    </div>
  );
}

function ThinkingFold({ text }: { text: string }) {
  return (
    <Collapsible className="mb-2">
      <CollapsibleTrigger className="group -ml-1.5 flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-accent hover:text-foreground">
        <ChevronRightIcon className="size-3 transition-transform group-data-[panel-open]:rotate-90" />
        thinking
      </CollapsibleTrigger>
      <CollapsibleContent>
        <div className="mt-1 mb-2 ml-1 max-h-64 overflow-y-auto border-l-2 pl-3 text-[13px] leading-relaxed break-words whitespace-pre-wrap text-muted-foreground">
          {text}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Renders the bounded raw argument preview and any structured tool result. */
function ToolBody({ tool, showArguments = true }: { tool: ToolCall; showArguments?: boolean }) {
  const body = previewBody(tool.argsPreview);
  const lines = body.split("\n");
  const shown = lines.slice(0, 24).join("\n");
  return (
    <>
      {showArguments ? (
        <div className="relative mt-1">
          <CopyButton
            className="absolute top-1 right-1 z-10"
            label="Copy tool arguments"
            text={body}
          />
          <pre className="max-h-56 overflow-auto rounded-md bg-muted p-2.5 pe-10 font-mono text-[11.5px] leading-relaxed break-words whitespace-pre-wrap">
            {shown}
            {lines.length > 24 ? `\n… ${lines.length - 24} more lines` : ""}
          </pre>
        </div>
      ) : null}
      {tool.details !== undefined ? <ToolDetails details={tool.details} /> : null}
    </>
  );
}

function ToolDetails({ details }: { details: unknown }) {
  if (details === null || typeof details !== "object") return null;
  const record = details as Record<string, unknown>;
  const artifactPaths = [record.artifactPath, record.resultArtifact, record.transcriptArtifact]
    .filter((value): value is string => typeof value === "string" && value.length > 0);
  let text: string;
  try {
    text = JSON.stringify(details, null, 2);
  } catch {
    return null;
  }
  const lines = text.split("\n");
  return (
    <div className="mt-1">
      {artifactPaths.length > 0 ? (
        <div className="mb-1 text-[11px] text-muted-foreground">
          Artifacts: {artifactPaths.join(" · ")}
        </div>
      ) : null}
      <div className="relative">
        <CopyButton
          className="absolute top-1 right-1 z-10"
          label="Copy tool result"
          text={text}
        />
        <pre className="max-h-72 overflow-auto rounded-md border bg-card p-2.5 pe-10 font-mono text-[11px] leading-relaxed break-words whitespace-pre-wrap">
          {lines.slice(0, 80).join("\n")}
          {lines.length > 80 ? `\n… ${lines.length - 80} more lines` : ""}
        </pre>
      </div>
    </div>
  );
}

/** Cheap unified-diff sniff: hunk headers plus +/- bodies. */
function looksLikeDiff(text: string): boolean {
  const lines = text.split("\n");
  let hunks = 0;
  let changes = 0;
  for (const line of lines) {
    if (line.startsWith("@@") || line.startsWith("+++ ") || line.startsWith("--- ")) {
      hunks += 1;
    } else if (line.startsWith("+") || line.startsWith("-")) {
      changes += 1;
    }
  }
  return hunks >= 1 && changes >= 2;
}

type AskUserEntry = Extract<ThreadEntry, { kind: "askUser" }>;

const CHANGE_TOOL_NAMES = new Set(["edit", "write", "create", "multiedit"]);

function collectChanges(entries: DatedEntry[]): string[] {
  const paths = new Set<string>();
  for (const dated of entries) {
    if (dated.entry.kind !== "assistant") continue;
    for (const tool of dated.entry.tools ?? []) {
      // Preserve the header's historical meaning: count file paths from the
      // file-edit tools only, not every action classified as a change.
      if (!CHANGE_TOOL_NAMES.has(tool.name.toLowerCase())) continue;
      for (const target of tool.targets) paths.add(target);
    }
  }
  return [...paths];
}

/** Number shown in the persistent header's Diff action. */
export function threadChangeCount(entries: DatedEntry[]): number {
  return collectChanges(entries).length;
}

/**
 * Extracts the displayable body from a (possibly truncated) tool argsPreview.
 * Strict JSON.parse fails on cut-off previews, so string fields are pulled
 * with tolerant regexes and escapes resolved.
 */
function previewBody(preview: string): string {
  const bracketed = /^\s*\[[^\]\n]+\]\s*/.exec(preview);
  const stripped = bracketed ? preview.slice(bracketed[0].length) : preview;
  const body = jsonStringField(stripped, ["text", "content", "command"]);
  return body ?? stripped;
}

/** First matching string field from JSON-ish text, tolerating truncation. */
function jsonStringField(text: string, keys: string[]): string | null {
  for (const key of keys) {
    const match = new RegExp(`"${key}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)`).exec(text);
    if (match?.[1]) {
      try {
        return JSON.parse(`"${match[1]}"`) as string;
      } catch {
        return match[1];
      }
    }
  }
  return null;
}

/**
 * Historical `ask_user` card: shows the question and its options, plus the
 * recorded answer. Live answering is the global extension host's job, so
 * pending requests are never rendered (or answered) from inside a card.
 */
function AskUserCard({ entry }: { entry: AskUserEntry }) {
  const questions = readQuestions(entry.questions);
  const answered = entry.answer !== null && entry.answer !== undefined;
  return (
    <div
      className={cn(
        "my-2 rounded-xl border p-3.5",
        answered ? "border-border" : "border-warning/40 bg-warning/[0.06]",
      )}
    >
      <div
        className={cn(
          "mb-1 text-[11px] font-medium",
          answered ? "text-success" : "text-warning",
        )}
      >
        {answered ? "Answered" : "Question"}
      </div>
      {questions.map((question, qIndex) => (
        <div key={question.question ?? qIndex}>
          <p className="mt-1.5 text-sm font-medium">{question.question}</p>
          <ul className="mt-1 flex flex-col gap-0.5">
            {((question.options ?? []) as Array<{ label?: string }>).map(
              (option, oIndex) => (
                <li
                  className="rounded-md bg-accent/60 px-2.5 py-1 text-[13px] text-muted-foreground"
                  key={option.label ?? oIndex}
                >
                  {option.label ?? "…"}
                </li>
              ),
            )}
          </ul>
        </div>
      ))}
      {answered ? (
        <p className="mt-2 border-t pt-2 text-sm text-muted-foreground">
          → {readText(entry.answer)}
        </p>
      ) : null}
    </div>
  );
}

function Panels({ data }: { data: ThreadView }) {
  const taskCount = data.tasks.reduce(
    (total, list) => total + list.tasks.length,
    0,
  );
  if (taskCount === 0 && data.workflows.length === 0) return null;
  return (
    <div className="mt-6 flex flex-col gap-2 border-t pt-3">
      {taskCount > 0 ? <TaskListPanel groups={data.tasks} /> : null}
      {data.workflows.length > 0 ? <WorkflowListPanel runs={data.workflows} /> : null}
    </div>
  );
}

// ---------------------------------------------------------------- helpers ----

type Turn = {
  key: string;
  entries: DatedEntry[];
  firstLine: string;
};

function buildTurns(entries: DatedEntry[]): Turn[] {
  const turns: Turn[] = [];
  for (const dated of entries) {
    if (dated.entry.kind === "user" || turns.length === 0) {
      turns.push({
        key: entryKey(dated),
        entries: [dated],
        firstLine:
          dated.entry.kind === "user"
            ? (dated.entry.text.split("\n")[0]?.slice(0, 90) ?? "")
            : "",
      });
    } else {
      turns.at(-1)?.entries.push(dated);
    }
  }
  return turns;
}

function entryKey(dated: DatedEntry): string {
  const head =
    dated.entry.kind === "user" ? dated.entry.text.slice(0, 24) : "";
  return `${dated.ts ?? "t"}:${dated.entry.kind}:${head}`;
}


function readQuestions(
  questions: unknown,
): Array<{ question?: string; options?: unknown[] }> {
  if (typeof questions === "object" && questions !== null && "questions" in questions) {
    const inner = (questions as { questions: unknown }).questions;
    if (!Array.isArray(inner)) return [];
    return inner.filter(
      (item): item is { question?: string; options?: unknown[] } =>
        typeof item === "object" && item !== null,
    );
  }
  return [];
}

function readText(answer: unknown): string {
  if (answer === null || answer === undefined) return "";
  if (typeof answer === "string") return answer.slice(0, 200);
  if (Array.isArray(answer)) {
    return answer
      .filter((block): block is { type: "text"; text: string } => block?.type === "text")
      .map((block) => block.text)
      .join(" ")
      .slice(0, 200);
  }
  return JSON.stringify(answer).slice(0, 200);
}

/** "anthropic/claude-sonnet-4" → "claude-sonnet-4". */
function shortModel(model: string) {
  return model.split("/").at(-1) ?? model;
}
