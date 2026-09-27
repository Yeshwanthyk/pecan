/** Lazy-loaded, bounded workspace review panel powered by Pierre Diffs. */
import { parsePatchFiles, setLanguageOverride } from "@pierre/diffs";
import { FileDiff } from "@pierre/diffs/react";
import { Columns2Icon, FileDiffIcon, Rows3Icon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

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

  function chooseStyle(next: DiffStyle) {
    setStyle(next);
    writeLocalStorage(DIFF_STYLE_KEY, next);
  }

  return (
    <Sheet onOpenChange={onOpenChange} open={open}>
      <SheetPopup
        className="w-full max-w-none sm:w-[min(88vw,72rem)] sm:max-w-none"
        side="right"
      >
        <SheetHeader className="shrink-0 gap-1 border-b px-3 py-3 pe-12 sm:px-4">
          <div className="flex min-w-0 items-center gap-2">
            <FileDiffIcon className="size-4 shrink-0 text-muted-foreground" />
            <SheetTitle className="truncate text-sm leading-5">Workspace diff</SheetTitle>
            {data?.branch ? (
              <span className="truncate font-mono text-[11px] text-muted-foreground">
                {data.branch}
              </span>
            ) : null}
            <div className="ms-auto hidden shrink-0 items-center rounded-md bg-muted p-0.5 sm:flex">
              <StyleButton active={style === "split"} onClick={() => chooseStyle("split")}>
                <Columns2Icon /> Split
              </StyleButton>
              <StyleButton active={style === "unified"} onClick={() => chooseStyle("unified")}>
                <Rows3Icon /> Unified
              </StyleButton>
            </div>
          </div>
          <SheetDescription className="flex min-w-0 items-center gap-1.5 text-[11px]">
            <span>{data ? `${data.files} ${data.files === 1 ? "file" : "files"}` : "Current checkout"}</span>
            {data?.untracked ? <span>· {data.untracked} untracked</span> : null}
            {data?.truncated ? <span className="text-warning">· truncated</span> : null}
            <span>· includes all local workspace changes</span>
          </SheetDescription>
          <div className="mt-1 flex w-fit items-center rounded-md bg-muted p-0.5 sm:hidden">
            <StyleButton active={style === "unified"} onClick={() => chooseStyle("unified")}>
              <Rows3Icon /> Unified
            </StyleButton>
            <StyleButton active={style === "split"} onClick={() => chooseStyle("split")}>
              <Columns2Icon /> Split
            </StyleButton>
          </div>
        </SheetHeader>

        <div className="min-h-0 flex-1 overflow-auto bg-background">
          {loading ? (
            <DiffLoading />
          ) : error ? (
            <div className="mx-auto max-w-lg p-6 text-sm text-destructive">{error}</div>
          ) : files.length > 0 ? (
            <div className="space-y-px bg-border/50 pb-8">
              {files.map((file, index) => (
                <FileDiff
                  className="min-w-0 bg-background"
                  fileDiff={file}
                  key={`${file.prevName ?? file.name}:${file.name}:${index}`}
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
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Button
      className={cn("h-7 gap-1 border-0 px-2 text-[11px]", active && "shadow-xs")}
      onClick={onClick}
      size="compact"
      variant={active ? "outline" : "ghost-muted"}
    >
      {children}
    </Button>
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
