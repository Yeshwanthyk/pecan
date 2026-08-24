/**
 * Grouped app sidebar: brand header, Projects group, recent Sessions group,
 * settled shelf, footer. Docked on desktop, Sheet on mobile (via `Sidebar`).
 * Session rows are capped — the full index can hold thousands.
 */
import {
  ChevronRightIcon,
  SettingsIcon,
  LoaderCircleIcon,
  PinIcon,
  PinOffIcon,
  PlusIcon,
  RefreshCwIcon,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useState,
  type ComponentProps,
  type ReactNode,
} from "react";

import { api, readAiTitleGenerationEnabled } from "~/api/client";
import type { SessionRow as SessionRowData } from "~/api/types";
import { Button } from "~/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "~/components/ui/popover";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "~/components/ui/sidebar";
import { ProjectsGroup } from "~/components/projects-group";
import { useApp } from "~/store";
import { cn } from "~/lib/utils";

const MAX_ACTIVE_ROWS = 30;
const MAX_SETTLED_ROWS = 50;

export function AppSidebar() {
  return (
    <Sidebar collapsible="offcanvas">
      <SidebarHeader className="min-h-12 flex-row items-center border-b border-sidebar-border px-3 py-2">
        <span className="text-sm font-semibold tracking-[-0.01em]">Pecan</span>
        <ConnectionStatus />
      </SidebarHeader>
      <SidebarContent className="gap-0">
        <ProjectsGroup />
        <SessionsGroup />
      </SidebarContent>
      <SidebarFooter className="border-t border-sidebar-border p-2">
        <SettingsLink />
      </SidebarFooter>
    </Sidebar>
  );
}

function ConnectionStatus() {
  const connected = useApp((state) => state.connected);
  return (
    <span
      className="ml-auto flex items-center gap-1.5 text-[11px] font-medium text-sidebar-muted-foreground"
      title={connected ? "Live updates connected" : "Reconnecting…"}
    >
      <span
        aria-hidden
        className={cn(
          "size-1.5 rounded-full",
          connected ? "bg-success" : "animate-pulse bg-warning",
        )}
      />
      {connected ? "Live" : "Reconnecting"}
    </span>
  );
}

function SettingsLink() {
  const active = useHash() === "#/settings";
  return (
    <SidebarMenuButton
      render={<NavHashLink href="#/settings" />}
      isActive={active}
      tooltip="Settings"
      className="min-h-11 w-full px-2.5 md:min-h-8"
    >
      <SettingsIcon />
      <span>Settings</span>
    </SidebarMenuButton>
  );
}

function SessionsGroup() {
  const sessions = useApp((state) => state.sessions);
  const bootstrap = useApp((state) => state.bootstrap);
  const activeRoute = useActiveSessionRoute();
  const activeId = activeRoute?.sessionId ?? null;
  const activeProject = useActiveProjectPath();

  // Only sessions belonging to projects the user explicitly linked are
  // tracked, matching pican's curated-sidebar model.
  const addedCwds = useMemo(
    () => new Set(bootstrap?.projects.map((project) => project.cwd)),
    [bootstrap],
  );

  const { pinned, active, settled } = useMemo(() => {
    const scoped = sessions.filter(
      (row) => addedCwds.has(row.cwd) || row.kind === "subagent",
    );
    const byRecency = [...scoped].sort(
      (a, b) => Date.parse(b.lastActivity) - Date.parse(a.lastActivity),
    );
    return {
      // Child threads are navigated from their parent's bottom agent rail,
      // where the exact parent route is available.
      pinned: byRecency.filter(
        (row) => row.kind !== "subagent" && !row.settled && row.pinned,
      ),
      active: byRecency.filter((row) => row.kind !== "subagent" && !row.settled),
      settled: byRecency.filter((row) => row.kind !== "subagent" && row.settled),
    };
  }, [sessions, addedCwds]);

  const visible = activeProject
    ? active.filter((row) => row.cwd === activeProject)
    : active.filter((row) => !row.pinned);

  return (
    <SidebarGroup className="flex-1 border-t border-sidebar-border pt-1.5">
      <div className="flex min-h-9 items-center px-2">
        <SidebarGroupLabel className="h-auto flex-1 px-0 text-xs font-semibold">
          {activeProject ? projectName(activeProject) : "Sessions"}
        </SidebarGroupLabel>
        <NewSessionMenu projects={bootstrap?.projects ?? []} />
      </div>
      <SidebarGroupContent>
        <SidebarMenu>
          {pinned.length > 0 && !activeProject ? (
            <>
              <ListSectionLabel count={pinned.length}>Pinned</ListSectionLabel>
              {pinned.map((row) => (
                <SessionRow key={row.id} active={row.id === activeId} row={row} />
              ))}
              {visible.length > 0 ? <ListSectionLabel>Recent</ListSectionLabel> : null}
            </>
          ) : null}
          {visible.slice(0, MAX_ACTIVE_ROWS).map((row) => (
            <SessionRow key={row.id} active={row.id === activeId} row={row} />
          ))}
          {visible.length > MAX_ACTIVE_ROWS ? (
            <li className="px-2 py-2 text-xs text-sidebar-muted-foreground">
              {visible.length - MAX_ACTIVE_ROWS} older sessions not shown
            </li>
          ) : null}
          {visible.length === 0 && (activeProject !== null || pinned.length === 0) ? (
            <li className="px-2 py-2 text-xs leading-5 text-sidebar-muted-foreground">
              {addedCwds.size === 0
                ? "Link a project to see its sessions."
                : "No unsettled sessions"}
            </li>
          ) : null}
        </SidebarMenu>
        {activeProject ? null : (
          <SettledShelf activeId={activeId} rows={settled} />
        )}
      </SidebarGroupContent>
    </SidebarGroup>
  );
}

function ListSectionLabel({
  children,
  count,
}: {
  children: ReactNode;
  count?: number;
}) {
  return (
    <li className="flex h-7 items-center gap-1 px-2 text-[11px] font-medium text-sidebar-muted-foreground">
      <span>{children}</span>
      {count === undefined ? null : (
        <span className="tabular-nums opacity-80">{count}</span>
      )}
    </li>
  );
}

function NewSessionMenu({
  projects,
}: {
  projects: Array<{ cwd: string; name: string }>;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create(cwd: string) {
    setBusy(true);
    setError(null);
    try {
      const { id } = await api.newSession(cwd);
      setOpen(false);
      location.hash = `#/s/${id}`;
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  if (projects.length === 0) return null;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label="New session"
        className="size-11 md:size-7"
        render={<Button size="icon-xs" variant="ghost" />}
      >
        <PlusIcon />
      </PopoverTrigger>
      <PopoverContent align="end" className="w-60 p-1" side="bottom">
        <p className="px-2 py-1 text-[11px] text-muted-foreground">
          Start a session in
        </p>
        {projects.map((project) => (
          <button
            className="flex min-h-11 w-full items-center gap-2 rounded-md px-2.5 text-left text-sm outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50 md:min-h-8 md:text-[13px]"
            disabled={busy}
            key={project.cwd}
            onClick={() => void create(project.cwd)}
            type="button"
          >
            <span className="truncate">{project.name}</span>
          </button>
        ))}
        {error ? (
          <p className="px-2 py-1 text-[11px] text-destructive">{error}</p>
        ) : null}
      </PopoverContent>
    </Popover>
  );
}

function useActiveProjectPath(): string | null {
  const hash = useHash();
  if (!hash.startsWith("#/p/")) return null;
  try {
    return decodeURIComponent(hash.slice(4));
  } catch {
    return null;
  }
}

function SettledShelf({
  rows,
  activeId,
}: {
  rows: SessionRowData[];
  activeId: string | null;
}) {
  const open = useApp((state) => state.settledFoldOpen);
  const toggle = useApp((state) => state.toggleSettledFold);
  if (rows.length === 0) return null;
  return (
    <div className="mt-1.5 border-t border-sidebar-border pt-1.5">
      <button
        aria-expanded={open}
        className="flex min-h-11 w-full items-center gap-1.5 rounded-[var(--control-radius)] px-2 text-xs font-medium text-sidebar-muted-foreground outline-none hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-8"
        onClick={toggle}
        type="button"
      >
        <ChevronRightIcon
          className={cn("size-3.5 transition-transform", open && "rotate-90")}
        />
        Settled
        <span className="ms-auto tabular-nums opacity-80">{rows.length}</span>
      </button>
      {open ? (
        <SidebarMenu>
          {rows.slice(0, MAX_SETTLED_ROWS).map((row) => (
            <SessionRow key={row.id} active={row.id === activeId} row={row} />
          ))}
        </SidebarMenu>
      ) : null}
    </div>
  );
}

function SessionRow({
  row,
  active,
}: {
  row: SessionRowData;
  active: boolean;
}) {
  const [titlePending, setTitlePending] = useState(false);
  const [aiTitlesEnabled, setAiTitlesEnabled] = useState(
    readAiTitleGenerationEnabled,
  );
  const titleAction = row.title
    ? "Regenerate title with AI (uses tokens)"
    : "Generate title with AI (uses tokens)";

  useEffect(() => {
    const sync = () => setAiTitlesEnabled(readAiTitleGenerationEnabled());
    window.addEventListener("pecan:title-settings", sync);
    return () => window.removeEventListener("pecan:title-settings", sync);
  }, []);

  async function togglePin() {
    try {
      if (row.pinned) await api.unpin(row.id);
      else await api.pin(row.id);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch {
      /* transient */
    }
  }

  async function generateTitle() {
    if (titlePending) return;
    setTitlePending(true);
    try {
      await api.regenerateTitle(row.id);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
      const current = useApp.getState().thread;
      if (current?.summary.id === row.id) {
        const updated = await api.thread(row.id);
        if (useApp.getState().thread?.summary.id === row.id) {
          useApp.getState().setThread(updated);
        }
      }
    } catch (error) {
      window.dispatchEvent(
        new CustomEvent("pecan:error", {
          detail: error instanceof Error ? error.message : String(error),
        }),
      );
    } finally {
      setTitlePending(false);
    }
  }

  return (
    <SidebarMenuItem
      className="group/row flex min-h-11 items-stretch rounded-[var(--control-radius)] outline-none ring-ring hover:bg-sidebar-row-hover focus-within:ring-2 data-[active=true]:bg-sidebar-row-selected md:min-h-9"
      data-active={active}
    >
      <NavHashLink
        aria-current={active ? "page" : undefined}
        className="flex min-w-0 flex-1 flex-col justify-center px-[var(--sidebar-row-content-inset)] py-1 outline-none"
        href={`#/s/${row.id}`}
      >
        <span className="flex w-full items-center gap-1.5">
          {row.waitingAskuser ? (
            <span
              className="size-1.5 shrink-0 rounded-full bg-warning"
              title="Waiting for your answer"
            />
          ) : (
            <span className="size-1.5 shrink-0" />
          )}
          <span className="truncate text-sm font-medium leading-4 text-sidebar-foreground md:text-[13px]">
            {row.title ?? row.preview ?? row.id.slice(0, 8)}
          </span>
          <span className="ms-auto shrink-0 ps-1 text-[11px] tabular-nums text-sidebar-muted-foreground">
            {timeLabel(row.lastActivity)}
          </span>
        </span>
        <span className="ps-3 text-xs leading-3.5 text-sidebar-muted-foreground md:text-[10px]">
          {projectName(row.cwd)}
          {row.kind === "subagent" ? " · sub" : ""}
        </span>
      </NavHashLink>
      <span className="flex shrink-0 items-center pe-0.5 md:pe-1">
        {aiTitlesEnabled ? (
          <button
            aria-label={titleAction}
            className={cn(
              "flex size-11 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none hover:bg-sidebar-row-active hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:pointer-events-none md:size-7 md:opacity-0 md:group-focus-within/row:pointer-events-auto md:group-focus-within/row:opacity-100 md:group-hover/row:pointer-events-auto md:group-hover/row:opacity-100",
              titlePending && "md:pointer-events-auto md:opacity-100",
            )}
            disabled={titlePending}
            onClick={() => void generateTitle()}
            title={titleAction}
            type="button"
          >
            {titlePending ? (
              <LoaderCircleIcon className="size-3.5 animate-spin" />
            ) : (
              <RefreshCwIcon className="size-3.5" />
            )}
          </button>
        ) : null}
        <button
          aria-label={row.pinned ? "Unpin session" : "Pin session"}
          className={cn(
            "flex size-11 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none hover:bg-sidebar-row-active hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:size-7",
            row.pinned
              ? "text-sidebar-foreground"
              : "md:pointer-events-none md:opacity-0 md:group-focus-within/row:pointer-events-auto md:group-focus-within/row:opacity-100 md:group-hover/row:pointer-events-auto md:group-hover/row:opacity-100",
          )}
          onClick={() => void togglePin()}
          title={row.pinned ? "Unpin" : "Pin"}
          type="button"
        >
          {row.pinned ? (
            <PinOffIcon className="size-3.5" />
          ) : (
            <PinIcon className="size-3.5" />
          )}
        </button>
      </span>
    </SidebarMenuItem>
  );
}

/** Anchor that closes the mobile sheet after navigating. */
function NavHashLink({ onClick, ...props }: ComponentProps<"a">) {
  return (
    <a
      {...props}
      onClick={(event) => {
        onClick?.(event);
        window.dispatchEvent(new CustomEvent("pecan:navigated"));
      }}
    />
  );
}

type ActiveSessionRoute = {
  sessionId: string;
  parentId?: string;
};

function useActiveSessionRoute(): ActiveSessionRoute | null {
  const hash = useHash();
  if (!hash.startsWith("#/s/")) return null;
  const [sessionId, query = ""] = hash.slice(4).split("?", 2);
  if (!sessionId) return null;
  const parentId = new URLSearchParams(query).get("parent")?.trim();
  return { sessionId, ...(parentId ? { parentId } : {}) };
}

/** Reactive location.hash. */
export function useHash(): string {
  const [hash, setHash] = useState(() => location.hash);
  useEffect(() => {
    const onChange = () => setHash(location.hash);
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);
  return hash;
}

function projectName(cwd: string) {
  return cwd.split("/").findLast((segment) => segment.length > 0) ?? cwd;
}

function timeLabel(iso: string) {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const seconds = Math.max(0, (Date.now() - date.getTime()) / 1000);
  if (seconds < 3600) {
    if (seconds < 90) return "now";
    return `${Math.round(seconds / 60)}m`;
  }
  if (date.toDateString() === new Date().toDateString()) {
    return date.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
  }
  if (seconds < 86_400 * 7) return `${Math.round(seconds / 86_400)}d`;
  return `${date.getMonth() + 1}/${date.getDate()}`;
}
