/** Lazy-loaded, bounded workspace review panel powered by Pierre Diffs. */
import {
  type FileDiffMetadata,
  parsePatchFiles,
  setLanguageOverride,
} from "@pierre/diffs";
import { FileDiff } from "@pierre/diffs/react";
import {
  ChevronRightIcon,
  Columns2Icon,
  GitBranchIcon,
  Rows3Icon,
  XIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useState } from "react";

import { api } from "~/api/client";
import type { WorkspaceDiff } from "~/api/types";
import { Button } from "~/components/ui/button";
import {
  Sheet,
  SheetDescription,
  SheetHeader,
  SheetPopup,
  SheetTitle,
} from "~/components/ui/sheet";
import { useApp } from "~/store";
import { readLocalStorage, writeLocalStorage } from "~/lib/storage";
import { cn } from "~/lib/utils";

type DiffStyle = "split" | "unified";
const DIFF_STYLE_KEY = "pecan:diff-style";

export default function DiffSidebar({
  sessionId,
  open,
  onOpenChange,
}: {
  sessionId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const theme = useApp((state) => state.theme);
  const [data, setData] = useState<WorkspaceDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [style, setStyle] = useState<DiffStyle>(initialDiffStyle);
  const files = useMemo(() => {
    if (!data?.patch) return [];
    return parsePatchFiles(data.patch)
      .flatMap((parsed) => parsed.files)
      .map((file) => setLanguageOverride(file, "text"));
  }, [data?.patch]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    void api
      .workspaceDiff(sessionId)
      .then((next) => {
        if (!cancelled) setData(next);
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, sessionId]);

  const totals = useMemo(() => {
    let additions = 0;
    let deletions = 0;
    for (const file of files) {
      const stats = fileStats(file);
      additions += stats.additions;
      deletions += stats.deletions;
    }
    return { additions, deletions };
  }, [files]);

  function jumpTo(index: number) {
    document
      .getElementById(fileAnchor(index))
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function chooseStyle(next: DiffStyle) {
    setStyle(next);
    writeLocalStorage(DIFF_STYLE_KEY, next);
  }

  return (
    <Sheet onOpenChange={onOpenChange} open={open}>
      <SheetPopup
        className="w-full max-w-none sm:w-[min(88vw,72rem)] sm:max-w-none"
        showCloseButton={false}
        side="right"
      >
        <SheetHeader className="shrink-0 gap-0.5 border-b px-3 py-2.5 sm:px-4">
          <div className="flex min-w-0 items-center gap-2">
            <SheetTitle className="truncate text-sm leading-5">
              Changes
            </SheetTitle>
            {data?.branch ? (
              <span className="flex min-w-0 items-center gap-1 font-mono text-[11px] text-muted-foreground">
                <GitBranchIcon className="size-3 shrink-0 opacity-60" />
                <span className="truncate">{data.branch}</span>
              </span>
            ) : null}
            <div className="ms-auto flex shrink-0 items-center rounded-md bg-muted p-0.5">
              <StyleButton
                active={style === "unified"}
                label="Unified"
                onClick={() => chooseStyle("unified")}
              >
                <Rows3Icon />
              </StyleButton>
              <StyleButton
                active={style === "split"}
                label="Split"
                onClick={() => chooseStyle("split")}
              >
                <Columns2Icon />
              </StyleButton>
            </div>
            <Button
              aria-label="Close"
              className="shrink-0"
              onClick={() => onOpenChange(false)}
              size="icon"
              variant="ghost"
            >
              <XIcon />
            </Button>
          </div>
          <SheetDescription className="flex min-w-0 flex-wrap items-center gap-x-1.5 text-[11px]">
            {data ? (
              <>
                <span>
                  {data.files} {data.files === 1 ? "file" : "files"}
                </span>
                {totals.additions + totals.deletions > 0 ? (
                  <LineStats {...totals} />
                ) : null}
                {data.untracked ? (
                  <span>· {data.untracked} untracked</span>
                ) : null}
                {data.truncated ? (
                  <span className="text-warning">· truncated</span>
                ) : null}
              </>
            ) : (
              <span>All local workspace changes</span>
            )}
          </SheetDescription>
        </SheetHeader>

        <div className="min-h-0 flex-1 overflow-auto bg-background">
          {loading ? (
            <DiffLoading />
          ) : error ? (
            <div className="mx-auto max-w-lg p-6 text-sm text-destructive">
              {error}
            </div>
          ) : files.length > 0 ? (
            <div className="space-y-px bg-border/50 pb-8">
              {files.length > 1 ? (
                <FileIndex files={files} onJump={jumpTo} />
              ) : null}
              {files.map((file, index) => (
                <div
                  className="scroll-mt-2"
                  id={fileAnchor(index)}
                  key={`${file.prevName ?? file.name}:${file.name}:${index}`}
                >
                  <FileDiff
                    className="min-w-0 bg-background"
                    fileDiff={file}
                    options={{
                      diffIndicators: "bars",
                      diffStyle: style,
                      hunkSeparators: "line-info",
                      lineDiffType: "word-alt",
                      overflow: "scroll",
                      theme: { dark: "pierre-dark", light: "pierre-light" },
                      themeType: theme === "one-dark" ? "dark" : "light",
                    }}
                  />
                </div>
              ))}
            </div>
          ) : (
            <div className="flex h-full min-h-64 items-center justify-center px-6 text-center">
              <div>
                <p className="text-sm font-medium">No workspace changes</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  The checkout matches HEAD and has no untracked files.
                </p>
              </div>
            </div>
          )}
        </div>
      </SheetPopup>
    </Sheet>
  );
}

function StyleButton({
  active,
  label,
  onClick,
  children,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Button
      aria-label={`${label} diff`}
      aria-pressed={active}
      className={cn(
        "h-7 gap-1 border-0 px-2 text-[11px]",
        active && "shadow-xs",
      )}
      onClick={onClick}
      size="compact"
      title={`${label} diff`}
      variant={active ? "outline" : "ghost-muted"}
    >
      {children}
      <span className="hidden sm:inline">{label}</span>
    </Button>
  );
}

type Stats = { additions: number; deletions: number };

function fileStats(file: FileDiffMetadata): Stats {
  let additions = 0;
  let deletions = 0;
  for (const hunk of file.hunks) {
    additions += hunk.additionLines;
    deletions += hunk.deletionLines;
  }
  return { additions, deletions };
}

function fileAnchor(index: number): string {
  return `diff-file-${index}`;
}

function LineStats({ additions, deletions }: Stats) {
  return (
    <span className="font-mono tabular-nums">
      <span className="text-success">+{additions}</span>{" "}
      <span className="text-destructive">−{deletions}</span>
    </span>
  );
}

const CHANGE_LABEL: Record<FileDiffMetadata["type"], string> = {
  change: "M",
  "rename-pure": "R",
  "rename-changed": "R",
  new: "A",
  deleted: "D",
};

/** Jump list of changed files, collapsed by default when long. */
function FileIndex({
  files,
  onJump,
}: {
  files: FileDiffMetadata[];
  onJump: (index: number) => void;
}) {
  return (
    <details className="group bg-background" open={files.length <= 8}>
      <summary className="flex min-h-10 cursor-pointer list-none items-center gap-1.5 px-3 text-[12px] font-medium text-muted-foreground select-none hover:text-foreground sm:px-4 [&::-webkit-details-marker]:hidden">
        <ChevronRightIcon className="size-3.5 transition-transform group-open:rotate-90" />
        Files
      </summary>
      <ul className="pb-2">
        {files.map((file, index) => {
          const stats = fileStats(file);
          const slash = file.name.lastIndexOf("/");
          return (
            <li key={`${file.name}:${index}`}>
              <button
                className="flex min-h-10 w-full items-center gap-2.5 px-3 text-left text-[12px] outline-none hover:bg-accent focus-visible:bg-accent sm:min-h-8 sm:px-4"
                onClick={() => onJump(index)}
                title={
                  file.prevName ? `${file.prevName} → ${file.name}` : file.name
                }
                type="button"
              >
                <span
                  className={cn(
                    "w-3 shrink-0 text-center font-mono text-[11px] font-semibold",
                    file.type === "new" && "text-success",
                    file.type === "deleted" && "text-destructive",
                    file.type === "change" && "text-warning",
                    file.type.startsWith("rename") && "text-muted-foreground",
                  )}
                >
                  {CHANGE_LABEL[file.type]}
                </span>
                <span className="min-w-0 flex-1 truncate">
                  <span className="font-medium">
                    {file.name.slice(slash + 1)}
                  </span>
                  {slash > 0 ? (
                    <span className="ms-1.5 text-muted-foreground">
                      {file.name.slice(0, slash)}
                    </span>
                  ) : null}
                </span>
                <span className="shrink-0 text-[11px]">
                  <LineStats {...stats} />
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </details>
  );
}

function DiffLoading() {
  return (
    <div className="space-y-2 p-4" aria-label="Loading workspace diff">
      <div className="h-8 animate-pulse rounded-md bg-muted" />
      <div className="h-48 animate-pulse rounded-md bg-muted/70" />
      <div className="h-32 animate-pulse rounded-md bg-muted/50" />
    </div>
  );
}

function initialDiffStyle(): DiffStyle {
  const stored = readLocalStorage(DIFF_STYLE_KEY);
  if (stored === "split" || stored === "unified") return stored;
  return window.matchMedia("(max-width: 767px)").matches ? "unified" : "split";
}
