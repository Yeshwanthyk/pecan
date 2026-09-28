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
type SessionKind = Static<typeof SessionKind>;

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
  agentName: Optional(Union([String(), Null(), Undefined()])),
});
export type SessionSummary = Static<typeof SessionSummary>;

export const SessionRow = Intersect([
  SessionSummary,
  Object({
    settled: Boolean(),
    waitingAskuser: Boolean(),
    pinned: Boolean(),
    parentSessionId: Optional(Union([String(), Null(), Undefined()])),
  }),
]);
export type SessionRow = Static<typeof SessionRow>;

export const Project = Object({
  cwd: String(),
  name: String(),
  sessionCount: Number(),
  lastActivity: Optional(Union([String(), Null(), Undefined()])),
});
export type Project = Static<typeof Project>;

export const FolderPick = Object({
  home: Union([String(), Null()]),
  known: Array(
    Object({
      path: String(),
      name: String(),
      sessions: Number(),
      lastActivity: String(),
    }),
  ),
  entries: Array(Object({ path: String(), name: String() })),
});
export type FolderPick = Static<typeof FolderPick>;

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
type PullRequest = Static<typeof PullRequest>;

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
});
export type Bootstrap = Static<typeof Bootstrap>;

export const SessionPage = Object({
  total: Number(),
  offset: Number(),
  limit: Number(),
  sessions: Array(SessionRow),
});
type SessionPage = Static<typeof SessionPage>;

export const ToolCategory = Union([
  Literal("inspect"),
  Literal("change"),
  Literal("check"),
  Literal("research"),
  Literal("agent"),
  Literal("task"),
  Literal("runtime"),
  Literal("other"),
]);
type ToolCategory = Static<typeof ToolCategory>;

export const ToolCall = Object({
  toolCallId: String(),
  name: String(),
  argsPreview: String(),
  summary: String(),
  category: ToolCategory,
  targets: Array(String()),
  targetCount: Number(),
  details: Optional(Unknown()),
});
export type ToolCall = Static<typeof ToolCall>;

export const UserImage = Object({
  mimeType: String(),
  data: String(),
});
type UserImage = Static<typeof UserImage>;

export const ThreadEntry = Union([
  Object({
    kind: Literal("user"),
    text: String(),
    truncated: Boolean(),
    images: Array(UserImage),
  }),
  Object({
    kind: Literal("assistant"),
    text: String(),
    truncated: Boolean(),
    thinking: Optional(Union([String(), Null(), Undefined()])),
    tools: Array(ToolCall),
    model: Optional(Union([String(), Null(), Undefined()])),
    error: Optional(String()),
  }),
  Object({
    kind: Literal("askUser"),
    questions: Unknown(),
    answer: Optional(Unknown()),
  }),
  Object({
    kind: Literal("childQuestions"),
    questions: Array(
      Object({
        childId: String(),
        requestId: String(),
        question: String(),
        context: Optional(Union([String(), Null(), Undefined()])),
        deadlineAt: Optional(Union([Number(), Null(), Undefined()])),
        answered: Boolean(),
      }),
    ),
  }),
  Object({
    kind: Literal("childResults"),
    text: String(),
    truncated: Boolean(),
    results: Array(Object({ id: String(), title: String(), status: String() })),
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
  activeForm: Union([String(), Null()]),
  owner: Union([String(), Null()]),
  harness: Union([String(), Null()]),
  blockedBy: Array(String()),
  blocks: Array(String()),
  execution: Union([Unknown(), Null()]),
  updatedAt: Union([Number(), Null()]),
});
export const TaskList = Object({ sessionId: String(), tasks: Array(TaskItem) });
export type TaskItem = Static<typeof TaskItem>;
export type TaskGroup = Static<typeof TaskList>;

export const WorkflowTask = Object({
  id: String(),
  label: String(),
  kind: Optional(Union([String(), Null(), Undefined()])),
  needs: Array(String()),
  status: String(),
  attempt: Number(),
  childId: Optional(Union([String(), Null(), Undefined()])),
  startedAt: Optional(Union([Number(), Null(), Undefined()])),
  finishedAt: Optional(Union([Number(), Null(), Undefined()])),
  result: Optional(Union([String(), Null(), Undefined()])),
  error: Optional(Union([String(), Null(), Undefined()])),
  sessionId: Optional(Union([String(), Null(), Undefined()])),
});
export type WorkflowTask = Static<typeof WorkflowTask>;
export const WorkflowRun = Object({
  runId: String(),
  name: Optional(Union([String(), Null(), Undefined()])),
  description: Optional(Union([String(), Null(), Undefined()])),
  status: String(),
  createdAt: Optional(Union([Number(), Null(), Undefined()])),
  startedAt: Optional(Union([Number(), Null(), Undefined()])),
  finishedAt: Optional(Union([Number(), Null(), Undefined()])),
  lastActivityAt: Optional(Union([Number(), Null(), Undefined()])),
  outcome: Optional(Union([String(), Null(), Undefined()])),
  tasks: Array(WorkflowTask),
});
export type WorkflowRun = Static<typeof WorkflowRun>;

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
type AgentModel = Static<typeof AgentModel>;

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
  favorites: Optional(Array(String())),
});
export type AgentSnapshot = Static<typeof AgentSnapshot>;

/** One live extension dialog (select / confirm / input / editor) awaiting an
 * answer. `method`, `options` and the extra text fields mirror the pi
 * `extension_ui_request` payload the server recorded. */
export const PendingAsk = Object({
  id: String(),
  method: String(),
  title: Optional(Union([String(), Null(), Undefined()])),
  message: Optional(Union([String(), Null(), Undefined()])),
  placeholder: Optional(Union([String(), Null(), Undefined()])),
  prefill: Optional(Union([String(), Null(), Undefined()])),
  options: Optional(Union([Undefined(), Null(), Array(Unknown())])),
  recordedAtMs: Number(),
});

export const Health = Object({
  status: Union([Literal("ok"), Literal("missing"), Literal("error")]),
  version: Optional(Union([String(), Null()])),
  detail: Optional(Union([String(), Null()])),
  agentDir: Boolean(),
  authFile: Boolean(),
});
export type Health = Static<typeof Health>;

/** A browser or phone allowed to use this server. */
export const Device = Object({
  id: String(),
  name: String(),
  createdAtMs: Number(),
  lastSeenAtMs: Number(),
});

/** `POST /api/pair` success. */
export const Paired = Object({ device: Device });
