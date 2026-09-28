/**
 * Thread view: scrollable transcript with turn folding, user bubbles,
 * collapsible thinking + deterministic tool activity, ask_user cards and
 * workspace-change summaries. Pins to bottom while the reader is already near
 * it so streaming never yanks the viewport.
 */
import {
  ActivityIcon,
  ArrowUpRightIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleAlertIcon,
  CornerDownLeftIcon,
  MessageCircleQuestionIcon,
} from "lucide-react";
import { createContext, memo, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { DatedEntry, SessionRow, ThreadEntry, ThreadView, ToolCall } from "~/api/types";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "~/components/ui/collapsible";
import { Spinner } from "~/components/ui/spinner";
import { DiffBlock } from "~/components/diff-block";
import { ChatMarkdown } from "~/components/markdown";
import { CopyButton } from "~/components/copy-button";
import { TaskListPanel, WorkflowListPanel } from "~/components/extension-ui";
import { api } from "~/api/client";
import { useApp } from "~/store";
import { agentTitle, shortModel } from "~/lib/format";
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

/** A `subagent_spawn` tool call resolved to the child session it produced. */
type SpawnLink = { childId: string; parentId: string };

/** Keyed by `toolCallId`; empty outside a `Thread` that found any spawns. */
const SpawnLinksContext = createContext<Map<string, SpawnLink>>(new Map());

function parseSpawnDetails(details: unknown): { title: string; cwd: string } | null {
  if (typeof details !== "object" || details === null) return null;
  const record = details as Record<string, unknown>;
  const { title, cwd } = record;
  if (typeof title !== "string" || typeof cwd !== "string") return null;
  return { title, cwd };
}

/**
 * Resolves each `subagent_spawn` tool call in this thread to the child
 * session row it produced, so the tool row can link straight to it.
 *
 * Pi's toolResult carries the spawn's title/cwd but never a session id, and
 * the subagent-manager's spawn-id counter is reused across spawn rounds, so
 * the same title can legitimately be spawned more than once in one thread.
 * Matching goes through the same title+cwd+recency claiming `SubagentStrip`
 * uses for live activity: each session row is claimed by at most one call,
 * closest by open time, so a repeated title can't link two calls to the
 * same child or vice versa. `cwd` here is the exact cwd recorded on that
 * spawn (not the parent's own cwd) since a child can run in a different
 * cwd than its parent.
 */
function resolveSpawnChildren(
  entries: DatedEntry[],
  sessions: SessionRow[],
  parentId: string,
): Map<string, SpawnLink> {
  const spawns: Array<{ toolCallId: string; title: string; cwd: string; ts: number }> = [];
  for (const dated of entries) {
    if (dated.entry.kind !== "assistant") continue;
    const ts = dated.ts ? Date.parse(dated.ts) : Number.NaN;
    for (const tool of dated.entry.tools ?? []) {
      if (tool.name !== "subagent_spawn") continue;
      const spawn = parseSpawnDetails(tool.details);
      if (spawn) spawns.push({ toolCallId: tool.toolCallId, ts, ...spawn });
    }
  }
  if (spawns.length === 0) return new Map();

  const candidates = sessions.filter((row) => row.kind === "subagent");
  const claimed = new Set<string>();
  const links = new Map<string, SpawnLink>();
  for (const spawn of spawns) {
    const title = agentTitle(spawn.title);
    const match = candidates
      .filter(
        (row) =>
          !claimed.has(row.id) &&
          row.cwd === spawn.cwd &&
          agentTitle(row.agentName ?? "") === title &&
          (!Number.isFinite(spawn.ts) ||
            !Number.isFinite(Date.parse(row.openedAt)) ||
            Date.parse(row.openedAt) >= spawn.ts),
      )
      .sort(
        (a, b) =>
          Math.abs(Date.parse(a.openedAt) - spawn.ts) -
          Math.abs(Date.parse(b.openedAt) - spawn.ts),
      )[0];
    if (match) {
      claimed.add(match.id);
      links.set(spawn.toolCallId, { childId: match.id, parentId });
    }
  }
  return links;
}

/** Distinct cwds this thread's `subagent_spawn` calls ran in. */
function spawnCwds(entries: DatedEntry[]): string[] {
  const cwds = new Set<string>();
  for (const dated of entries) {
    if (dated.entry.kind !== "assistant") continue;
    for (const tool of dated.entry.tools ?? []) {
      if (tool.name !== "subagent_spawn") continue;
      const spawn = parseSpawnDetails(tool.details);
      if (spawn) cwds.add(spawn.cwd);
    }
  }
  return [...cwds].sort();
}

/**
 * Store sessions plus the subagent rows for every cwd this thread spawned
 * into. The store's page lists threads first and caps its size, so older
 * children (or children in unlinked folders) are usually missing from it.
 */
function useSpawnCandidates(entries: DatedEntry[]): SessionRow[] {
  const sessions = useApp((state) => state.sessions);
  const key = spawnCwds(entries).join("\n");
  const [fetched, setFetched] = useState<SessionRow[]>([]);

  useEffect(() => {
    if (key === "") return;
    let cancelled = false;
    Promise.all(
      key.split("\n").map((project) =>
        api.sessions({ project, kind: "subagent", limit: 400 }),
      ),
    )
      .then((pages) => {
        if (!cancelled) setFetched(pages.flatMap((page) => page.sessions));
      })
      .catch(() => {
        // Links fall back to whatever the store already holds.
      });
    return () => {
      cancelled = true;
    };
  }, [key]);

  return useMemo(() => {
    if (fetched.length === 0) return sessions;
    const seen = new Set(sessions.map((row) => row.id));
    return [...sessions, ...fetched.filter((row) => !seen.has(row.id))];
  }, [sessions, fetched]);
}

export function Thread({ data }: { data: ThreadView }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const sessions = useSpawnCandidates(data.entries);

  // Keep newest content in view only when the reader hasn't scrolled up.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !pinnedRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [data]);

  const turns = useMemo(() => buildTurns(data.entries), [data.entries]);
  const foldCount = Math.max(0, turns.length - OPEN_TURNS);
  const openTurns = turns.slice(foldCount);
  const spawnLinks = useMemo(
    () => resolveSpawnChildren(data.entries, sessions, data.summary.id),
    [data.entries, sessions, data.summary.id],
  );

  return (
    <div
      aria-label="Conversation"
      className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto [mask-image:linear-gradient(to_bottom,transparent,black_20px,black_calc(100%-20px),transparent)]"
      data-testid="thread"
      onScroll={(event) => {
        const el = event.currentTarget;
        pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 140;
      }}
      ref={scrollRef}
    >
      <SpawnLinksContext.Provider value={spawnLinks}>
        <div className="mx-auto w-full max-w-[46rem] px-4 pt-4 pb-6 md:px-6" role="log">
          {turns.length === 0 ? <BlankThread cwd={data.summary.cwd} /> : null}
          {foldCount > 0 ? <FoldedTurns turns={turns.slice(0, foldCount)} /> : null}
          {openTurns.map((turn) => (
            <TurnBlock key={turn.key} turn={turn} />
          ))}
          <StreamingDraft sessionId={data.summary.id} />
          <Panels data={data} />
          <div className="h-2" />
        </div>
      </SpawnLinksContext.Provider>
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
  const working = useApp((state) => state.streaming && state.thread?.summary.id === sessionId);
  if (!draft || draft.text.length === 0) {
    if (!working) return null;
    return (
      <div
        className="flex h-8 items-center gap-2 text-[11px] font-medium text-muted-foreground/80"
        data-testid="working"
      >
        <Spinner className="size-3" />
        Working
        {model ? <span className="font-normal">· {shortModel(model)}</span> : null}
      </div>
    );
  }
  return (
    <div className="py-2">
      <div className="mb-1 flex h-5 items-center gap-1.5 text-[11px] text-muted-foreground/70">
        <span>Pi{model ? ` · ${shortModel(model)}` : ""}</span>
        <Spinner className="size-3" />
      </div>
      <div className="chat-md prose prose-sm break-words">
        <ChatMarkdown text={draft.text} />
      </div>
    </div>
  );
}

/** First-run hint for a session with no turns yet. */
function BlankThread({ cwd }: { cwd: string }) {
  return (
    <div className="flex flex-col items-center py-[18vh] text-center text-muted-foreground">
      <p className="text-sm font-medium text-foreground">New session</p>
      <p className="mt-1 max-w-xs truncate font-mono text-[11px]" title={cwd}>
        {cwd}
      </p>
      <p className="mt-3 text-xs">Ask anything below. Esc stops a running turn.</p>
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

/** Groups consecutive tool-loop entries into one quiet activity block. */
const ToolRunBlock = memo(function ToolRunBlock({ entries }: { entries: DatedEntry[] }) {
  const blocks: Array<
    | {
        kind: "run";
        tools: ToolCall[];
        thoughts: string[];
        model: string | null;
      }
    | { kind: "entry"; dated: DatedEntry }
  > = [];
  for (const dated of entries) {
    const entry = dated.entry;
    const isRunPart =
      entry.kind === "assistant" &&
      !entry.text &&
      !entry.error &&
      ((entry.tools?.length ?? 0) > 0 || Boolean(entry.thinking));
    if (!isRunPart) {
      blocks.push({ kind: "entry", dated });
      continue;
    }
    const last = blocks.at(-1);
    if (last?.kind === "run") {
      last.tools.push(...(entry.tools ?? []));
      if (entry.thinking) last.thoughts.push(entry.thinking);
    } else {
      blocks.push({
        kind: "run",
        tools: [...(entry.tools ?? [])],
        thoughts: entry.thinking ? [entry.thinking] : [],
        model: entry.model ?? null,
      });
    }
  }

  return (
    <>
      {blocks.map((block, index) =>
        block.kind === "run" ? (
          <ToolRun
            key={runKey(index, block.tools)}
            model={block.model}
            thoughts={block.thoughts}
            tools={block.tools}
          />
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

/** One compact tool loop: one header, latest thought, one activity row. */
function ToolRun({
  tools,
  thoughts,
  model,
}: {
  tools: ToolCall[];
  thoughts: string[];
  model: string | null;
}) {
  const latestThought = thoughts.at(-1);
  return (
    <div className="py-1.5">
      <AssistantByline model={model} />
      {latestThought ? <ThinkingLine text={latestThought} /> : null}
      {tools.length > 0 ? <ToolActivity tools={tools} /> : null}
    </div>
  );
}

type ToolCategory = ToolCall["category"];
type ToolGroup = { category: ToolCategory; tools: ToolCall[] };

function ToolActivity({ tools }: { tools: ToolCall[] }) {
  const groups = groupTools(tools);
  const firstSummary = tools[0]?.summary;
  const title =
    tools.length === 1
      ? (firstSummary ?? "Tool activity")
      : `${tools.length} actions${firstSummary ? ` · ${firstSummary}` : ""}`;
  const only = tools.length === 1 ? tools[0] : undefined;
  const [open, setOpen] = useState(false);
  const spawnLinks = useContext(SpawnLinksContext);
  const spawned = tools.flatMap((tool) => {
    const link = spawnLinks.get(tool.toolCallId);
    return link ? [{ tool, link }] : [];
  });
  if (only) return <ToolAction lead tool={only} />;
  return (
    <Collapsible className="my-0.5 min-w-0" onOpenChange={setOpen} open={open}>
      <CollapsibleTrigger
        aria-label={`Show ${tools.length} tool ${tools.length === 1 ? "action" : "actions"}`}
        className="group -ms-1.5 flex min-h-11 max-w-full min-w-0 items-center gap-1.5 rounded-md px-1.5 text-left text-[13px] text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-7"
      >
        <ActivityIcon className="size-3.5 shrink-0 opacity-70" />
        <span className="min-w-0 truncate">{title}</span>
        <ChevronRightIcon className="size-3.5 shrink-0 opacity-0 transition-[rotate,opacity] duration-200 group-hover:opacity-70 group-focus-visible:opacity-70 group-data-[panel-open]:rotate-90 group-data-[panel-open]:opacity-70 max-md:opacity-70" />
      </CollapsibleTrigger>
      {!open && spawned.length > 0 ? (
        <div className="ms-5 flex flex-wrap gap-1.5 pb-1">
          {spawned.map(({ tool, link }) => (
            <SpawnChip key={tool.toolCallId} label={spawnLabel(tool)} link={link} />
          ))}
        </div>
      ) : null}
      <CollapsibleContent className="ease-[cubic-bezier(0.2,0,0,1)]">
        <div className="ms-[5px] mt-0.5 mb-1 border-s ps-3">
          {groups.map((group) => (
            <ToolActivityGroup group={group} key={group.category} labelled={groups.length > 1} />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

function spawnLabel(tool: ToolCall): string {
  const title = parseSpawnDetails(tool.details)?.title;
  return title ? agentTitle(title) : "subagent";
}

/** Compact jump into a spawned child's thread. */
function SpawnChip({ label, link }: { label: string; link: SpawnLink }) {
  return (
    <a
      className="flex h-7 max-w-full min-w-0 items-center gap-1 rounded-full border bg-card px-2.5 text-[12px] text-foreground/80 outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring max-md:h-9"
      href={`#/s/${link.childId}?parent=${encodeURIComponent(link.parentId)}`}
      title="Open the subagent's thread"
    >
      <span className="truncate">{label}</span>
      <ArrowUpRightIcon className="size-3 shrink-0 opacity-60" />
    </a>
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

/** Tools of one category; a caption only when a run mixes categories. */
function ToolActivityGroup({ group, labelled }: { group: ToolGroup; labelled: boolean }) {
  return (
    <div className="py-0.5">
      {labelled ? (
        <div className="flex h-7 items-center gap-1.5 text-[11px] text-muted-foreground/80">
          <span>{TOOL_CATEGORY_LABELS[group.category]}</span>
          <span className="tabular-nums">{group.tools.length}</span>
        </div>
      ) : null}
      {group.tools.map((tool, index) => (
        <ToolAction key={`${tool.toolCallId}-${index}`} tool={tool} />
      ))}
    </div>
  );
}

/** One tool call; `lead` styles it as a run's only activity row. */
function ToolAction({ tool, lead = false }: { tool: ToolCall; lead?: boolean }) {
  const body = previewBody(tool.argsPreview);
  const diff = looksLikeDiff(body);
  const spawnLink = useContext(SpawnLinksContext).get(tool.toolCallId);
  return (
    <Collapsible className={lead ? "my-0.5 min-w-0" : undefined}>
      <div className="flex min-w-0 items-center">
        <CollapsibleTrigger
          className={cn(
            "group flex min-h-11 max-w-full min-w-0 flex-1 items-center gap-1.5 rounded-md text-left text-[13px] outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-7",
            lead ? "text-muted-foreground" : "text-foreground/80",
          )}
          title={tool.summary}
        >
          {lead ? <ActivityIcon className="size-3.5 shrink-0 opacity-70" /> : null}
          <span className="min-w-0 truncate">{tool.summary}</span>
          <ChevronRightIcon className="size-3 shrink-0 text-muted-foreground transition-transform duration-200 group-data-[panel-open]:rotate-90" />
        </CollapsibleTrigger>
        {spawnLink ? (
          <a
            className="flex shrink-0 items-center gap-1 rounded-md px-2 text-[11px] text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
            href={`#/s/${spawnLink.childId}?parent=${encodeURIComponent(spawnLink.parentId)}`}
            title="Open the spawned session's thread"
          >
            Open thread
            <ArrowUpRightIcon className="size-3" />
          </a>
        ) : null}
      </div>
      <CollapsibleContent className="ease-[cubic-bezier(0.2,0,0,1)]">
        <div className={cn("pb-2 pe-1", lead && "ms-[5px] mt-0.5 border-s ps-3")}>
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
  if (entry.kind === "childQuestions") return <ChildQuestionsCard entry={entry} />;
  if (entry.kind === "childResults") return <ChildResultsNote entry={entry} />;
  return <AskUserCard entry={entry} />;
});

type ChildQuestionsEntry = Extract<ThreadEntry, { kind: "childQuestions" }>;

/** `ask_parent` questions from subagents; the parent agent replies by request id. */
function ChildQuestionsCard({ entry }: { entry: ChildQuestionsEntry }) {
  return (
    <div className="my-2 flex flex-col gap-2">
      {entry.questions.map((q) => {
        const expired =
          !q.answered && typeof q.deadlineAt === "number" && q.deadlineAt < Date.now();
        return (
          <div
            key={q.requestId}
            className="animate-row-in rounded-xl border border-border/70 bg-muted/30 px-3 py-2.5"
          >
            <div className="flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
              <MessageCircleQuestionIcon className="size-3.5 text-primary" />
              <span className="font-medium text-foreground/90">{q.childId} asks</span>
              <span className="ml-auto tabular-nums">
                {q.answered ? "Replied" : expired ? "Expired" : "Waiting for reply"}
              </span>
            </div>
            <div className="mt-1 text-[13.5px] leading-relaxed whitespace-pre-wrap">
              {q.question}
            </div>
            {q.context ? (
              <div className="mt-1 text-[12px] leading-relaxed whitespace-pre-wrap text-muted-foreground">
                {q.context}
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

type ChildResultsEntry = Extract<ThreadEntry, { kind: "childResults" }>;

/** Settled subagents handed back to this session; the report stays collapsed. */
function ChildResultsNote({ entry }: { entry: ChildResultsEntry }) {
  const [open, setOpen] = useState(false);
  const label =
    entry.results.length > 0
      ? entry.results
          .map((r) => `${r.title || r.id} ${r.status === "done" ? "finished" : r.status}`)
          .join(" · ")
      : "Subagent results";
  const failed = entry.results.some((r) => r.status !== "done");
  return (
    <div className="my-1.5">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full min-w-0 items-center gap-1.5 text-left text-[12px] text-muted-foreground hover:text-foreground"
      >
        <CornerDownLeftIcon className={cn("size-3.5 shrink-0", failed && "text-destructive")} />
        <span className="truncate">{label}</span>
        <ChevronRightIcon
          className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
        />
      </button>
      {open ? (
        <div className="mt-1.5 rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-[12.5px]">
          <ChatMarkdown text={entry.text} />
        </div>
      ) : null}
    </div>
  );
}

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
      <div className="chat-md prose prose-sm relative min-w-0 max-w-[85%] break-words rounded-2xl rounded-br-md bg-bubble px-3.5 py-2 [&_:where(p,ul,ol,pre)]:my-1.5 [&_:where(p,ul,ol,pre):first-child]:mt-0 [&_:where(p,ul,ol,pre):last-child]:mb-0">
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
      <AssistantByline
        loading={!entry.text && tools.length === 0 && !entry.thinking && !entry.error}
        model={entry.model ?? null}
      />
      {entry.thinking ? <ThinkingLine text={entry.thinking} /> : null}
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
      {entry.error ? <TurnError reason={entry.error} /> : null}
    </div>
  );
}

/** A turn that ended early: a quiet note for user aborts, an error otherwise. */
function TurnError({ reason }: { reason: string }) {
  if (reason === "Stopped") {
    return (
      <p className="mt-1 text-[12px] text-muted-foreground" data-testid="turn-stopped">
        Stopped
      </p>
    );
  }
  return (
    <div data-testid="turn-error">
      <ToolErrorNote text={reason} />
    </div>
  );
}

function AssistantByline({ model, loading = false }: { model: string | null; loading?: boolean }) {
  return (
    <div className="mb-1 flex h-5 items-center gap-1.5 text-[11px] text-muted-foreground/70">
      <span>Pi{model ? ` · ${shortModel(model)}` : ""}</span>
      {loading ? <Spinner className="size-3" /> : null}
    </div>
  );
}

function ThinkingLine({ text }: { text: string }) {
  return (
    <p className="mb-1.5 line-clamp-2 text-[12px] italic leading-relaxed text-muted-foreground/65">
      {plainThought(text)}
    </p>
  );
}

function plainThought(text: string): string {
  return text
    .trim()
    .replace(/^\*\*(.+)\*\*$/s, "$1")
    .replace(/^__(.+)__$/s, "$1")
    .replace(/\s+/g, " ");
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
  const artifactPaths = [
    record.artifactPath,
    record.resultArtifact,
    record.transcriptArtifact,
  ].filter((value): value is string => typeof value === "string" && value.length > 0);
  let text: string | null = null;
  try {
    text = JSON.stringify(details, null, 2);
  } catch {
    // Circular or non-serializable tool result; nothing safe to render.
  }
  if (text === null) return null;
  const lines = text.split("\n");
  return (
    <div className="mt-1">
      {artifactPaths.length > 0 ? (
        <div className="mb-1 text-[11px] text-muted-foreground">
          Artifacts: {artifactPaths.join(" · ")}
        </div>
      ) : null}
      <div className="relative">
        <CopyButton className="absolute top-1 right-1 z-10" label="Copy tool result" text={text} />
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
        className={cn("mb-1 text-[11px] font-medium", answered ? "text-success" : "text-warning")}
      >
        {answered ? "Answered" : "Question"}
      </div>
      {questions.map((question, qIndex) => (
        <div key={question.question ?? qIndex}>
          <p className="mt-1.5 text-sm font-medium">{question.question}</p>
          <ul className="mt-1 flex flex-col gap-0.5">
            {((question.options ?? []) as Array<{ label?: string }>).map((option, oIndex) => (
              <li
                className="rounded-md bg-accent/60 px-2.5 py-1 text-[13px] text-muted-foreground"
                key={option.label ?? oIndex}
              >
                {option.label ?? "…"}
              </li>
            ))}
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
  const taskCount = data.tasks.reduce((total, list) => total + list.tasks.length, 0);
  if (taskCount === 0 && data.workflows.length === 0) return null;
  return (
    <div className="mt-6 flex flex-col gap-2 border-t pt-3">
      {taskCount > 0 ? <TaskListPanel groups={data.tasks} /> : null}
      {data.workflows.length > 0 ? (
        <WorkflowListPanel parentId={data.summary.id} runs={data.workflows} />
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
          dated.entry.kind === "user" ? (dated.entry.text.split("\n")[0]?.slice(0, 90) ?? "") : "",
      });
    } else {
      turns.at(-1)?.entries.push(dated);
    }
  }
  return turns;
}

function entryKey(dated: DatedEntry): string {
  const head = dated.entry.kind === "user" ? dated.entry.text.slice(0, 24) : "";
  return `${dated.ts ?? "t"}:${dated.entry.kind}:${head}`;
}

function readQuestions(questions: unknown): Array<{ question?: string; options?: unknown[] }> {
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
