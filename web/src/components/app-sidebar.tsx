/**
 * App sidebar: brand header, pinned sessions, a project tree with each
 * project's open sessions nested under it, the Done shelf, and the footer.
 * Docked on desktop, Sheet on mobile (via `Sidebar`). Rows per project are
 * capped — the full index can hold thousands.
 */
import {
  ChevronRightIcon,
  SettingsIcon,
  SquarePenIcon,
  LoaderCircleIcon,
  PinIcon,
  PinOffIcon,
  PlusIcon,
  RefreshCwIcon,
  type LucideIcon,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useState,
  type ComponentProps,
  type ReactNode,
} from "react";

import { api } from "~/api/client";
import { timeLabel } from "~/lib/format";
import type { SessionRow as SessionRowData } from "~/api/types";
import { reportError } from "~/lib/errors";
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
  SidebarTrigger,
} from "~/components/ui/sidebar";
import {
  AddProjectForm,
  Collapse,
  ProjectFolder,
  useCollapsedProjects,
} from "~/components/projects-group";
import { Spinner } from "~/components/ui/spinner";
import { useApp } from "~/store";
import { startSession } from "~/lib/sessions";
import { cn } from "~/lib/utils";

const MAX_PROJECT_ROWS = 6;
const MAX_SETTLED_ROWS = 50;

export function AppSidebar() {
  return (
    <Sidebar collapsible="offcanvas" variant="inset">
      <SidebarHeader className="h-14 flex-row items-center gap-2 py-0 ps-3.5 pe-1">
        <BrandMark />
        <span className="text-sm font-semibold tracking-[-0.01em]">Pecan</span>
        <span className="ms-auto flex items-center gap-1">
          <ConnectionStatus />
          <SidebarTrigger className="hidden text-sidebar-muted-foreground md:inline-flex" />
        </span>
      </SidebarHeader>
      <SidebarContent className="gap-0 px-1">
        <PrimaryNav />
        <SessionTree />
      </SidebarContent>
      <SidebarFooter className="p-2">
        <SettingsLink />
      </SidebarFooter>
    </Sidebar>
  );
}

/** Rounded pecan-coloured monogram; the only brand colour in the chrome. */
export function BrandMark({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        "flex size-5 shrink-0 items-center justify-center rounded-[6px] bg-brand text-[11px] font-bold leading-none text-white dark:text-black",
        className,
      )}
    >
      p
    </span>
  );
}

/** Silent while live; a small amber pill only when the event stream drops. */
function ConnectionStatus() {
  const connected = useApp((state) => state.connected);
  return (
    <span role="status">
      {connected ? (
        <span className="sr-only">Live</span>
      ) : (
        <span
          className="flex animate-toast-in items-center gap-1.5 rounded-full bg-warning/12 px-2 py-0.5 text-[11px] font-medium text-sidebar-muted-foreground"
          title="Live updates reconnecting…"
        >
          <span
            aria-hidden
            className="size-1.5 animate-pulse rounded-full bg-warning"
          />
          Reconnecting
        </span>
      )}
    </span>
  );
}

/** Top-level actions, styled as quiet icon rows. */
function PrimaryNav() {
  const projects = useApp((state) => state.bootstrap)?.projects ?? [];
  const openCwd = useApp((state) => state.thread?.summary.cwd);
  const startingCwd = useApp((state) => state.startingCwd);
  const onlyProject =
    projects.find((project) => project.cwd === openCwd) ??
    (projects.length === 1 ? projects[0] : undefined);
  return (
    <SidebarMenu className="px-1 pt-1 pb-2">
      {onlyProject ? (
        <SidebarMenuItem>
          <SidebarMenuButton
            className="h-9 text-sidebar-muted-foreground md:h-8 md:text-[13px]"
            data-testid="nav-new-session"
            disabled={startingCwd !== null}
            onClick={() => void startSession(onlyProject.cwd)}
          >
            {startingCwd ? <Spinner className="size-4" /> : <SquarePenIcon />}
            <span>New session</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      ) : null}
    </SidebarMenu>
  );
}

function SettingsLink() {
  const active = useHash() === "#/settings";
  return (
    <SidebarMenuButton
      render={<NavHashLink href="#/settings" />}
      isActive={active}
      tooltip="Settings"
      className="min-h-10 w-full px-2.5 text-sidebar-muted-foreground md:min-h-8 md:text-[13px]"
    >
      <SettingsIcon />
      <span>Settings</span>
    </SidebarMenuButton>
  );
}

function SessionTree() {
  const sessions = useApp((state) => state.sessions);
  const projects = useApp((state) => state.bootstrap)?.projects ?? [];
  const activeId = useActiveSessionRoute()?.sessionId ?? null;
  const { collapsed, toggle } = useCollapsedProjects();
  const [adding, setAdding] = useState(false);

  const { pinned, byProject, settled } = useMemo(() => {
    const cwds = new Set(projects.map((project) => project.cwd));
    // Child threads are navigated from their parent's agent rail, where the
    // exact parent route is available; only linked projects are tracked.
    const rows = sessions
      .filter((row) => row.kind !== "subagent" && cwds.has(row.cwd))
      .sort((a, b) => Date.parse(b.lastActivity) - Date.parse(a.lastActivity));
    const grouped = new Map<string, SessionRowData[]>();
    for (const row of rows) {
      if (row.settled || row.pinned) continue;
      const list = grouped.get(row.cwd);
      if (list) list.push(row);
      else grouped.set(row.cwd, [row]);
    }
    return {
      pinned: rows.filter((row) => !row.settled && row.pinned),
      byProject: grouped,
      settled: rows.filter((row) => row.settled),
    };
  }, [sessions, projects]);

  return (
    <>
      {pinned.length > 0 ? (
        <SidebarGroup className="py-1">
          <GroupHeading>Pinned</GroupHeading>
          <SidebarMenu>
            {pinned.map((row, index) => (
              <SessionRow
                key={row.id}
                active={row.id === activeId}
                index={index}
                row={row}
              />
            ))}
          </SidebarMenu>
        </SidebarGroup>
      ) : null}
      <SidebarGroup className="flex-1 py-1">
        <GroupHeading
          action={{
            icon: PlusIcon,
            label: "Add project",
            pressed: adding,
            onClick: () => setAdding((value) => !value),
          }}
        >
          Projects
        </GroupHeading>
        <SidebarGroupContent>
          <Collapse open={adding}>
            <div className="px-1 pt-1 pb-2">
              {adding ? (
                <AddProjectForm onDone={() => setAdding(false)} />
              ) : null}
            </div>
          </Collapse>
          <SidebarMenu className="gap-0.5">
            {projects.map((project) => (
              <ProjectFolder
                key={project.cwd}
                onToggle={() => toggle(project.cwd)}
                open={!collapsed.has(project.cwd)}
                project={project}
              >
                <ProjectSessions
                  activeId={activeId}
                  rows={byProject.get(project.cwd) ?? []}
                />
              </ProjectFolder>
            ))}
            {projects.length === 0 && !adding ? (
              <li className="px-2 py-1 text-xs leading-5 text-sidebar-muted-foreground">
                No projects yet. Tap + and paste a folder path.
              </li>
            ) : null}
          </SidebarMenu>
          <SettledShelf activeId={activeId} rows={settled} />
        </SidebarGroupContent>
      </SidebarGroup>
    </>
  );
}

function GroupHeading({
  children,
  action,
}: {
  children: ReactNode;
  action?: {
    icon: LucideIcon;
    label: string;
    pressed: boolean;
    onClick: () => void;
  };
}) {
  return (
    <div className="flex min-h-9 items-center px-2">
      <SidebarGroupLabel className="h-auto flex-1 px-0 text-xs font-normal text-sidebar-muted-foreground">
        {children}
      </SidebarGroupLabel>
      {action ? (
        <button
          aria-label={action.label}
          aria-pressed={action.pressed}
          className="-me-1 flex size-11 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none transition-[color,background-color,rotate] duration-200 hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring aria-pressed:rotate-45 md:size-6"
          onClick={action.onClick}
          title={action.label}
          type="button"
        >
          <action.icon className="size-3.5" />
        </button>
      ) : null}
    </div>
  );
}

/** A project's open sessions, indented so titles align with the folder name. */
function ProjectSessions({
  rows,
  activeId,
}: {
  rows: SessionRowData[];
  activeId: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
  // Keep the open session visible even when it sits past the cap.
  const activeIndex = rows.findIndex((row) => row.id === activeId);
  const limit = expanded
    ? rows.length
    : Math.max(MAX_PROJECT_ROWS, activeIndex + 1);
  const hidden = rows.length - limit;
  return (
    <SidebarMenu className="ps-2.5 pt-0.5 pb-1.5">
      {rows.slice(0, limit).map((row, index) => (
        <SessionRow
          key={row.id}
          active={row.id === activeId}
          index={index}
          row={row}
        />
      ))}
      {rows.length === 0 ? (
        <li className="px-[var(--sidebar-row-content-inset)] py-1 ps-[calc(var(--sidebar-row-content-inset)+14px)] text-xs text-sidebar-muted-foreground/80">
          No open sessions
        </li>
      ) : null}
      {hidden > 0 || expanded ? (
        <li>
          <button
            className="flex min-h-9 w-full items-center rounded-[var(--control-radius)] ps-[calc(var(--sidebar-row-content-inset)+14px)] text-xs text-sidebar-muted-foreground outline-none transition-colors hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-7"
            onClick={() => setExpanded((value) => !value)}
            type="button"
          >
            {expanded ? "Show less" : `Show ${hidden} more`}
          </button>
        </li>
      ) : null}
    </SidebarMenu>
  );
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
    <div className="mt-2 pt-1">
      <button
        aria-expanded={open}
        className="flex min-h-11 w-full items-center gap-1.5 rounded-[var(--control-radius)] px-2 text-xs font-medium text-sidebar-muted-foreground outline-none hover:bg-sidebar-row-hover hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:min-h-8"
        onClick={toggle}
        type="button"
      >
        <ChevronRightIcon
          className={cn(
            "size-3.5 transition-transform duration-200 ease-[cubic-bezier(0.2,0,0,1)]",
            open && "rotate-90",
          )}
        />
        Done
        <span className="ms-auto tabular-nums opacity-80">{rows.length}</span>
      </button>
      <Collapse open={open}>
        <SidebarMenu>
          {rows.slice(0, MAX_SETTLED_ROWS).map((row) => (
            <SessionRow key={row.id} active={row.id === activeId} row={row} />
          ))}
        </SidebarMenu>
      </Collapse>
    </div>
  );
}

function SessionRow({
  row,
  active,
  index = 0,
}: {
  row: SessionRowData;
  active: boolean;
  index?: number;
}) {
  const waiting = useApp(
    (state) =>
      row.waitingAskuser || (state.pendingAsks[row.id]?.length ?? 0) > 0,
  );
  const running = useApp(
    (state) =>
      !waiting && state.streaming && state.thread?.summary.id === row.id,
  );
  const [titlePending, setTitlePending] = useState(false);
  const titleAction = row.title
    ? "Regenerate title with AI (uses tokens)"
    : "Generate title with AI (uses tokens)";

  async function togglePin() {
    try {
      if (row.pinned) await api.unpin(row.id);
      else await api.pin(row.id);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (error) {
      reportError(
        error instanceof Error ? error.message : "Failed to update pin.",
      );
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
      className="group/row flex min-h-11 animate-row-in items-stretch rounded-[var(--control-radius)] outline-none ring-ring transition-colors duration-150 hover:bg-sidebar-row-hover focus-within:ring-2 data-[active=true]:bg-sidebar-row-selected md:min-h-8"
      data-active={active}
      style={{ animationDelay: `${Math.min(index, 8) * 24}ms` }}
    >
      <NavHashLink
        aria-current={active ? "page" : undefined}
        className="flex min-w-0 flex-1 flex-col justify-center px-[var(--sidebar-row-content-inset)] outline-none transition-transform duration-100 active:scale-[0.985]"
        href={`#/s/${row.id}`}
      >
        <span className="flex w-full items-center gap-2">
          <StatusDot running={running} waiting={waiting} />
          <span
            className={cn(
              "truncate text-sm leading-5 transition-colors md:text-[13px]",
              running
                ? "text-shimmer"
                : active
                  ? "text-sidebar-foreground"
                  : "text-sidebar-foreground/80",
            )}
          >
            {row.title ?? row.preview ?? row.id.slice(0, 8)}
          </span>
          {waiting ? (
            <span className="sr-only">Waiting for your answer</span>
          ) : null}
          {running ? <span className="sr-only">Working</span> : null}
          <span
            className={cn(
              "ms-auto shrink-0 ps-1 text-[11px] tabular-nums text-sidebar-muted-foreground/80 transition-opacity md:group-hover/row:opacity-0 md:group-focus-within/row:opacity-0",
              row.pinned && "md:opacity-0",
            )}
          >
            {timeLabel(row.lastActivity)}
            {row.kind === "subagent" ? " · sub" : ""}
          </span>
        </span>
      </NavHashLink>
      <span className="flex shrink-0 items-center pe-0.5 md:absolute md:inset-y-0 md:end-0 md:pe-1">
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
        <button
          aria-label={row.pinned ? "Unpin session" : "Pin session"}
          className={cn(
            "flex size-11 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none hover:bg-sidebar-row-active hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:size-7",
            row.pinned
              ? "text-sidebar-foreground"
              : "text-sidebar-muted-foreground/50 md:pointer-events-none md:opacity-0 md:group-focus-within/row:pointer-events-auto md:group-focus-within/row:opacity-100 md:group-hover/row:pointer-events-auto md:group-hover/row:opacity-100",
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

/** Pulsing green while Pi works, amber while it waits on you, quiet otherwise. */
function StatusDot({
  running,
  waiting,
}: {
  running: boolean;
  waiting: boolean;
}) {
  return (
    <span aria-hidden className="relative flex size-1.5 shrink-0">
      {running || waiting ? (
        <span
          className={cn(
            "absolute inset-0 animate-ping rounded-full opacity-60 motion-reduce:hidden",
            running ? "bg-success" : "bg-warning",
          )}
        />
      ) : null}
      <span
        className={cn(
          "relative size-1.5 rounded-full transition-colors duration-300",
          running
            ? "bg-success"
            : waiting
              ? "bg-warning"
              : "bg-sidebar-muted-foreground/35",
        )}
      />
    </span>
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
