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
  SearchIcon,
  XIcon,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { api } from "~/api/client";
import type { FolderPick, Project } from "~/api/types";
import { Input } from "~/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "~/components/ui/popover";
import { Spinner } from "~/components/ui/spinner";
import { reportError } from "~/lib/errors";
import { timeLabel } from "~/lib/format";
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
export function Collapse({
  open,
  children,
}: {
  open: boolean;
  children: ReactNode;
}) {
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
            {starting ? (
              <Spinner className="size-3.5" />
            ) : (
              <PlusIcon className="size-3.5" />
            )}
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
        <p
          className="truncate px-2 py-1.5 font-mono text-[11px] text-muted-foreground"
          title={project.cwd}
        >
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

/** Shows `path` relative to `home` as `~/…` when it lives under it. */
export function tildify(path: string, home: string | null | undefined): string {
  if (!home) return path;
  if (path === home) return "~";
  return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

type PickRow =
  | { kind: "known"; path: string; name: string; detail: string }
  | { kind: "entry"; path: string; name: string };

const PICK_DEBOUNCE_MS = 90;

/**
 * Folder picker that links a project: suggests folders Pi already worked in,
 * completes typed paths (`~/` or `/`) one directory at a time, and adds on
 * tap or Enter. `onDone` runs after success or Escape.
 */
export function AddProjectForm({
  onDone,
  autoFocus = true,
}: {
  onDone?: () => void;
  autoFocus?: boolean;
}) {
  const [draft, setDraft] = useState("");
  const [pick, setPick] = useState<FolderPick | null>(null);
  const [highlight, setHighlight] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const seq = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const mine = ++seq.current;
    const timer = window.setTimeout(
      () => {
        api.folders(draft.trim()).then(
          (result) => {
            if (seq.current === mine) {
              setPick(result);
              setHighlight(0);
            }
          },
          () => {
            if (seq.current === mine) setPick(null);
          },
        );
      },
      draft === "" ? 0 : PICK_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [draft]);

  const home = pick?.home ?? null;
  const rows: PickRow[] = [
    ...(pick?.known ?? []).map((folder) => ({
      kind: "known" as const,
      path: folder.path,
      name: folder.name,
      detail: `${folder.sessions} session${folder.sessions === 1 ? "" : "s"} · ${timeLabel(folder.lastActivity)}`,
    })),
    ...(pick?.entries ?? []).map((entry) => ({
      kind: "entry" as const,
      path: entry.path,
      name: entry.name,
    })),
  ];
  const knownCount = pick?.known.length ?? 0;

  async function add(path: string) {
    const cwd = path.trim();
    if (!cwd.startsWith("/") && !cwd.startsWith("~")) {
      setError("Type a path starting with ~/ or /");
      return;
    }
    setBusy(cwd);
    try {
      await api.addProject(cwd);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
      setDraft("");
      setError(null);
      onDone?.();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }

  function browse(path: string) {
    setDraft(`${tildify(path, home)}/`);
    setError(null);
    inputRef.current?.focus();
  }

  return (
    <form
      className="flex flex-col"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy) return;
        const row = rows[highlight];
        void add(
          row && draft.trim() !== "" && !draft.endsWith("/")
            ? row.path
            : draft || row?.path || "",
        );
      }}
    >
      <div className="relative">
        <SearchIcon className="pointer-events-none absolute start-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          ref={inputRef}
          autoFocus={autoFocus}
          aria-label="Project folder"
          aria-autocomplete="list"
          aria-controls="folder-pick-list"
          className="min-h-11 ps-8 font-mono text-sm md:min-h-9 md:text-[13px]"
          placeholder="~/code/project"
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
            } else if (event.key === "ArrowDown" && rows.length > 0) {
              event.preventDefault();
              setHighlight((index) => (index + 1) % rows.length);
            } else if (event.key === "ArrowUp" && rows.length > 0) {
              event.preventDefault();
              setHighlight((index) => (index - 1 + rows.length) % rows.length);
            } else if (
              event.key === "Tab" &&
              rows[highlight]?.kind === "entry" &&
              !event.shiftKey
            ) {
              event.preventDefault();
              browse(rows[highlight].path);
            }
          }}
        />
      </div>
      {error ? (
        <p className="mt-1.5 px-1 text-[11px] leading-4 text-destructive">
          {error}
        </p>
      ) : null}
      <ul
        className="mt-1.5 -mx-1 flex max-h-72 flex-col overflow-y-auto overscroll-contain"
        id="folder-pick-list"
        role="listbox"
      >
        {rows.map((row, index) => (
          <li key={`${row.kind}:${row.path}`} role="presentation">
            {index === 0 && row.kind === "known" ? (
              <PickHeading>Pi has worked in</PickHeading>
            ) : null}
            {index === knownCount && row.kind === "entry" ? (
              <PickHeading>{knownCount > 0 ? "Folders" : "Browse"}</PickHeading>
            ) : null}
            <div
              aria-selected={index === highlight}
              className="group/pick flex min-h-11 items-center rounded-md transition-colors aria-selected:bg-accent md:min-h-8"
              onMouseEnter={() => setHighlight(index)}
              role="option"
            >
              <button
                className="flex min-w-0 flex-1 items-center gap-2 self-stretch px-2 text-left outline-none"
                disabled={busy !== null}
                onClick={() =>
                  row.kind === "entry" ? browse(row.path) : void add(row.path)
                }
                title={row.path}
                type="button"
              >
                {busy === row.path ? (
                  <Spinner className="size-3.5 shrink-0" />
                ) : (
                  <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                )}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] leading-5">
                    {row.name}
                  </span>
                  {row.kind === "known" ? (
                    <span className="block truncate text-[11px] leading-4 text-muted-foreground">
                      {tildify(row.path, home)} · {row.detail}
                    </span>
                  ) : null}
                </span>
                {row.kind === "entry" ? (
                  <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
                ) : null}
              </button>
              {row.kind === "entry" ? (
                <button
                  className="me-1 hidden h-7 shrink-0 items-center rounded-md px-2 text-xs font-medium text-primary outline-none hover:bg-background focus-visible:ring-2 focus-visible:ring-ring group-aria-selected/pick:flex max-md:flex"
                  disabled={busy !== null}
                  onClick={() => void add(row.path)}
                  type="button"
                >
                  Add
                </button>
              ) : null}
            </div>
          </li>
        ))}
        {pick && rows.length === 0 ? (
          <li className="px-2 py-2 text-[11px] leading-4 text-muted-foreground">
            {draft.trim() === ""
              ? "Type ~/ to browse folders on this machine."
              : "No matching folders. Press Enter to add this path."}
          </li>
        ) : null}
      </ul>
      {draft.trim() !== "" ? (
        <button
          className="mt-1 flex min-h-11 items-center justify-center gap-1.5 rounded-md bg-primary px-3 text-[13px] font-medium text-primary-foreground outline-none transition active:scale-[0.98] focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60 md:min-h-8"
          disabled={busy !== null}
          type="submit"
        >
          {busy === draft.trim() ? (
            <Spinner className="size-3.5" />
          ) : (
            <PlusIcon className="size-3.5" />
          )}
          <span className="truncate">
            Add {draft.trim().replace(/\/+$/, "") || "/"}
          </span>
        </button>
      ) : null}
    </form>
  );
}

function PickHeading({ children }: { children: string }) {
  return (
    <p className="px-2 pt-2 pb-1 text-[11px] font-medium text-muted-foreground">
      {children}
    </p>
  );
}
