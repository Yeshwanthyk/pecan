/** App shell: docked sidebar on desktop, Sheet sidebar on mobile, thread pane. */
import {
  CheckIcon,
  ChevronRightIcon,
  CircleAlertIcon,
  FileDiffIcon,
  GitBranchIcon,
  MoreHorizontalIcon,
  PackageCheckIcon,
  PlusIcon,
} from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";

import { api, captureShipToken, takePairCode, UNPAIRED_EVENT } from "~/api/client";
import type { ThreadView } from "~/api/types";
import { AppSidebar, BrandMark, useHash } from "~/components/app-sidebar";
import { AddProjectForm } from "~/components/projects-group";
import { Composer } from "~/components/composer";
import { ErrorBoundary } from "~/components/error-boundary";
import { HealthBanner } from "~/components/health-banner";
import { PairScreen } from "~/components/pair-screen";
import { ExtensionHost } from "~/components/extension-host";
import { ExtensionChrome } from "~/components/extension-chrome";
import { ActiveTaskStatus } from "~/components/extension-ui";
import { SettingsPage } from "~/components/settings-page";
import { Thread, threadChangeCount } from "~/components/thread";
import { SubagentStrip } from "~/components/subagent-strip";
import { Toaster } from "~/components/toaster";
import { Button } from "~/components/ui/button";
import { Spinner } from "~/components/ui/spinner";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "~/components/ui/sidebar";
import { connectEvents, resumeEvents, syncPendingAsks } from "~/events";
import { reportError } from "~/lib/errors";
import { agentTitle, projectName, shortModel } from "~/lib/format";
import { startSession } from "~/lib/sessions";
import { useApp } from "~/store";

const DiffSidebar = lazy(() => import("~/components/diff-sidebar"));
const ShipSidebar = lazy(() => import("~/components/ship-sidebar"));

export function App() {
  const [diffOpen, setDiffOpen] = useState(false);
  const [shipOpen, setShipOpen] = useState(false);
  const thread = useApp((state) => state.thread);
  const sessions = useApp((state) => state.sessions);
  const sessionScope = useApp((state) => state.bootstrap?.sessionScope ?? null);
  const paired = useApp((state) => state.paired);
  const hash = useHash();
  const route = parseSessionRoute(hash);
  const isSettings = sessionScope === null && hash === "#/settings";
  const activeThread = route?.sessionId === thread?.summary.id ? thread : null;
  const routeParent = route?.parentId
    ? sessions.find((session) => session.id === route.parentId)
    : undefined;

  // Initial data load + SSE wiring + manual refresh requests.
  useEffect(() => {
    captureShipToken();
    const load = () => {
      void api
        .bootstrap()
        .then((data) => {
          useApp.getState().setPaired(true);
          useApp.getState().setBootstrap(data);
          useApp.getState().setSessions(data.sessions);
          resumeEvents();
        })
        .catch(() => undefined);
    };
    const unpaired = () => useApp.getState().setPaired(false);
    window.addEventListener("pecan:refresh", load);
    window.addEventListener(UNPAIRED_EVENT, unpaired);
    // A `?pair=` link from `pecan pair` pairs before the first load; a bad
    // code just falls through to the pair screen.
    const code = takePairCode();
    const start = () => {
      load();
      connectEvents();
    };
    if (code) void api.pair(code).catch(() => undefined).finally(start);
    else start();
    return () => {
      window.removeEventListener("pecan:refresh", load);
      window.removeEventListener(UNPAIRED_EVENT, unpaired);
    };
  }, []);

  // Embedded servers own navigation: only the requested main session and its
  // bounded child set may be opened, including after reload or a pasted hash.
  useEffect(() => {
    if (!sessionScope) return;
    const allowed = new Set(sessions.map((session) => session.id));
    if (
      route &&
      allowed.has(route.sessionId) &&
      (route.sessionId === sessionScope.mainSessionId || route.parentId === sessionScope.mainSessionId)
    ) {
      return;
    }
    location.hash = `#/s/${encodeURIComponent(sessionScope.mainSessionId)}`;
  }, [route, sessionScope, sessions]);

  // The optional parent query keeps child navigation exact across reloads.
  useEffect(() => {
    const apply = () => {
      const nextRoute = parseSessionRoute(location.hash);
      if (nextRoute) void openThread(nextRoute.sessionId);
      else useApp.getState().setThread(null);
    };
    apply();
    window.addEventListener("hashchange", apply);
    return () => window.removeEventListener("hashchange", apply);
  }, []);

  if (!paired) {
    return <PairScreen onPaired={() => window.dispatchEvent(new CustomEvent("pecan:refresh"))} />;
  }

  return (
    <SidebarProvider className="h-svh overflow-hidden">
      {sessionScope ? null : <AppSidebar />}
      {sessionScope ? null : <MobileSheetCloser />}
      <SidebarInset>
        <header className="flex min-h-12 shrink-0 items-center gap-2 px-2.5 py-1.5">
          {sessionScope ? null : <SidebarTrigger />}
          {isSettings ? (
            <span className="text-sm font-semibold tracking-tight">Settings</span>
          ) : activeThread ? (
            <HeaderContext
              data={activeThread}
              embedded={sessionScope !== null}
              onOpenDiff={() => setDiffOpen(true)}
              onOpenShip={() => setShipOpen(true)}
              parentThreadId={route?.parentId}
            />
          ) : (
            <span className="flex items-center gap-2 text-sm font-semibold tracking-tight md:hidden">
              <BrandMark className="size-5 rounded-md text-[11px]" />
              Pecan
            </span>
          )}
        </header>
        <HealthBanner />
        {isSettings ? (
          <SettingsPage />
        ) : activeThread ? (
          <>
            <ErrorBoundary
              key={activeThread.summary.id}
              label="This thread"
              resetKey={activeThread.summary.id}
            >
              <Thread data={activeThread} />
            </ErrorBoundary>
            <SubagentStrip
              current={activeThread.summary}
              parent={routeParent}
              parentId={route?.parentId ?? activeThread.summary.id}
            />
            <ExtensionChrome
              placement="aboveEditor"
              sessionId={activeThread.summary.id}
            />
            <ExtensionHost
              sessionId={activeThread.summary.id}
              settled={activeThread.settled}
            />
            <Composer
              key={activeThread.summary.id}
              sessionId={activeThread.summary.id}
              targetLabel={threadTargetLabel(activeThread)}
            />
            <ExtensionChrome
              placement="belowEditor"
              sessionId={activeThread.summary.id}
            />
            <Suspense fallback={null}>
              <DiffSidebar
                key={activeThread.summary.id}
                onOpenChange={setDiffOpen}
                open={diffOpen}
                sessionId={activeThread.summary.id}
              />
            </Suspense>
            <Suspense fallback={null}>
              <ShipSidebar
                key={activeThread.summary.id}
                onOpenChange={setShipOpen}
                onReviewDiff={() => {
                  setShipOpen(false);
                  setDiffOpen(true);
                }}
                open={shipOpen}
                sessionId={activeThread.summary.id}
              />
            </Suspense>
          </>
        ) : (
          <EmptyState />
        )}
      </SidebarInset>
      <Toaster />
    </SidebarProvider>
  );
}

/** Persistent thread controls: identity, useful context, changes, and completion. */
function HeaderContext({
  data,
  embedded,
  onOpenDiff,
  onOpenShip,
  parentThreadId,
}: {
  data: ThreadView;
  embedded: boolean;
  onOpenDiff: () => void;
  onOpenShip: () => void;
  parentThreadId?: string;
}) {
  const [branch, setBranch] = useState<string | null>(null);
  const [settling, setSettling] = useState(false);
  const sessions = useApp((state) => state.sessions);

  useEffect(() => {
    let cancelled = false;
    setBranch(null);
    void api
      .gitBranch(data.summary.id)
      .then((res) => {
        if (!cancelled) setBranch(res.branch ?? null);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [data.summary.id]);

  // Subagents show a parent breadcrumb: Parent › agent-name.
  const isSubagent = data.summary.kind === "subagent";
  const parent = isSubagent
    ? sessions.find((candidate) => candidate.id === parentThreadId)
    : undefined;
  const parentRouteId = parentThreadId ?? "";
  const hasParentContext = isSubagent && parentRouteId.length > 0;

  const project = projectName(data.summary.cwd);
  const title = data.summary.title ?? data.summary.preview ?? data.summary.id.slice(0, 8);
  const changes = threadChangeCount(data.entries);

  async function toggleSettled() {
    if (settling) return;
    setSettling(true);
    try {
      if (data.settled) await api.reopen(data.summary.id);
      else await api.settle(data.summary.id);
      const updated = await api.thread(data.summary.id);
      if (useApp.getState().thread?.summary.id === data.summary.id) {
        useApp.getState().setThread(updated);
      }
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setSettling(false);
    }
  }

  return (
    <div className="flex min-w-0 flex-1 items-center gap-2.5">
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5">
          {hasParentContext ? (
            <>
              <a
                className="hidden max-w-[32%] shrink truncate text-sm font-medium text-muted-foreground hover:text-foreground sm:block"
                href={`#/s/${parentRouteId}`}
                title={parent?.title ?? parent?.preview ?? "Parent thread"}
              >
                {parent?.title ?? parent?.preview ?? "Parent"}
              </a>
              <ChevronRightIcon aria-hidden className="hidden size-3.5 shrink-0 text-border sm:block" />
            </>
          ) : null}
          <h1 className="min-w-0 truncate text-[13px] font-medium text-foreground/90" title={title}>
            {isSubagent
              ? agentTitle(data.summary.agentName ?? title)
              : title}
          </h1>
          {data.waitingAskuser && !data.settled ? (
            <span
              className="flex shrink-0 items-center gap-1 text-[11px] font-medium text-warning"
              title="Waiting for your answer"
            >
              <CircleAlertIcon className="size-3.5" />
              <span className="hidden lg:inline">Needs reply</span>
            </span>
          ) : null}
        </div>
        <div className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[11px] leading-4 text-muted-foreground">
          {embedded ? (
            <span className="truncate">{project}</span>
          ) : (
            <a
              className="truncate hover:text-foreground"
              href={`#/p/${encodeURIComponent(data.summary.cwd)}`}
              title={data.summary.cwd}
            >
              {project}
            </a>
          )}
          {branch ? (
            <>
              <span aria-hidden className="text-border">/</span>
              <span className="flex min-w-0 items-center gap-1 font-mono">
                <GitBranchIcon className="size-3 shrink-0 opacity-60" />
                <span className="truncate">{branch}</span>
              </span>
            </>
          ) : null}
          {data.summary.model ? (
            <>
              <span aria-hidden className="hidden text-border md:inline">/</span>
              <span className="hidden shrink-0 md:inline">{shortModel(data.summary.model)}</span>
            </>
          ) : null}
        </div>
        <ActiveTaskStatus groups={data.tasks} />
      </div>
      {embedded ? null : <NewInProjectButton cwd={data.summary.cwd} />}
      <Button
        aria-label="Open workspace diff"
        className="min-w-8 px-2 sm:px-2.5"
        onClick={onOpenDiff}
        size="sm"
        title="Review all local workspace changes"
        variant="ghost-muted"
      >
        <FileDiffIcon />
        <span className="hidden sm:inline">Diff</span>
        {changes > 0 ? (
          <span className="tabular-nums text-[11px] opacity-70">{changes}</span>
        ) : null}
      </Button>
      <Button
        aria-label={data.settled ? "Reopen settled thread" : "Mark thread done in Pecan"}
        className="min-w-8 px-2"
        disabled={settling}
        onClick={() => void toggleSettled()}
        size="sm"
        title={data.settled ? "Move back to active threads" : "Mark done without changing Git"}
        variant="ghost-muted"
      >
        {settling ? <MoreHorizontalIcon className="animate-pulse" /> : <CheckIcon />}
        <span className="hidden lg:inline">{data.settled ? "Reopen" : "Done"}</span>
      </Button>
      <Button
        aria-label="Review and ship with Git"
        className="min-w-8 px-2 sm:px-2.5"
        onClick={onOpenShip}
        size="sm"
        title="Review, commit all changes, push, and create a pull request"
        variant="outline"
      >
        <PackageCheckIcon />
        <span className="hidden sm:inline">Ship</span>
      </Button>
    </div>
  );
}

/** Starts a fresh session in the current thread's project. */
function NewInProjectButton({ cwd }: { cwd: string }) {
  const starting = useApp((state) => state.startingCwd === cwd);
  return (
    <Button
      aria-label="New session in this project"
      className="min-w-8 px-2"
      data-testid="header-new-session"
      disabled={starting}
      onClick={() => void startSession(cwd)}
      size="sm"
      title="New session in this project"
      variant="ghost-muted"
    >
      {starting ? <Spinner className="size-4" /> : <PlusIcon />}
      <span className="hidden lg:inline">New</span>
    </Button>
  );
}

const RECENT_ON_HOME = 5;

function EmptyState() {
  const bootstrap = useApp((state) => state.bootstrap);
  const sessions = useApp((state) => state.sessions);
  const startingCwd = useApp((state) => state.startingCwd);
  if (bootstrap === null) {
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <Spinner className="size-5" />
      </div>
    );
  }
  const projects = bootstrap.projects;
  const linked = new Set(projects.map((project) => project.cwd));
  const recent = sessions
    .filter((row) => row.kind !== "subagent" && !row.settled && linked.has(row.cwd))
    .sort((a, b) => Date.parse(b.lastActivity) - Date.parse(a.lastActivity))
    .slice(0, RECENT_ON_HOME);

  return (
    <div className="flex-1 overflow-y-auto" data-testid="empty-state">
      <div className="mx-auto flex w-full max-w-xl flex-col px-5 pt-[10vh] pb-16 sm:pt-[14vh]">
        <BrandMark className="size-9 rounded-[10px] text-lg" />
        <h1 className="mt-5 text-xl font-semibold tracking-[-0.015em]">
          {projects.length === 0 ? "Welcome to Pecan" : "What should Pi work on?"}
        </h1>
        <p className="mt-1 text-sm leading-6 text-muted-foreground">
          {projects.length === 0
            ? "Add a project folder, then start Pi sessions in it from any device."
            : "Start a new session in a project, or pick up where you left off."}
        </p>

        {projects.length === 0 ? (
          <div className="mt-7 rounded-2xl border bg-card p-4 shadow-xs">
            <AddProjectForm autoFocus={false} />
          </div>
        ) : (
          <>
            <SectionHeading>New session</SectionHeading>
            <ul className="grid gap-2 sm:grid-cols-2">
              {projects.map((project) => (
                <li className="min-w-0" key={project.cwd}>
                  <button
                    className="group flex min-h-16 w-full items-center gap-3 rounded-2xl border bg-card px-3.5 py-3 text-left shadow-xs outline-none transition hover:border-input hover:shadow-sm focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
                    data-testid="empty-new-session"
                    disabled={startingCwd !== null}
                    onClick={() => void startSession(project.cwd)}
                    title={project.cwd}
                    type="button"
                  >
                    <span className="flex size-9 shrink-0 items-center justify-center rounded-xl bg-muted text-muted-foreground transition-colors group-hover:bg-primary group-hover:text-primary-foreground">
                      {startingCwd === project.cwd ? (
                        <Spinner className="size-4" />
                      ) : (
                        <PlusIcon className="size-4" />
                      )}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[15px] font-medium">{project.name}</span>
                      <span className="block truncate text-xs text-muted-foreground" dir="rtl">
                        <bdi>{project.cwd}</bdi>
                      </span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>

            {recent.length > 0 ? (
              <>
                <SectionHeading>Recent</SectionHeading>
                <ul className="overflow-hidden rounded-2xl border bg-card shadow-xs">
                  {recent.map((row) => (
                    <li className="border-b last:border-b-0" key={row.id}>
                      <a
                        className="flex min-h-14 items-center gap-3 px-4 py-2.5 outline-none transition-colors hover:bg-accent focus-visible:bg-accent"
                        href={`#/s/${row.id}`}
                      >
                        <span
                          aria-hidden
                          className={
                            row.waitingAskuser
                              ? "size-2 shrink-0 rounded-full bg-warning"
                              : "size-2 shrink-0 rounded-full bg-border"
                          }
                        />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm font-medium">
                            {row.title ?? row.preview ?? row.id.slice(0, 8)}
                          </span>
                          <span className="block truncate text-xs text-muted-foreground">
                            {projectName(row.cwd)}
                            {row.waitingAskuser ? " · needs your answer" : ""}
                          </span>
                        </span>
                        <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground/60" />
                      </a>
                    </li>
                  ))}
                </ul>
              </>
            ) : null}
          </>
        )}
      </div>
    </div>
  );
}

function SectionHeading({ children }: { children: string }) {
  return (
    <h2 className="mt-8 mb-2 px-1 text-xs text-muted-foreground">
      {children}
    </h2>
  );
}

/** Closes the mobile sidebar sheet whenever in-sidebar navigation happens. */
function MobileSheetCloser() {
  const { isMobile, setOpenMobile } = useSidebar();
  useEffect(() => {
    const close = () => {
      if (isMobile) setOpenMobile(false);
    };
    window.addEventListener("pecan:navigated", close);
    return () => window.removeEventListener("pecan:navigated", close);
  }, [isMobile, setOpenMobile]);
  return null;
}

async function openThread(id: string) {
  try {
    const data = await api.thread(id);
    if (parseSessionRoute(location.hash)?.sessionId !== id) return;
    useApp.getState().setThread(data);
    void syncPendingAsks(id);
  } catch (error) {
    if (parseSessionRoute(location.hash)?.sessionId !== id) return;
    reportError(error instanceof Error ? error.message : String(error));
  }
}

type SessionRoute = {
  sessionId: string;
  parentId?: string;
};

function parseSessionRoute(hash: string): SessionRoute | null {
  if (!hash.startsWith("#/s/")) return null;
  const [encodedSessionId, query = ""] = hash.slice(4).split("?", 2);
  if (!encodedSessionId) return null;
  const parentId = new URLSearchParams(query).get("parent")?.trim();
  let sessionId: string | null = null;
  try {
    sessionId = decodeURIComponent(encodedSessionId);
  } catch {
    // Malformed percent-encoding in the route; treat as no matching session.
  }
  if (sessionId === null) return null;
  return {
    sessionId,
    ...(parentId ? { parentId } : {}),
  };
}

function threadTargetLabel(thread: ThreadView) {
  if (thread.summary.kind !== "subagent") return "main thread";
  return agentTitle(thread.summary.agentName ?? "subagent");
}
