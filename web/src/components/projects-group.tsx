/**
 * Projects sidebar group: linked project directories with add/remove.
 * Adding a directory links it; it appears immediately with its live session
 * count. Errors surface inline instead of vanishing.
 */
import { PlusIcon, XIcon } from "lucide-react";
import { useState } from "react";

import { api } from "~/api/client";
import type { Project } from "~/api/types";
import {
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
} from "~/components/ui/sidebar";
import { Input } from "~/components/ui/input";
import { useHash } from "~/components/app-sidebar";
import { useApp } from "~/store";

export function ProjectsGroup() {
  // Select the stable bootstrap object; deriving the array in the selector
  // would return a fresh reference each snapshot and loop useSyncExternalStore.
  const projects = useApp((state) => state.bootstrap)?.projects ?? [];
  const [adding, setAdding] = useState(false);
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
      setAdding(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove(cwd: string) {
    try {
      await api.removeProject(cwd);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <SidebarGroup className="pb-2 pt-1.5">
      <div className="flex min-h-9 items-center px-2">
        <SidebarGroupLabel className="h-auto flex-1 px-0 text-xs font-semibold">
          Projects
        </SidebarGroupLabel>
        <SidebarGroupAction
          className="static size-11 md:size-7"
          onClick={() => setAdding((value) => !value)}
          title="Link a project directory"
          aria-label="Add project"
        >
          <PlusIcon />
        </SidebarGroupAction>
      </div>
      <SidebarGroupContent>
        {adding ? (
          <div className="px-1 pb-2 pt-1">
            <Input
              autoFocus
              aria-label="Project directory"
              className="min-h-11 font-mono text-sm md:min-h-8 md:text-xs"
              placeholder="/Users/you/code/project"
              spellCheck={false}
              value={draft}
              onChange={(event) => {
                setDraft(event.target.value);
                setError(null);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !busy) void submit();
                if (event.key === "Escape") {
                  setAdding(false);
                  setError(null);
                }
              }}
            />
            {error ? (
              <p className="mt-1 px-1 text-[11px] leading-4 text-destructive">
                {error}
              </p>
            ) : null}
          </div>
        ) : null}

        <SidebarMenu>
          {projects.map((project: Project) => (
            <ProjectRow
              key={project.cwd}
              project={project}
              onRemove={() => void remove(project.cwd)}
            />
          ))}
          {projects.length === 0 && !adding ? (
            <li className="px-2 py-1 text-xs leading-5 text-sidebar-muted-foreground">
              Press + to link a project directory.
            </li>
          ) : null}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}

function ProjectRow({
  project,
  onRemove,
}: {
  project: Project;
  onRemove: () => void;
}) {
  const href = `#/p/${encodeURIComponent(project.cwd)}`;
  const hash = useHash();
  const active = hash === href;
  return (
    <li
      className="group/row flex min-h-11 items-center rounded-[var(--control-radius)] outline-hidden ring-ring focus-within:ring-2 hover:bg-sidebar-row-hover data-[active=true]:bg-sidebar-row-selected md:min-h-8"
      data-active={active}
    >
      <a
        aria-current={active ? "page" : undefined}
        className="flex min-h-11 min-w-0 flex-1 items-center gap-2 px-[var(--sidebar-row-content-inset)] outline-none md:min-h-8"
        href={href}
        onClick={() => {
          window.dispatchEvent(new CustomEvent("pecan:navigated"));
        }}
      >
        <span
          className="truncate text-sm font-medium text-sidebar-foreground md:text-[13px]"
          title={project.cwd}
        >
          {project.name}
        </span>
        <span className="ms-auto shrink-0 ps-1 text-xs tabular-nums text-sidebar-muted-foreground md:text-[11px]">
          {project.sessionCount > 0 ? project.sessionCount : ""}
        </span>
      </a>
      <button
        aria-label={`Unlink ${project.name}`}
        className="flex size-11 shrink-0 items-center justify-center rounded-md text-sidebar-muted-foreground outline-none hover:bg-sidebar-row-active hover:text-sidebar-foreground focus-visible:ring-2 focus-visible:ring-ring md:pointer-events-none md:size-7 md:opacity-0 md:group-focus-within/row:pointer-events-auto md:group-focus-within/row:opacity-100 md:group-hover/row:pointer-events-auto md:group-hover/row:opacity-100"
        onClick={onRemove}
        type="button"
      >
        <XIcon className="size-3.5" />
      </button>
    </li>
  );
}
