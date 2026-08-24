/**
 * Wire types for pecan's HTTP API, expressed once as TypeBox schemas.
 * Runtime validation happens in `client.ts`; these are the single source of truth.
 */
import {
  Array,
  Boolean,
  Intersect,
  Literal,
  Null,
  Number,
  Object,
  Optional,
  String,
  Undefined,
  Union,
  Unknown,
  type Static,
} from "@sinclair/typebox";

export const SessionKind = Union([Literal("normal"), Literal("subagent")]);
export type SessionKind = Static<typeof SessionKind>;

export const SessionSummary = Object({
  id: String(),
  path: String(),
  cwd: String(),
  openedAt: String(),
  lastActivity: String(),
  bytes: Number(),
  provider: Optional(Union([String(), Null(), Undefined()])),
  model: Optional(Union([String(), Null(), Undefined()])),
  preview: Optional(Union([String(), Null(), Undefined()])),
  title: Optional(Union([String(), Null(), Undefined()])),
  kind: SessionKind,
  parentId: Optional(Union([String(), Null(), Undefined()])),
  agentName: Optional(Union([String(), Null(), Undefined()])),
});
export type SessionSummary = Static<typeof SessionSummary>;

export const SessionRow = Intersect([
  SessionSummary,
  Object({ settled: Boolean(), waitingAskuser: Boolean(), pinned: Boolean() }),
]);
export type SessionRow = Static<typeof SessionRow>;

export const Project = Object({
  cwd: String(),
  name: String(),
  sessionCount: Number(),
  lastActivity: Optional(Union([String(), Null(), Undefined()])),
});
export type Project = Static<typeof Project>;

export const UiPlugin = Object({
  id: String(),
  name: String(),
  description: String(),
  detected: Boolean(),
  sourceEnabled: Boolean(),
  enabled: Boolean(),
});
export type UiPlugin = Static<typeof UiPlugin>;

export const WorkspaceDiff = Object({
  branch: Union([String(), Null()]),
  files: Number(),
  patch: String(),
  truncated: Boolean(),
  untracked: Number(),
});
export type WorkspaceDiff = Static<typeof WorkspaceDiff>;

export const PullRequest = Object({
  number: Number(),
  url: String(),
  state: String(),
  title: String(),
  baseRefName: String(),
  headRefName: String(),
});
export type PullRequest = Static<typeof PullRequest>;

export const ShipPlan = Object({
  branch: Union([String(), Null()]),
  baseBranch: String(),
  suggestedBranch: String(),
  commitMessage: String(),
  pullRequestTitle: String(),
  pullRequestBody: String(),
  hasChanges: Boolean(),
  changedFiles: Number(),
  aheadCount: Number(),
  unpushedCount: Number(),
  behindCount: Number(),
  hasRemote: Boolean(),
  hasHead: Boolean(),
  pullRequest: Union([PullRequest, Null()]),
  steps: Array(Object({ id: String(), label: String(), required: Boolean() })),
  blockers: Array(String()),
  reviewToken: String(),
});
export type ShipPlan = Static<typeof ShipPlan>;

export const ShipResult = Object({
  branch: String(),
  commit: String(),
  pullRequest: PullRequest,
  settled: Boolean(),
});
export type ShipResult = Static<typeof ShipResult>;

export const Bootstrap = Object({
  seeded: Boolean(),
  sessionScope: Union([
    Null(),
    Object({
      mainSessionId: String(),
      childResolution: String(),
      maxChildren: Number(),
    }),
  ]),
  projects: Array(Project),
  sessions: Array(SessionRow),
  uiPlugins: Array(UiPlugin),
});
export type Bootstrap = Static<typeof Bootstrap>;

export const SessionPage = Object({
  total: Number(),
  offset: Number(),
  limit: Number(),
  sessions: Array(SessionRow),
});
export type SessionPage = Static<typeof SessionPage>;

export const ToolCall = Object({
  toolCallId: String(),
  name: String(),
  argsPreview: String(),
});
export type ToolCall = Static<typeof ToolCall>;

export const ThreadEntry = Union([
  Object({ kind: Literal("user"), text: String(), truncated: Boolean() }),
  Object({
    kind: Literal("assistant"),
    text: String(),
    truncated: Boolean(),
    thinking: Optional(Union([String(), Null(), Undefined()])),
    tools: Array(ToolCall),
    model: Optional(Union([String(), Null(), Undefined()])),
  }),
  Object({
    kind: Literal("askUser"),
    questions: Unknown(),
    answer: Optional(Unknown()),
  }),
  Object({
    kind: Literal("toolError"),
    text: String(),
    truncated: Boolean(),
  }),
]);
export type ThreadEntry = Static<typeof ThreadEntry>;

export const DatedEntry = Object({
  ts: Union([String(), Null()]),
  entry: ThreadEntry,
});
export type DatedEntry = Static<typeof DatedEntry>;

export const TaskItem = Object({
  id: String(),
  subject: String(),
  status: String(),
  description: Union([String(), Null()]),
  owner: Union([String(), Null()]),
  harness: Union([String(), Null()]),
  blockedBy: Array(String()),
});
export const TaskList = Object({ sessionId: String(), tasks: Array(TaskItem) });

export const WorkflowAgent = Object({
  label: Optional(Union([String(), Null(), Undefined()])),
  phase: Optional(Union([String(), Null(), Undefined()])),
  state: Optional(Union([String(), Null(), Undefined()])),
});
export const WorkflowRun = Object({
  runId: String(),
  sessionId: Optional(Union([String(), Null(), Undefined()])),
  name: Optional(Union([String(), Null(), Undefined()])),
  status: Optional(Union([String(), Null(), Undefined()])),
  agents: Array(WorkflowAgent),
});

export const ThreadView = Object({
  summary: SessionSummary,
  settled: Boolean(),
  waitingAskuser: Boolean(),
  omitted: Number(),
  entries: Array(DatedEntry),
  tasks: Array(TaskList),
  workflows: Array(WorkflowRun),
});
export type ThreadView = Static<typeof ThreadView>;

export const AgentModel = Object({
  id: String(),
  name: Optional(Union([String(), Null(), Undefined()])),
  provider: String(),
});
export type AgentModel = Static<typeof AgentModel>;

export const AgentSnapshot = Object({
  sessionId: String(),
  state: Object({
    isStreaming: Boolean(),
    thinkingLevel: Optional(Union([String(), Null(), Undefined()])),
    model: Optional(
      Union([
        Undefined(),
        Null(),
        Object({
          id: String(),
          provider: String(),
          displayName: Optional(Union([String(), Null(), Undefined()])),
          thinkingLevelMap: Optional(Union([Undefined(), Null(), Unknown()])),
        }),
      ]),
    ),
    pendingMessageCount: Optional(Union([Number(), Null(), Undefined()])),
  }),
  stats: Optional(
    Union([
      Undefined(),
      Null(),
      Object({
        contextUsage: Optional(
          Union([
            Undefined(),
            Null(),
            Object({
              percent: Union([Number(), Null()]),
              tokens: Union([Number(), Null()]),
              contextWindow: Union([Number(), Null()]),
            }),
          ]),
        ),
      }),
    ]),
  ),
  models: Optional(Union([Undefined(), Null(), Array(AgentModel)])),
});
export type AgentSnapshot = Static<typeof AgentSnapshot>;

/** One live dialog request (ask_user / confirm / input) awaiting an answer. */
export const PendingAsk = Object({
  id: String(),
  method: String(),
  title: Optional(Union([String(), Null(), Undefined()])),
  options: Optional(Union([Undefined(), Null(), Array(Unknown())])),
  recordedAtMs: Number(),
});
export type PendingAskWire = Static<typeof PendingAsk>;
