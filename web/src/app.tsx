/** App shell: docked sidebar on desktop, Sheet sidebar on mobile, thread pane. */
import {
  CheckIcon,
  ChevronRightIcon,
  CircleAlertIcon,
  FileDiffIcon,
  GitBranchIcon,
  MoreHorizontalIcon,
  PackageCheckIcon,
} from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";

import { api, captureShipToken } from "~/api/client";
import type { ThreadView } from "~/api/types";
import { AppSidebar, useHash } from "~/components/app-sidebar";
import { Composer } from "~/components/composer";
import { ExtensionHost } from "~/components/extension-host";
import { ExtensionChrome } from "~/components/extension-chrome";
import { ActiveTaskStatus } from "~/components/extension-ui";
import { SettingsPage } from "~/components/settings-page";
import { Thread, threadChangeCount } from "~/components/thread";
import { SubagentStrip } from "~/components/subagent-strip";
import { Button } from "~/components/ui/button";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "~/components/ui/sidebar";
import { connectEvents, syncPendingAsks } from "~/events";
import { useApp } from "~/store";

const DiffSidebar = lazy(() => import("~/components/diff-sidebar"));
const ShipSidebar = lazy(() => import("~/components/ship-sidebar"));

export function App() {
  const [diffOpen, setDiffOpen] = useState(false);
  const [shipOpen, setShipOpen] = useState(false);
  const thread = useApp((state) => state.thread);
  const sessions = useApp((state) => state.sessions);
  const sessionScope = useApp((state) => state.bootstrap?.sessionScope ?? null);
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
          useApp.getState().setBootstrap(data);
          useApp.getState().setSessions(data.sessions);
        })
        .catch(() => undefined);
    };
    window.addEventListener("pecan:refresh", load);
    load();
    connectEvents();
    return () => window.removeEventListener("pecan:refresh", load);
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

  return (
    <SidebarProvider className="h-svh overflow-hidden">
      {sessionScope ? null : <AppSidebar />}
      {sessionScope ? null : <MobileSheetCloser />}
      <SidebarInset>
        <header className="flex min-h-14 shrink-0 items-center gap-2 border-b border-border/60 px-2.5 py-1.5">
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
            <span className="text-sm font-semibold tracking-tight">pecan</span>
          )}
        </header>
        {isSettings ? (
          <SettingsPage />
        ) : activeThread ? (
          <>
            <Thread key={activeThread.summary.id} data={activeThread} />
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
          <h1 className="min-w-0 truncate text-sm font-semibold tracking-[-0.01em]" title={title}>
            {isSubagent
              ? (data.summary.agentName ?? title).replace(/^subagents:\s*/, "")
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
        variant="default"
      >
        <PackageCheckIcon />
        <span className="hidden sm:inline">Ship</span>
      </Button>
    </div>
  );
}

function EmptyState() {
  const loading = useApp((state) => state.bootstrap === null);
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-1 px-6 text-center text-muted-foreground">
      <p className="text-sm font-medium">
        {loading ? "Loading…" : "Select a session"}
      </p>
      <p className="text-xs">
        Move completed threads to Settled; reopen them any time.
      </p>
    </div>
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
  try {
    return {
      sessionId: decodeURIComponent(encodedSessionId),
      ...(parentId ? { parentId } : {}),
    };
  } catch {
    return null;
  }
}

function reportError(message: string) {
  window.dispatchEvent(new CustomEvent("pecan:error", { detail: message }));
}

function projectName(cwd: string) {
  return cwd.split("/").findLast((segment) => segment.length > 0) ?? cwd;
}

function shortModel(model: string) {
  return model.split("/").at(-1) ?? model;
}

function threadTargetLabel(thread: ThreadView) {
  if (thread.summary.kind !== "subagent") return "main thread";
  return (thread.summary.agentName ?? "subagent").replace(/^subagents:\s*/, "");
}
