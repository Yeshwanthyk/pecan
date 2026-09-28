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
  RotateCcwIcon,
  SparklesIcon,
} from "lucide-react";
import { lazy, type ReactNode, Suspense, useEffect, useState } from "react";

import {
  api,
  captureShipToken,
  takePairCode,
  UNPAIRED_EVENT,
} from "~/api/client";
import type { ThreadView } from "~/api/types";
import { AppSidebar, BrandMark, useHash } from "~/components/app-sidebar";
import { Composer } from "~/components/composer";
import { ErrorBoundary } from "~/components/error-boundary";
import { HealthBanner } from "~/components/health-banner";
import { Home } from "~/components/home";
import { PairScreen } from "~/components/pair-screen";
import { ExtensionHost } from "~/components/extension-host";
import { ExtensionChrome } from "~/components/extension-chrome";
import { ActiveTaskStatus } from "~/components/extension-ui";
import { SettingsPage } from "~/components/settings-page";
import { Thread, threadChangeCount } from "~/components/thread";
import { SubagentStrip } from "~/components/subagent-strip";
import { Toaster } from "~/components/toaster";
import { Button } from "~/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "~/components/ui/popover";
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
    if (code)
      void api
        .pair(code)
        .catch(() => undefined)
        .finally(start);
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
      (route.sessionId === sessionScope.mainSessionId ||
        route.parentId === sessionScope.mainSessionId)
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
    return (
      <PairScreen
        onPaired={() => window.dispatchEvent(new CustomEvent("pecan:refresh"))}
      />
    );
  }

  return (
    <SidebarProvider className="h-svh overflow-hidden">
      {sessionScope ? null : <AppSidebar />}
      {sessionScope ? null : <MobileSheetCloser />}
      <SidebarInset>
        <header className="flex h-14 shrink-0 items-center gap-2 px-2.5">
          {sessionScope ? null : <HeaderSidebarTrigger />}
          {isSettings ? (
            <span className="text-sm font-semibold tracking-tight">
              Settings
            </span>
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
          <Home />
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
  const title =
    data.summary.title ?? data.summary.preview ?? data.summary.id.slice(0, 8);
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
              <ChevronRightIcon
                aria-hidden
                className="hidden size-3.5 shrink-0 text-border sm:block"
              />
            </>
          ) : null}
          <h1
            className="min-w-0 truncate text-[13px] font-medium text-foreground/90"
            title={title}
          >
            {isSubagent ? agentTitle(data.summary.agentName ?? title) : title}
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
              <span aria-hidden className="text-border">
                /
              </span>
              <span className="flex min-w-0 items-center gap-1 font-mono">
                <GitBranchIcon className="size-3 shrink-0 opacity-60" />
                <span className="truncate">{branch}</span>
              </span>
            </>
          ) : null}
          {data.summary.model ? (
            <>
              <span aria-hidden className="hidden text-border md:inline">
                /
              </span>
              <span className="hidden shrink-0 md:inline">
                {shortModel(data.summary.model)}
              </span>
            </>
          ) : null}
        </div>
        <ActiveTaskStatus groups={data.tasks} />
      </div>
      <Button
        aria-label="Open workspace diff"
        className="min-w-8 gap-1 px-2"
        onClick={onOpenDiff}
        size="sm"
        title="Review all local workspace changes"
        variant="ghost-muted"
      >
        <FileDiffIcon />
        {changes > 0 ? (
          <span className="tabular-nums text-[11px]">{changes}</span>
        ) : null}
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
      <ThreadMenu
        busy={settling}
        data={data}
        embedded={embedded}
        onToggleSettled={() => void toggleSettled()}
      />
    </div>
  );
}

/** Less frequent thread actions, folded out of the header. */
function ThreadMenu({
  data,
  embedded,
  busy,
  onToggleSettled,
}: {
  data: ThreadView;
  embedded: boolean;
  busy: boolean;
  onToggleSettled: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [titling, setTitling] = useState(false);
  const cwd = data.summary.cwd;
  const starting = useApp((state) => state.startingCwd === cwd);

  function run(action: () => void) {
    setOpen(false);
    action();
  }

  async function regenerateTitle() {
    if (titling) return;
    setTitling(true);
    try {
      await api.regenerateTitle(data.summary.id);
      const updated = await api.thread(data.summary.id);
      if (useApp.getState().thread?.summary.id === data.summary.id) {
        useApp.getState().setThread(updated);
      }
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setTitling(false);
    }
  }

  return (
    <Popover onOpenChange={setOpen} open={open}>
      <PopoverTrigger
        aria-label="Thread actions"
        className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-[popup-open]:bg-accent data-[popup-open]:text-foreground"
        data-testid="thread-menu"
      >
        {busy || titling || starting ? (
          <Spinner className="size-4" />
        ) : (
          <MoreHorizontalIcon className="size-4" />
        )}
      </PopoverTrigger>
      <PopoverContent align="end" className="w-60 p-1" side="bottom">
        {embedded ? null : (
          <MenuItem
            data-testid="header-new-session"
            disabled={starting}
            hint="Fresh session in this project"
            icon={<PlusIcon />}
            label="New session"
            onClick={() => run(() => void startSession(cwd))}
          />
        )}
        <MenuItem
          disabled={titling}
          hint="Uses a few tokens"
          icon={<SparklesIcon />}
          label={data.summary.title ? "Regenerate title" : "Generate title"}
          onClick={() => run(() => void regenerateTitle())}
        />
        <div className="my-1 h-px bg-border" />
        <MenuItem
          disabled={busy}
          hint={
            data.settled
              ? "Move back to active threads"
              : "Archive in Pecan; Git is untouched"
          }
          icon={data.settled ? <RotateCcwIcon /> : <CheckIcon />}
          label={data.settled ? "Reopen" : "Mark done"}
          onClick={() => run(onToggleSettled)}
        />
      </PopoverContent>
    </Popover>
  );
}

function MenuItem({
  icon,
  label,
  hint,
  onClick,
  disabled,
  ...rest
}: {
  icon: ReactNode;
  label: string;
  hint: string;
  onClick: () => void;
  disabled?: boolean;
  "data-testid"?: string;
}) {
  return (
    <button
      className="flex min-h-11 w-full items-center gap-2.5 rounded-md px-2 text-left outline-none transition-colors hover:bg-accent focus-visible:bg-accent disabled:opacity-50 md:min-h-9 [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-muted-foreground"
      disabled={disabled}
      onClick={onClick}
      type="button"
      {...rest}
    >
      {icon}
      <span className="min-w-0 flex-1">
        <span className="block text-[13px] leading-5">{label}</span>
        <span className="block truncate text-[11px] leading-4 text-muted-foreground">
          {hint}
        </span>
      </span>
    </button>
  );
}

/** The sidebar owns its toggle while open on desktop; the header shows it otherwise. */
function HeaderSidebarTrigger() {
  const { isMobile, open } = useSidebar();
  if (!isMobile && open) return null;
  return <SidebarTrigger className="animate-toast-in" />;
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
