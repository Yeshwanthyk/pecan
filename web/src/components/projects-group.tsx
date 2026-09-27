/**
 * Project folders for the sidebar tree: a collapsible header per linked
 * directory with its sessions nested inside (Cursor's agents layout), plus the
 * inline add-project form. Collapse state persists per device.
 */
import {
  ChevronRightIcon,
  EllipsisIcon,
  FolderIcon,
  FolderOpenIcon,
  PlusIcon,
  XIcon,
} from "lucide-react";
import { useState, type ReactNode } from "react";

import { api } from "~/api/client";
import type { Project } from "~/api/types";
import { Input } from "~/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "~/components/ui/popover";
import { Spinner } from "~/components/ui/spinner";
import { reportError } from "~/lib/errors";
import { startSession } from "~/lib/sessions";
import { readStoredStrings, writeLocalStorage } from "~/lib/storage";
import { cn } from "~/lib/utils";
import { useApp } from "~/store";

const COLLAPSED_KEY = "pecan:collapsed-projects";

function readCollapsed(): Set<string> {
  return new Set(readStoredStrings(COLLAPSED_KEY));
}

/** Per-device set of collapsed project cwds. */
export function useCollapsedProjects() {
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const toggle = (cwd: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(cwd)) next.add(cwd);
      writeLocalStorage(COLLAPSED_KEY, JSON.stringify([...next]));
      return next;
    });
  return { collapsed, toggle };
}

/** Animated height collapse: grid rows 0fr ↔ 1fr, content stays mounted. */
export function Collapse({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      className={cn(
        "grid transition-[grid-template-rows,opacity] duration-200 ease-[cubic-bezier(0.2,0,0,1)] motion-reduce:transition-none",
        open ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
      )}
      inert={!open}
    >
      <div className="min-h-0 overflow-hidden">{children}</div>
    </div>
  );
}

/** One project: header row (toggle, count, actions) with its sessions below. */
export function ProjectFolder({
  project,
  open,
  onToggle,
  children,
}: {
  project: Project;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  const startingCwd = useApp((state) => state.startingCwd);
  const starting = startingCwd === project.cwd;
  return (
    <li className="group/project">
      <div className="group/header flex min-h-11 items-center rounded-[var(--control-radius)] transition-colors duration-150 hover:bg-sidebar-row-hover md:min-h-8">
        <button
          aria-expanded={open}
          className="flex min-h-11 min-w-0 flex-1 items-center gap-2 rounded-[var(--control-radius)] px-[var(--sidebar-row-content-inset)] text-left outline-none focus-visible:ring-2 focus-visible:ring-ring active:scale-[0.985] transition-transform duration-100 md:min-h-8"
          onClick={onToggle}
          title={project.cwd}
          type="button"
        >
          <span className="relative size-4 shrink-0 text-[var(--sidebar-icon-color)]">
            {open ? (
              <FolderOpenIcon className="absolute inset-0 size-4 transition-opacity duration-150 md:group-hover/header:opacity-0" />
            ) : (
              <FolderIcon className="absolute inset-0 size-4 transition-opacity duration-150 md:group-hover/header:opacity-0" />
            )}
            <ChevronRightIcon
              className={cn(
                "absolute inset-0 size-4 opacity-0 transition-[opacity,rotate] duration-200 ease-[cubic-bezier(0.2,0,0,1)] md:group-hover/header:opacity-100",
                open && "rotate-90",
              )}
            />
          </span>
          <span className="truncate text-sm font-medium text-sidebar-foreground/85 md:text-[13px]">
            {project.name}
          </span>
          <span className="ms-auto shrink-0 ps-1 text-[11px] tabular-nums text-sidebar-muted-foreground/70 md:group-hover/header:opacity-0">
            {project.sessionCount > 0 ? project.sessionCount : ""}
          </span>
        </button>
        <span className="flex shrink-0 items-center pe-0.5 transition-opacity duration-150 md:pe-1 md:opacity-0 md:group-focus-within/header:opacity-100 md:group-hover/header:opacity-100">
          <ProjectMenu project={project} />
          <button
            aria-label={`New session in ${project.name}`}
            className="flex size-11 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none transition-[color,background-color,scale] hover:bg-sidebar-row-active hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring active:scale-90 disabled:opacity-50 md:size-6"
            data-testid="project-new-session"
            disabled={startingCwd !== null}
            onClick={() => void startSession(project.cwd)}
            title="New session"
            type="button"
          >
            {starting ? <Spinner className="size-3.5" /> : <PlusIcon className="size-3.5" />}
          </button>
        </span>
      </div>
      <Collapse open={open}>{children}</Collapse>
    </li>
  );
}

function ProjectMenu({ project }: { project: Project }) {
  const [open, setOpen] = useState(false);
  async function unlink() {
    try {
      await api.removeProject(project.cwd);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
      setOpen(false);
    } catch (cause) {
      reportError(cause instanceof Error ? cause.message : String(cause));
    }
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={`${project.name} options`}
        className="flex size-11 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none transition-colors hover:bg-sidebar-row-active hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:size-6"
      >
        <EllipsisIcon className="size-3.5" />
      </PopoverTrigger>
      <PopoverContent align="end" className="w-64 p-1" side="bottom">
        <p className="truncate px-2 py-1.5 font-mono text-[11px] text-muted-foreground" title={project.cwd}>
          {project.cwd}
        </p>
        <button
          className="flex min-h-11 w-full items-center gap-2 rounded-md px-2.5 text-left text-sm text-destructive outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring md:min-h-8 md:text-[13px]"
          onClick={() => void unlink()}
          type="button"
        >
          <XIcon className="size-3.5" />
          Remove from sidebar
        </button>
      </PopoverContent>
    </Popover>
  );
}

/** Links a project folder by absolute path; `onDone` runs after success or Escape. */
export function AddProjectForm({
  onDone,
  autoFocus = true,
}: {
  onDone?: () => void;
  autoFocus?: boolean;
}) {
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    const cwd = draft.trim();
    if (!cwd.startsWith("/")) {
      setError("Enter an absolute path, e.g. /Users/you/code/project");
      return;
    }
    setBusy(true);
    try {
      await api.addProject(cwd);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
      setDraft("");
      setError(null);
      onDone?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (!busy) void submit();
      }}
    >
      <label className="block px-1 pb-1.5 text-[11px] leading-4 text-muted-foreground" htmlFor="add-project-path">
        Full path of the folder Pi should work in
      </label>
      <Input
        autoFocus={autoFocus}
        id="add-project-path"
        aria-label="Project directory"
        className="min-h-11 font-mono text-sm md:min-h-9 md:text-[13px]"
        placeholder="/path/to/project"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        enterKeyHint="go"
        value={draft}
        onChange={(event) => {
          setDraft(event.target.value);
          setError(null);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            setError(null);
            onDone?.();
          }
        }}
      />
      {error ? (
        <p className="mt-1 px-1 text-[11px] leading-4 text-destructive">{error}</p>
      ) : null}
    </form>
  );
}
