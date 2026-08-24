/**
 * Composer: auto-growing message input with model picker, thinking level,
 * context meter, and send modes. While streaming, offers steer/queue plus
 * abort. Agent state refreshes on mount and on SSE agent events — no polling.
 */
import { ChevronDownIcon, CircleStopIcon, ArrowUpIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { api } from "~/api/client";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "~/components/ui/popover";
import { useApp } from "~/store";
import { cn } from "~/lib/utils";

const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high"] as const;

export function Composer({
  sessionId,
  targetLabel,
}: {
  sessionId: string;
  targetLabel: string;
}) {
  const streaming = useApp((state) =>
    state.agent?.sessionId === sessionId ? state.streaming : false,
  );
  const sendMode = useApp((state) => state.sendMode);
  const agent = useApp((state) =>
    state.agent?.sessionId === sessionId ? state.agent : null,
  );
  const contextPercent = useApp((state) =>
    state.agent?.sessionId === sessionId ? state.contextPercent : null,
  );
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [expanded, setExpanded] = useState(false);

  const autogrow = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    const cap = window.matchMedia("(max-width: 639px)").matches ? 128 : 240;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, cap)}px`;
  }, []);

  // Attach once per thread; SSE agent events drive further updates (see events.ts).
  useEffect(() => {
    let cancelled = false;
    useApp.getState().setAgent(null);
    void api
      .attachAgent(sessionId)
      .then((agentSnapshot) => {
        if (
          !cancelled &&
          agentSnapshot.sessionId === sessionId &&
          useApp.getState().thread?.summary.id === sessionId
        ) {
          useApp.getState().setAgent(agentSnapshot);
        }
      })
      .catch(() => undefined);
    autogrow();
    return () => {
      cancelled = true;
    };
  }, [sessionId, autogrow]);

  /** Clicking anywhere in the shell focuses the input and expands it. */
  function openEditor() {
    textareaRef.current?.focus();
    setExpanded(true);
  }

  function collapseIfEmpty() {
    if (!textareaRef.current?.value.trim()) setExpanded(false);
  }

  function submit() {
    const el = textareaRef.current;
    const text = el?.value.trim();
    if (!el || !text) return;
    el.value = "";
    autogrow();
    void api
      .send(sessionId, text, streaming ? sendMode : "send")
      .then(() => {
        if (isCurrentSession(sessionId)) {
          useApp.getState().setStreaming(true);
        }
      })
      .catch((error: Error) =>
        reportError(error.message),
      );
  }

  async function setThinking(next: string) {
    await api.setThinking(sessionId, next);
    void refreshAgent(sessionId);
  }

  const model = agent?.state.model;
  const thinking = agent?.state.thinkingLevel ?? "medium";
  const pendingCount = agent?.state.pendingMessageCount ?? 0;

  return (
    <div className="min-w-0 shrink-0 overflow-x-clip bg-background pb-safe">
      <div className="mx-auto w-full max-w-[46rem] px-2 py-2 sm:px-3 sm:py-2.5 md:px-6">
        <div
          className={cn(
            "min-w-0 cursor-text rounded-lg border bg-card shadow-xs transition-colors focus-within:border-input sm:rounded-xl",
            expanded && "border-input",
          )}
          onClick={openEditor}
          onFocusCapture={() => setExpanded(true)}
          onBlurCapture={collapseIfEmpty}
        >
          <div className="flex min-w-0 items-center gap-1.5 px-3 pt-2 text-[11px] leading-4 text-muted-foreground sm:px-3.5 sm:pt-2.5">
            <span>To</span>
            <span className="truncate font-medium text-foreground">{targetLabel}</span>
          </div>
          <textarea
            aria-label={`Message ${targetLabel}`}
            className="block max-h-32 min-h-11 w-full resize-none bg-transparent px-3 py-2 text-base leading-6 outline-none placeholder:text-muted-foreground sm:max-h-[240px] sm:px-3.5 sm:pt-3 sm:pb-1 sm:text-[15px] sm:leading-relaxed"
            data-expanded={expanded}
            onChange={autogrow}
            onKeyDown={(event) => {
              if (
                event.key === "Enter" &&
                !event.shiftKey &&
                !event.nativeEvent.isComposing &&
                !window.matchMedia("(pointer: coarse)").matches
              ) {
                event.preventDefault();
                submit();
              }
            }}
            placeholder={streaming ? `Steer ${targetLabel}…` : `Message ${targetLabel}…`}
            ref={(el) => {
              textareaRef.current = el;
              if (el && !expanded) el.style.height = "auto";
            }}
            rows={1}
          />
          <div className="flex min-w-0 items-center gap-1 px-1.5 pt-0.5 pb-1.5 text-xs text-muted-foreground sm:px-2">
            <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden sm:gap-1">
              <ModelPicker
                models={agent?.models ?? []}
                sessionId={sessionId}
                current={model ?? null}
              />

              <Popover>
                <PopoverTrigger
                  render={
                    <Button
                      size="xs"
                      variant="ghost"
                      className="min-w-0 gap-1 rounded-[var(--control-radius)] px-1.5 sm:px-2"
                    />
                  }
                >
                  <span className="truncate">
                    <span className="hidden sm:inline">thinking: </span>
                    {thinking}
                  </span>
                  <ChevronDownIcon className="size-3 opacity-60" />
                </PopoverTrigger>
                <PopoverContent align="start" className="w-44 p-1" side="top">
                  {THINKING_LEVELS.map((level) => (
                    <button
                      className="flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-accent"
                      key={level}
                      onClick={() => void setThinking(level)}
                      type="button"
                    >
                      <span
                        className={cn(
                          "size-1.5 rounded-full",
                          level === thinking ? "bg-primary" : "bg-transparent",
                        )}
                      />
                      {level}
                    </button>
                  ))}
                </PopoverContent>
              </Popover>

              {contextPercent !== null ? <ContextMeter percent={contextPercent} /> : null}
              {pendingCount > 0 ? (
                <span className="hidden shrink-0 tabular-nums sm:inline">
                  {pendingCount} queued
                </span>
              ) : null}
            </div>

            <div className="ms-auto flex shrink-0 items-center gap-0.5">
              {streaming ? (
                <>
                <SendModeChip active={sendMode === "steer"} mode="steer" />
                <SendModeChip active={sendMode === "queue"} mode="queue" />
                <Button
                  aria-label={sendMode === "steer" ? "Steer agent" : "Queue message"}
                  className="shrink-0"
                  onClick={submit}
                  size="icon-sm"
                  title={sendMode === "steer" ? "Steer agent" : "Queue message"}
                >
                  <ArrowUpIcon />
                </Button>
                <Button
                  aria-label="Abort"
                  className="text-destructive"
                  onClick={() =>
                    void api
                      .abort(sessionId)
                      .then(() => {
                        if (!isCurrentSession(sessionId)) return;
                        useApp.getState().setStreaming(false);
                        refreshAgent(sessionId);
                      })
                      .catch((error: Error) => reportError(error.message))
                  }
                  size="icon-sm"
                  title="Abort"
                  variant="ghost"
                >
                  <CircleStopIcon />
                </Button>
                </>
              ) : (
                <>
                  <span className="hidden pr-1 sm:inline">↵ send</span>
                <Button
                  aria-label="Send"
                  className="shrink-0"
                  onClick={submit}
                  size="icon-sm"
                  title="Send"
                >
                  <ArrowUpIcon />
                </Button>
                </>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function ModelPicker({
  models,
  sessionId,
  current,
}: {
  models: Array<{ id: string; name?: string | null; provider: string }>;
  sessionId: string;
  current: { id: string; provider: string; displayName?: string | null } | null;
}) {
  const [query, setQuery] = useState("");
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? models.filter((model) =>
          `${model.name ?? model.id} ${model.provider}`
            .toLowerCase()
            .includes(q),
        )
      : models;
    return list.slice(0, 60);
  }, [models, query]);

  async function pick(candidate: { id: string; provider: string }) {
    await api.setModel(sessionId, candidate.provider, candidate.id);
    void refreshAgent(sessionId);
  }

  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button
            size="xs"
            variant="ghost"
            className="min-w-0 max-w-[8.5rem] gap-1 rounded-[var(--control-radius)] px-1.5 sm:max-w-none sm:px-2"
          />
        }
      >
        <span className="truncate">
          {current ? shortModel(current.displayName ?? current.id) : "model"}
        </span>
        <ChevronDownIcon className="size-3 opacity-60" />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-1" side="top">
        <div className="sticky top-0 bg-popover p-1 pb-1.5">
          <Input
            autoFocus
            className="h-7 text-[13px]"
            onChange={(event) => setQuery(event.target.value)}
            placeholder={`Filter ${models.length} models…`}
            value={query}
          />
        </div>
        <div className="max-h-64 overflow-y-auto">
          {filtered.map((candidate) => (
            <button
              className="flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-accent"
              key={`${candidate.provider}:${candidate.id}`}
              onClick={() => void pick(candidate)}
              type="button"
            >
              <span
                className={cn(
                  "size-1.5 shrink-0 translate-y-[-1px] rounded-full",
                  candidate.id === current?.id ? "bg-primary" : "bg-transparent",
                )}
              />
              <span className="truncate">{candidate.name ?? candidate.id}</span>
              <span className="ms-auto shrink-0 text-[11px] text-muted-foreground">
                {candidate.provider}
              </span>
            </button>
          ))}
          {filtered.length === 0 ? (
            <p className="px-2 py-3 text-center text-xs text-muted-foreground">
              No models match “{query}”
            </p>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function ContextMeter({ percent }: { percent: number }) {
  const clamped = Math.min(100, Math.max(0, Math.round(percent)));
  return (
    <span className="flex shrink-0 items-center gap-1 tabular-nums">
      <span className="hidden h-[3px] w-12 overflow-hidden rounded-full bg-border sm:block">
        <span
          className={cn(
            "block h-full rounded-full",
            clamped >= 80 ? "bg-warning" : "bg-muted-foreground/70",
          )}
          style={{ width: `${clamped}%` }}
        />
      </span>
      {clamped}%
    </span>
  );
}

function SendModeChip({
  active,
  mode,
}: {
  active: boolean;
  mode: "steer" | "queue";
}) {
  const setSendMode = useApp((state) => state.setSendMode);
  return (
    <Button
      className="rounded-[var(--control-radius)]"
      onClick={() => setSendMode(mode)}
      size="xs"
      title={
        mode === "steer"
          ? "Deliver right after the current tool calls finish"
          : "Wait until the agent finishes, then deliver"
      }
      variant={active ? "secondary" : "ghost"}
    >
      {mode}
    </Button>
  );
}

function refreshAgent(sessionId: string) {
  window.dispatchEvent(new CustomEvent("pecan:agent-refresh", { detail: sessionId }));
}

function isCurrentSession(sessionId: string) {
  const state = useApp.getState();
  return (
    state.thread?.summary.id === sessionId && state.agent?.sessionId === sessionId
  );
}

function reportError(message: string) {
  window.dispatchEvent(new CustomEvent("pecan:error", { detail: message }));
}

/** "anthropic/claude-sonnet-4" → "claude-sonnet-4". */
function shortModel(model: string) {
  return model.split("/").at(-1) ?? model;
}
