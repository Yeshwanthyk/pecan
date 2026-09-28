/**
 * Home: a Cursor-style task composer. Type what Pi should do, pick the
 * project from the chip, and send — that starts a session there and hands it
 * the prompt. Recent threads sit underneath for picking up earlier work.
 */
import {
  ArrowUpIcon,
  CheckIcon,
  ChevronDownIcon,
  FolderIcon,
  PlusIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { api } from "~/api/client";
import type { Project } from "~/api/types";
import { BrandMark } from "~/components/app-sidebar";
import { AddProjectForm, tildify } from "~/components/projects-group";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "~/components/ui/popover";
import { Spinner } from "~/components/ui/spinner";
import { reportError } from "~/lib/errors";
import { projectName, timeLabel } from "~/lib/format";
import { startSession } from "~/lib/sessions";
import { readLocalStorage, writeLocalStorage } from "~/lib/storage";
import { cn } from "~/lib/utils";
import { useApp } from "~/store";

const PROJECT_KEY = "pecan:home-project";
const RECENT_ON_HOME = 6;

/** The project the home composer targets: last used, else most recent. */
function initialProject(projects: Project[]): string | null {
  const stored = readLocalStorage(PROJECT_KEY);
  if (stored && projects.some((project) => project.cwd === stored))
    return stored;
  const latest = [...projects].sort(
    (a, b) =>
      Date.parse(b.lastActivity ?? "") - Date.parse(a.lastActivity ?? ""),
  )[0];
  return latest?.cwd ?? null;
}

export function Home() {
  const bootstrap = useApp((state) => state.bootstrap);
  if (bootstrap === null) {
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <Spinner className="size-5" />
      </div>
    );
  }
  return (
    <div className="flex-1 overflow-y-auto" data-testid="empty-state">
      <div className="mx-auto flex w-full max-w-2xl flex-col px-4 pt-[12vh] pb-16 sm:px-6 sm:pt-[18vh]">
        {bootstrap.projects.length === 0 ? (
          <Welcome />
        ) : (
          <>
            <TaskComposer projects={bootstrap.projects} />
            <RecentThreads projects={bootstrap.projects} />
          </>
        )}
      </div>
    </div>
  );
}

function Welcome() {
  return (
    <div className="animate-row-in">
      <BrandMark className="size-9 rounded-[10px] text-lg" />
      <h1 className="mt-5 text-xl font-semibold tracking-[-0.015em]">
        Welcome to Pecan
      </h1>
      <p className="mt-1 text-sm leading-6 text-muted-foreground">
        Pick a folder for Pi to work in. You can drive its sessions from any
        device.
      </p>
      <div className="mt-6 rounded-2xl border bg-card p-3 shadow-[0_1px_2px_rgb(0_0_0/4%),0_6px_24px_-8px_rgb(0_0_0/10%)]">
        <AddProjectForm autoFocus={false} />
      </div>
    </div>
  );
}

function TaskComposer({ projects }: { projects: Project[] }) {
  const [cwd, setCwd] = useState(() => initialProject(projects));
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const project =
    projects.find((candidate) => candidate.cwd === cwd) ?? projects[0];

  useEffect(() => {
    if (!window.matchMedia("(pointer: coarse)").matches)
      inputRef.current?.focus();
  }, []);

  function choose(next: string) {
    setCwd(next);
    writeLocalStorage(PROJECT_KEY, next);
    inputRef.current?.focus();
  }

  async function submit() {
    const prompt = text.trim();
    if (!project || sending) return;
    setSending(true);
    try {
      const id = await startSession(project.cwd);
      if (id !== null && prompt !== "") await api.send(id, prompt, "send");
      setText("");
    } catch (error) {
      reportError(error instanceof Error ? error.message : String(error));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="animate-row-in">
      <h1 className="mb-4 px-1 text-[22px] font-semibold tracking-[-0.02em] sm:text-2xl">
        What should Pi work on?
      </h1>
      <form
        className="rounded-2xl border bg-card shadow-[0_1px_2px_rgb(0_0_0/4%),0_8px_32px_-12px_rgb(0_0_0/14%)] transition-[border-color,box-shadow] focus-within:border-input focus-within:shadow-[0_1px_2px_rgb(0_0_0/4%),0_12px_40px_-12px_rgb(0_0_0/18%)]"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <textarea
          ref={inputRef}
          aria-label="Task for Pi"
          className="block max-h-[40vh] min-h-[88px] w-full resize-none bg-transparent px-4 pt-3.5 pb-1 text-base leading-6 outline-none [field-sizing:content] placeholder:text-muted-foreground sm:text-[15px]"
          data-testid="home-input"
          enterKeyHint="send"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.nativeEvent.isComposing &&
              !window.matchMedia("(pointer: coarse)").matches
            ) {
              event.preventDefault();
              void submit();
            }
          }}
          placeholder="Describe a task, ask about the code, plan a change…"
          value={text}
        />
        <div className="flex items-center gap-2 px-2 pt-1 pb-2">
          <ProjectChip
            current={project}
            onChoose={choose}
            projects={projects}
          />
          <button
            aria-label={
              text.trim() === "" ? "Start an empty session" : "Start session"
            }
            className={cn(
              "ms-auto flex size-9 shrink-0 items-center justify-center rounded-full outline-none transition-[background-color,color,scale] duration-150 focus-visible:ring-2 focus-visible:ring-ring active:scale-90 disabled:opacity-60 sm:size-8",
              text.trim() === ""
                ? "bg-muted text-muted-foreground hover:bg-accent hover:text-foreground"
                : "bg-primary text-primary-foreground",
            )}
            data-testid="home-send"
            disabled={sending || !project}
            title={
              text.trim() === ""
                ? "Start an empty session"
                : "Start session (Enter)"
            }
            type="submit"
          >
            {sending ? (
              <Spinner className="size-4" />
            ) : (
              <ArrowUpIcon className="size-4" />
            )}
          </button>
        </div>
      </form>
    </div>
  );
}

function ProjectChip({
  projects,
  current,
  onChoose,
}: {
  projects: Project[];
  current: Project | undefined;
  onChoose: (cwd: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const home = useHomeDir(projects);
  return (
    <Popover
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setAdding(false);
      }}
      open={open}
    >
      <PopoverTrigger
        className="flex h-9 min-w-0 items-center gap-1.5 rounded-full border border-transparent px-2.5 text-[13px] text-muted-foreground outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-[popup-open]:bg-accent data-[popup-open]:text-foreground sm:h-8"
        data-testid="home-project"
      >
        <FolderIcon className="size-3.5 shrink-0" />
        <span className="truncate font-medium">
          {current?.name ?? "Choose project"}
        </span>
        <ChevronDownIcon className="size-3.5 shrink-0 opacity-60" />
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-[min(22rem,calc(100vw-2rem))] p-1.5"
        side="bottom"
      >
        {adding ? (
          <AddProjectForm onDone={() => setAdding(false)} />
        ) : (
          <>
            <ul className="flex max-h-72 flex-col overflow-y-auto">
              {projects.map((project) => (
                <li key={project.cwd}>
                  <button
                    className="flex min-h-11 w-full items-center gap-2.5 rounded-md px-2 text-left outline-none hover:bg-accent focus-visible:bg-accent md:min-h-9"
                    onClick={() => {
                      onChoose(project.cwd);
                      setOpen(false);
                    }}
                    title={project.cwd}
                    type="button"
                  >
                    <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] leading-5">
                        {project.name}
                      </span>
                      <span className="block truncate text-[11px] leading-4 text-muted-foreground">
                        {tildify(project.cwd, home)}
                      </span>
                    </span>
                    {project.cwd === current?.cwd ? (
                      <CheckIcon className="size-3.5 shrink-0 text-foreground" />
                    ) : null}
                  </button>
                </li>
              ))}
            </ul>
            <div className="my-1 h-px bg-border" />
            <button
              className="flex min-h-11 w-full items-center gap-2.5 rounded-md px-2 text-left text-[13px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:bg-accent md:min-h-9"
              onClick={() => setAdding(true)}
              type="button"
            >
              <PlusIcon className="size-3.5" />
              Add folder…
            </button>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}

/** Home directory guessed from linked projects, for `~/…` display only. */
function useHomeDir(projects: Project[]): string | null {
  const match = /^\/(?:Users|home)\/[^/]+/.exec(projects[0]?.cwd ?? "");
  return match?.[0] ?? null;
}

function RecentThreads({ projects }: { projects: Project[] }) {
  const sessions = useApp((state) => state.sessions);
  const linked = new Set(projects.map((project) => project.cwd));
  const recent = sessions
    .filter((row) => row.kind !== "subagent" && linked.has(row.cwd))
    .sort((a, b) => Date.parse(b.lastActivity) - Date.parse(a.lastActivity))
    .slice(0, RECENT_ON_HOME);
  if (recent.length === 0) return null;
  return (
    <section className="mt-10 animate-row-in [animation-delay:60ms]">
      <h2 className="mb-1.5 px-1 text-xs font-medium text-muted-foreground">
        Recent
      </h2>
      <ul className="flex flex-col">
        {recent.map((row) => (
          <li key={row.id}>
            <a
              className="group flex min-h-11 items-center gap-3 rounded-lg px-2 outline-none transition-colors hover:bg-accent focus-visible:bg-accent md:min-h-9"
              href={`#/s/${row.id}`}
            >
              <span
                aria-hidden
                className={cn(
                  "size-1.5 shrink-0 rounded-full",
                  row.waitingAskuser ? "bg-warning" : "bg-muted-foreground/30",
                )}
              />
              <span className="min-w-0 flex-1 truncate text-sm">
                {row.title ?? row.preview ?? row.id.slice(0, 8)}
              </span>
              <span className="hidden shrink-0 truncate text-xs text-muted-foreground sm:block">
                {projectName(row.cwd)}
              </span>
              <span className="w-10 shrink-0 text-end text-xs tabular-nums text-muted-foreground/80">
                {timeLabel(row.lastActivity)}
              </span>
            </a>
          </li>
        ))}
      </ul>
    </section>
  );
}
