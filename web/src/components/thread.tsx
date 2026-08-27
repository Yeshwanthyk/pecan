/**
 * Thread view: scrollable transcript with turn folding, user bubbles,
 * collapsible thinking + tool chips (capped with "+N" expander), ask_user
 * cards and workspace-change summaries. Pins to bottom while the reader is already
 * near it so streaming never yanks the viewport.
 */
import {
  ChevronDownIcon,
  ChevronRightIcon,
  CircleAlertIcon,
  WrenchIcon,
} from "lucide-react";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import type { DatedEntry, ThreadEntry, ThreadView, ToolCall } from "~/api/types";
import { Badge } from "~/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "~/components/ui/collapsible";
import { Spinner } from "~/components/ui/spinner";
import { ChatMarkdown, DiffBlock } from "~/components/chat-markdown";
import { TaskListPanel } from "~/components/extension-ui";
import { useApp } from "~/store";
import { cn } from "~/lib/utils";

const OPEN_TURNS = 3;
const MAX_VISIBLE_TOOLS = 4;

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
 * Groups consecutive assistant entries that carry no text (tool/thinking
 * runs) into one collapsed row, so streaming turns read quietly instead of
 * repeating the "pecan · model" header per entry.
 */
const ToolRunBlock = memo(function ToolRunBlock({
  entries,
}: {
  entries: DatedEntry[];
}) {
  const blocks: Array<
    | { kind: "run"; tools: ToolCall[]; model: string | null }
    | { kind: "entry"; dated: DatedEntry }
  > = [];
  for (const dated of entries) {
    const entry = dated.entry;
    const isRunPart =
      entry.kind === "assistant" &&
      !entry.text &&
      ((entry.tools?.length ?? 0) > 0 || Boolean(entry.thinking));
    if (!isRunPart) {
      blocks.push({ kind: "entry", dated });
      continue;
    }
    const last = blocks.at(-1);
    if (last?.kind === "run") {
      last.tools.push(...(entry.tools ?? []));
    } else {
      blocks.push({ kind: "run", tools: [...(entry.tools ?? [])], model: entry.model ?? null });
    }
  }

  return (
    <>
      {blocks.map((block, index) =>
        block.kind === "run" ? (
          <ToolRun key={runKey(index, block.tools)} model={block.model} tools={block.tools} />
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

/** One tool run: shows its tool chips by default, with a compact toggle. */
function ToolRun({ tools, model }: { tools: ToolCall[]; model: string | null }) {
  const [open, setOpen] = useState(true);
  const summary = runSummary(tools);
  return (
    <div className="py-0.5">
      <button
        aria-expanded={open}
        className="group flex w-full items-center gap-2 rounded-md px-1 py-0.5 -ml-1 text-left text-[11px] font-medium text-muted-foreground/80 hover:bg-accent hover:text-foreground"
        onClick={() => setOpen((value) => !value)}
        type="button"
      >
        <ChevronRightIcon
          className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
        />
        <span className="shrink-0">pecan</span>
        {model ? (
          <span className="shrink-0 font-normal">{shortModel(model)}</span>
        ) : null}
        <span className="min-w-0 truncate font-normal opacity-80">
          {summary}
        </span>
        <span className="ms-auto shrink-0 tabular-nums opacity-70">
          {tools.length} tools
        </span>
      </button>
      {open ? (
        <div className="mt-1 flex flex-wrap items-center gap-1 ps-4">
          <ToolChips tools={tools} forceAll />
        </div>
      ) : null}
    </div>
  );
}

/** Short human summary for a collapsed run: files touched or command head. */
function runSummary(tools: ToolCall[]): string {
  const paths = new Set<string>();
  let firstCommand = "";
  for (const tool of tools) {
    const path = filePathOf(tool.argsPreview);
    if (path) {
      paths.add(path.split("/").at(-1) ?? path);
      continue;
    }
    const body = previewBody(tool.argsPreview);
    if (!firstCommand && body.length > 0) {
      firstCommand = body.split("\n")[0]?.slice(0, 40) ?? "";
    }
  }
  const parts: string[] = [];
  if (paths.size > 0) parts.push([...paths].slice(0, 3).join(", "));
  if (firstCommand) parts.push(firstCommand);
  return parts.join(" · ");
}

/** Memoized so unchanged entries skip re-render on SSE refreshes. */
const EntryRow = memo(function EntryRow({ dated }: { dated: DatedEntry }) {
  const entry = dated.entry;
  if (entry.kind === "user") return <UserMessage text={entry.text} />;
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

function UserMessage({ text }: { text: string }) {
  const lines = useMemo(() => text.split("\n"), [text]);
  const overflows = lines.length > USER_COLLAPSE_LINES;
  const [expanded, setExpanded] = useState(false);
  const shown = expanded || !overflows ? text : lines.slice(0, USER_COLLAPSE_LINES).join("\n");
  return (
    <div className="flex justify-end py-2">
      <div className="chat-md prose prose-sm relative min-w-0 max-w-[85%] break-words rounded-xl rounded-br-sm border border-bubble-border bg-bubble px-3.5 py-2">
        <div className={cn(!expanded && overflows && "relative overflow-hidden")}>
          <ChatMarkdown text={shown} />
          {!expanded && overflows ? (
            <div className="pointer-events-none absolute inset-x-0 bottom-0 h-16 bg-gradient-to-t from-bubble to-transparent" />
          ) : null}
        </div>
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
        {!entry.text && tools.length === 0 ? (
          <Spinner className="size-3" />
        ) : null}
      </div>
      {entry.thinking ? <ThinkingFold text={entry.thinking} /> : null}
      {entry.text ? (
        <div className="chat-md prose prose-sm break-words">
          <ChatMarkdown text={entry.text} />
        </div>
      ) : null}
      {tools.length > 0 ? <ToolChips tools={tools} /> : null}
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

function ToolChips({
  tools,
  forceAll = false,
}: {
  tools: ToolCall[];
  forceAll?: boolean;
}) {
  const [showAll, setShowAll] = useState(false);
  const showEverything = showAll || forceAll;
  const visible = showEverything ? tools : tools.slice(0, MAX_VISIBLE_TOOLS);
  const hiddenCount = tools.length - visible.length;
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-1">
      {visible.map((tool) => (
        <ToolChip key={tool.toolCallId} tool={tool} />
      ))}
      {!showAll && hiddenCount > 0 ? (
        <button
          className="rounded-md border border-dashed px-2 py-0.5 font-mono text-[11px] text-muted-foreground hover:bg-accent"
          onClick={() => setShowAll(true)}
          type="button"
        >
          +{hiddenCount} more
        </button>
      ) : null}
      {showAll && tools.length > MAX_VISIBLE_TOOLS ? (
        <button
          className="rounded-md px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-accent"
          onClick={() => setShowAll(false)}
          type="button"
        >
          less
        </button>
      ) : null}
    </div>
  );
}

function ToolChip({ tool }: { tool: ToolCall }) {
  const label = chipLabel(tool);
  return (
    <Collapsible className="min-w-0">
      <CollapsibleTrigger
        className="group flex min-w-0 max-w-full items-center gap-1 rounded-md border bg-card px-2 py-0.5 font-mono text-[11px] text-muted-foreground normal-case hover:bg-accent hover:text-foreground"
        title={previewBody(tool.argsPreview).split("\n")[0]?.slice(0, 160)}
      >
        <WrenchIcon className="size-3 shrink-0 opacity-60" />
        <span className="truncate">{label}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        {looksLikeDiff(previewBody(tool.argsPreview)) ? (
          <DiffBlock code={previewBody(tool.argsPreview)} />
        ) : (
          <ToolBody tool={tool} />
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Chip label differentiates targets: file basename for file tools,
 *  command head for bash, otherwise the bare tool name. */
function chipLabel(tool: ToolCall): string {
    const path = filePathOf(tool.argsPreview);
    if (path) {
      const base = path.split("/").at(-1) ?? path;
      const action =
        tool.name.toLowerCase() === "write" || tool.name.toLowerCase() === "create"
          ? "+"
          : "~";
      return `${action} ${base}`;
    }
    if (tool.name.toLowerCase() === "bash") {
      const command = previewBody(tool.argsPreview).split("\n")[0] ?? "";
      const trimmed = command.length > 28 ? `${command.slice(0, 28)}…` : command;
      if (trimmed) return `$ ${trimmed}`;
    }
    return tool.name;
}

/** Renders a tool body: unwrapped JSON fields, mono pre with head cap. */
function ToolBody({ tool }: { tool: ToolCall }) {
  const body = previewBody(tool.argsPreview);
  const lines = body.split("\n");
  const shown = lines.slice(0, 24).join("\n");
  return (
    <pre className="mt-1 max-h-56 overflow-auto rounded-md bg-muted p-2.5 font-mono text-[11.5px] leading-relaxed break-words whitespace-pre-wrap">
      {shown}
      {lines.length > 24 ? `\n… ${lines.length - 24} more lines` : ""}
    </pre>
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

type ThreadChange = { path: string; previews: string[] };

function collectChanges(entries: DatedEntry[]): ThreadChange[] {
  const byPath = new Map<string, ThreadChange>();
  for (const dated of entries) {
    if (dated.entry.kind !== "assistant") continue;
    for (const tool of dated.entry.tools ?? []) {
      const lower = tool.name.toLowerCase();
      if (!["edit", "write", "create", "multiedit"].includes(lower)) continue;
      const path = filePathOf(tool.argsPreview);
      if (!path) continue;
      const existing = byPath.get(path);
      if (existing) existing.previews.push(tool.argsPreview);
      else byPath.set(path, { path, previews: [tool.argsPreview] });
    }
  }
  return [...byPath.values()];
}

/** Number shown in the persistent header's Diff action. */
export function threadChangeCount(entries: DatedEntry[]): number {
  return collectChanges(entries).length;
}

/** Pulls a file path out of a tool argsPreview: `[path]` prefix or JSON field. */
function filePathOf(preview: string): string | null {
  const bracket = /^\s*\[([^\]\n]+)\]/.exec(preview);
  if (bracket?.[1]) return bracket[1];
  return jsonStringField(preview, ["filePath", "file_path", "path", "notebookPath"]);
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
      {data.workflows.length > 0 ? (
        <Collapsible>
          <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-[13px] font-medium text-muted-foreground hover:bg-accent">
            Workflows <Badge variant="secondary">{data.workflows.length}</Badge>
          </CollapsibleTrigger>
          <CollapsibleContent>
            {data.workflows.map((workflow) => (
              <div
                className="flex flex-wrap items-baseline gap-x-3 px-3 py-1 text-sm"
                key={workflow.runId}
              >
                <span className="font-medium">
                  {workflow.name ?? workflow.runId}
                </span>
                <span className="text-xs text-muted-foreground">
                  {workflow.agents
                    .map((agent) => `${agent.label ?? "?"}: ${agent.state ?? "?"}`)
                    .join(" · ")}
                </span>
              </div>
            ))}
          </CollapsibleContent>
        </Collapsible>
      ) : null}
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
