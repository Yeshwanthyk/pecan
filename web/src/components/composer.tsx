/**
 * Composer: auto-growing message input with model picker, thinking level,
 * context meter, and send modes. While streaming, offers steer/queue plus
 * abort. Agent state refreshes on mount and on SSE agent events — no polling.
 */
import {
  ChevronDownIcon,
  CircleStopIcon,
  ArrowUpIcon,
  ImagePlusIcon,
  XIcon,
} from "lucide-react";
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
import { clearDraft, readDraft, writeDraft } from "~/lib/drafts";
import { reportError } from "~/lib/errors";
import { shortModel } from "~/lib/format";
import { compressImageForUpload } from "~/lib/image";
import { readPromptHistory, recordPrompt } from "~/lib/prompt-history";
import { cn } from "~/lib/utils";

const BASE_THINKING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
] as const;
const EXTENDED_THINKING_LEVELS = ["xhigh", "max"] as const;

/** One staged image attachment awaiting send. */
type ComposerImage = {
  id: string;
  data: string;
  mimeType: string;
  previewUrl: string;
};

/// Maximum staged attachments; the server enforces the same cap.
const MAX_ATTACHMENTS = 4;
/// Reject files over ~8 MB before reading them.
const MAX_FILE_BYTES = 8 * 1024 * 1024;

/** Reads image files into staged attachments (base64 + object-URL preview), downscaling oversized photos first. */
function readImageFiles(
  files: FileList | File[] | null,
  onError: (message: string) => void,
): Promise<ComposerImage[]> {
  const incoming = Array.from(files ?? []).filter((file) =>
    file.type.startsWith("image/"),
  );
  if (incoming.length === 0) return Promise.resolve([]);
  return Promise.all(
    incoming.slice(0, MAX_ATTACHMENTS).map(async (file) => {
      const staged = await compressImageForUpload(file);
      if (staged.size > MAX_FILE_BYTES) {
        onError(`${file.name}: image too large (max 8 MB)`);
        return null;
      }
      return new Promise<ComposerImage | null>((resolve) => {
        const reader = new FileReader();
        reader.onload = () => {
          const dataUrl =
            typeof reader.result === "string" ? reader.result : "";
          const comma = dataUrl.indexOf(",");
          if (comma < 0) {
            resolve(null);
            return;
          }
          resolve({
            id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
            data: dataUrl.slice(comma + 1),
            mimeType: staged.type || "image/png",
            previewUrl: URL.createObjectURL(staged),
          });
        };
        reader.onerror = () => resolve(null);
        reader.readAsDataURL(staged);
      });
    }),
  ).then((images) =>
    images.filter((image): image is ComposerImage => image !== null),
  );
}

/** Extended levels are model-gated: pi exposes them in `thinkingLevelMap`. */
function availableThinkingLevels(
  model: { thinkingLevelMap?: unknown } | undefined | null,
): string[] {
  const supported = model?.thinkingLevelMap;
  const extended =
    supported !== null && typeof supported === "object"
      ? EXTENDED_THINKING_LEVELS.filter(
          (level) => level in (supported as object),
        )
      : [];
  return [...BASE_THINKING_LEVELS, ...extended];
}

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
  const extensionEditorText = useApp(
    (state) => state.extensionEditorText[sessionId],
  );
  const appliedEditorRevision = useRef(0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [images, setImages] = useState<ComposerImage[]>([]);
  const draftTimerRef = useRef<number | null>(null);
  // -1 = not browsing prompt history; otherwise an index into readPromptHistory() (0 = most recent).
  const historyIndexRef = useRef(-1);
  const historyDraftRef = useRef("");

  const scheduleDraftSave = useCallback(() => {
    if (draftTimerRef.current !== null)
      window.clearTimeout(draftTimerRef.current);
    draftTimerRef.current = window.setTimeout(() => {
      draftTimerRef.current = null;
      writeDraft(sessionId, textareaRef.current?.value ?? "");
    }, 300);
  }, [sessionId]);

  const addFiles = useCallback((files: FileList | File[] | null) => {
    void readImageFiles(files, reportError).then((staged) => {
      if (staged.length === 0) return;
      setImages((current) => [...current, ...staged].slice(0, MAX_ATTACHMENTS));
    });
  }, []);

  function removeImage(id: string) {
    setImages((current) => {
      const target = current.find((image) => image.id === id);
      if (target) URL.revokeObjectURL(target.previewUrl);
      return current.filter((image) => image.id !== id);
    });
  }

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
    // Restore a persisted draft before the extension-prefill effect below runs,
    // so set_editor_text still wins whenever it actually fires.
    const draft = readDraft(sessionId);
    if (draft && textareaRef.current) {
      textareaRef.current.value = draft;
      setExpanded(true);
    }
    autogrow();
    return () => {
      cancelled = true;
      if (draftTimerRef.current !== null)
        window.clearTimeout(draftTimerRef.current);
    };
  }, [sessionId, autogrow]);

  // Pi extensions can replace the composer text through set_editor_text.
  useEffect(() => {
    if (
      !extensionEditorText ||
      extensionEditorText.revision <= appliedEditorRevision.current
    )
      return;
    appliedEditorRevision.current = extensionEditorText.revision;
    if (!textareaRef.current) return;
    textareaRef.current.value = extensionEditorText.text;
    autogrow();
    setExpanded(extensionEditorText.text.length > 0);
  }, [autogrow, extensionEditorText]);

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
    const text = el?.value.trim() ?? "";
    if (!el || (!text && images.length === 0)) return;
    el.value = "";
    autogrow();
    if (draftTimerRef.current !== null) {
      window.clearTimeout(draftTimerRef.current);
      draftTimerRef.current = null;
    }
    historyIndexRef.current = -1;
    if (text) recordPrompt(text);
    const attachments = images.map(({ data, mimeType }) => ({
      data,
      mimeType,
    }));
    for (const image of images) URL.revokeObjectURL(image.previewUrl);
    setImages([]);
    void api
      .sendImages(sessionId, text, streaming ? sendMode : "send", attachments)
      .then(() => {
        clearDraft(sessionId);
        if (isCurrentSession(sessionId)) {
          useApp.getState().setStreaming(true);
        }
      })
      .catch((error: Error) => {
        reportError(error.message);
        // Give the words back rather than losing them.
        const current = textareaRef.current;
        if (isCurrentSession(sessionId) && current && !current.value.trim()) {
          current.value = text;
          autogrow();
          setExpanded(true);
        }
      });
  }

  function abort() {
    void api
      .abort(sessionId)
      .then(() => {
        if (!isCurrentSession(sessionId)) return;
        useApp.getState().setStreaming(false);
        refreshAgent(sessionId);
      })
      .catch((error: Error) => reportError(error.message));
  }

  async function setThinking(next: string) {
    await api.setThinking(sessionId, next);
    void refreshAgent(sessionId);
  }

  const model = agent?.state.model;
  const thinking = agent?.state.thinkingLevel ?? "medium";
  const thinkingLevels = availableThinkingLevels(model);
  const pendingCount = agent?.state.pendingMessageCount ?? 0;

  return (
    <div className="min-w-0 shrink-0 overflow-x-clip bg-background pb-safe">
      <div className="mx-auto w-full max-w-[46rem] px-2 py-2 sm:px-3 sm:py-2.5 md:px-6">
        <div
          className={cn(
            "min-w-0 cursor-text rounded-2xl border bg-card shadow-[0_1px_2px_rgb(0_0_0/4%),0_6px_24px_-8px_rgb(0_0_0/10%)] transition-colors focus-within:border-input",
            expanded && "border-input",
          )}
          onClick={openEditor}
          onFocusCapture={() => setExpanded(true)}
          onBlurCapture={collapseIfEmpty}
        >
          {targetLabel === "main thread" ? null : (
            <div className="flex min-w-0 items-center gap-1.5 px-3 pt-2 text-[11px] leading-4 text-muted-foreground sm:px-3.5 sm:pt-2.5">
              <span>To</span>
              <span className="truncate font-medium text-foreground">
                {targetLabel}
              </span>
            </div>
          )}
          <textarea
            aria-label={`Message ${targetLabel}`}
            data-testid="composer-input"
            className="block max-h-32 min-h-11 w-full resize-none bg-transparent px-3 py-2 text-base leading-6 outline-none placeholder:text-muted-foreground sm:max-h-[240px] sm:px-3.5 sm:pt-3 sm:pb-1 sm:text-[15px] sm:leading-relaxed"
            data-expanded={expanded}
            onChange={() => {
              // A genuine edit exits any in-progress history browse; a
              // programmatic value set (recall, extension prefill, draft
              // restore) never fires a React change event.
              historyIndexRef.current = -1;
              autogrow();
              scheduleDraftSave();
            }}
            onPaste={(event) => {
              const files = event.clipboardData?.files;
              if (files && files.length > 0) {
                event.preventDefault();
                addFiles(files);
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape" && streaming) {
                event.preventDefault();
                abort();
                return;
              }
              if (
                event.key === "Enter" &&
                !event.shiftKey &&
                !event.nativeEvent.isComposing &&
                !window.matchMedia("(pointer: coarse)").matches
              ) {
                event.preventDefault();
                submit();
                return;
              }
              if (event.key === "ArrowUp" || event.key === "ArrowDown") {
                const el = event.currentTarget;
                const browsing = historyIndexRef.current >= 0;
                const atEmptyStart =
                  el.value.length === 0 &&
                  el.selectionStart === 0 &&
                  el.selectionEnd === 0;
                if (!browsing && !atEmptyStart) return;
                const history = readPromptHistory();
                if (event.key === "ArrowUp") {
                  if (history.length === 0) return;
                  event.preventDefault();
                  if (!browsing) historyDraftRef.current = el.value;
                  historyIndexRef.current = Math.min(
                    historyIndexRef.current + 1,
                    history.length - 1,
                  );
                  el.value = history[historyIndexRef.current] ?? "";
                } else {
                  if (!browsing) return;
                  event.preventDefault();
                  historyIndexRef.current -= 1;
                  el.value =
                    historyIndexRef.current >= 0
                      ? (history[historyIndexRef.current] ?? "")
                      : historyDraftRef.current;
                }
                autogrow();
                setExpanded(true);
                el.setSelectionRange(el.value.length, el.value.length);
              }
            }}
            placeholder={
              targetLabel === "main thread"
                ? streaming
                  ? "Steer Pi…"
                  : "Ask Pi anything…"
                : streaming
                  ? `Steer ${targetLabel}…`
                  : `Message ${targetLabel}…`
            }
            ref={(el) => {
              textareaRef.current = el;
              if (el && !expanded) el.style.height = "auto";
            }}
            rows={1}
          />
          <div className="flex min-w-0 items-center gap-1 px-1.5 pt-0.5 pb-1.5 text-xs text-muted-foreground sm:px-2">
            <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden sm:gap-1">
              <input
                accept="image/*"
                className="hidden"
                multiple
                onChange={(event) => {
                  addFiles(event.target.files);
                  event.target.value = "";
                }}
                ref={fileInputRef}
                type="file"
              />
              <Button
                aria-label="Attach image"
                className="size-7 shrink-0 rounded-[var(--control-radius)]"
                disabled={images.length >= MAX_ATTACHMENTS}
                onClick={() => fileInputRef.current?.click()}
                size="icon"
                variant="ghost"
              >
                <ImagePlusIcon className="size-3.5" />
              </Button>
              <ModelPicker
                models={agent?.models ?? []}
                favorites={agent?.favorites ?? []}
                sessionId={sessionId}
                current={model ?? null}
              />

              <Popover>
                <PopoverTrigger
                  render={
                    <Button
                      size="xs"
                      variant="ghost"
                      className="min-w-0 gap-1 rounded-[var(--control-radius)] px-1.5 font-normal text-muted-foreground hover:text-foreground sm:px-2"
                    />
                  }
                >
                  <span className="truncate">
                    <span className="hidden sm:inline">
                      Thinking {thinking}
                    </span>
                    <span className="sm:hidden">{shortThinking(thinking)}</span>
                  </span>
                  <ChevronDownIcon className="size-3 opacity-60" />
                </PopoverTrigger>
                <PopoverContent align="start" className="w-44 p-1" side="top">
                  {thinkingLevels.map((level) => (
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

              {contextPercent !== null && contextPercent >= 1 ? (
                <ContextMeter percent={contextPercent} />
              ) : null}
              {pendingCount > 0 ? (
                <span className="hidden shrink-0 tabular-nums sm:inline">
                  {pendingCount} queued
                </span>
              ) : null}
            </div>

            <div className="ms-auto flex shrink-0 items-center gap-0.5">
              {streaming ? (
                <>
                  <SendModeToggle mode={sendMode} />
                  <Button
                    aria-label={
                      sendMode === "steer" ? "Steer agent" : "Queue message"
                    }
                    className="shrink-0"
                    data-testid="send"
                    onClick={submit}
                    size="icon-sm"
                    title={
                      sendMode === "steer" ? "Steer agent" : "Queue message"
                    }
                  >
                    <ArrowUpIcon />
                  </Button>
                  <Button
                    aria-label="Abort"
                    className="text-destructive"
                    data-testid="abort"
                    onClick={abort}
                    size="icon-sm"
                    title="Stop (Esc)"
                    variant="ghost"
                  >
                    <CircleStopIcon />
                  </Button>
                </>
              ) : (
                <>
                  <span className="hidden pr-1 text-[11px] opacity-70 lg:inline">
                    ↵ send
                  </span>
                  <Button
                    aria-label="Send"
                    className="shrink-0"
                    data-testid="send"
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
          {images.length > 0 ? (
            <div className="flex flex-wrap gap-1.5 px-2 pb-2 sm:px-2.5">
              {images.map((image) => (
                <span
                  className="relative overflow-hidden rounded-md border border-border"
                  key={image.id}
                >
                  <img
                    alt="attachment preview"
                    className="size-12 object-cover"
                    src={image.previewUrl}
                  />
                  <button
                    aria-label="Remove attachment"
                    className="absolute end-0.5 top-0.5 flex size-4 items-center justify-center rounded-full bg-background/80 text-foreground"
                    onClick={() => removeImage(image.id)}
                    type="button"
                  >
                    <XIcon className="size-3" />
                  </button>
                </span>
              ))}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}

type PickerModel = { id: string; name?: string | null; provider: string };

/**
 * Resolves pi `enabledModels` patterns (`provider/id` or bare `id`, with an
 * optional `:thinking` suffix) to available models, keeping pattern order.
 * Glob patterns are skipped.
 */
function favoriteModels(
  models: PickerModel[],
  patterns: string[],
): PickerModel[] {
  const picked: PickerModel[] = [];
  for (const raw of patterns) {
    const pattern = raw.replace(
      /:(off|minimal|low|medium|high|xhigh|max)$/,
      "",
    );
    if (/[*?]/.test(pattern)) continue;
    const slash = pattern.indexOf("/");
    const match =
      slash > 0
        ? models.find(
            (m) =>
              m.provider === pattern.slice(0, slash) &&
              m.id === pattern.slice(slash + 1),
          )
        : models.find((m) => m.id === pattern);
    if (match && !picked.includes(match)) picked.push(match);
  }
  return picked;
}

function ModelPicker({
  models,
  favorites,
  sessionId,
  current,
}: {
  models: PickerModel[];
  favorites: string[];
  sessionId: string;
  current: { id: string; provider: string; displayName?: string | null } | null;
}) {
  const [query, setQuery] = useState("");
  const favs = useMemo(
    () => favoriteModels(models, favorites),
    [models, favorites],
  );
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? models.filter((model) =>
          `${model.name ?? model.id} ${model.provider}`
            .toLowerCase()
            .includes(q),
        )
      : models.filter((model) => !favs.includes(model));
    return list.slice(0, 60);
  }, [models, favs, query]);
  const sections: Array<{ label: string | null; items: PickerModel[] }> =
    query.trim() || favs.length === 0
      ? [{ label: null, items: filtered }]
      : [
          { label: "Favorites", items: favs },
          { label: "All models", items: filtered },
        ];

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
            className="min-w-0 max-w-[8.5rem] gap-1 rounded-[var(--control-radius)] px-1.5 font-normal text-muted-foreground hover:text-foreground sm:max-w-none sm:px-2"
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
          {sections.map((section) => (
            <div key={section.label ?? "all"}>
              {section.label ? (
                <p className="px-2 pt-1.5 pb-0.5 text-[11px] font-medium text-muted-foreground">
                  {section.label}
                </p>
              ) : null}
              {section.items.map((candidate) => (
                <button
                  className="flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left text-[13px] hover:bg-accent"
                  key={`${candidate.provider}:${candidate.id}`}
                  onClick={() => void pick(candidate)}
                  type="button"
                >
                  <span
                    className={cn(
                      "size-1.5 shrink-0 translate-y-[-1px] rounded-full",
                      candidate.id === current?.id &&
                        candidate.provider === current?.provider
                        ? "bg-primary"
                        : "bg-transparent",
                    )}
                  />
                  <span className="truncate">
                    {candidate.name ?? candidate.id}
                  </span>
                  <span className="ms-auto shrink-0 text-[11px] text-muted-foreground">
                    {candidate.provider}
                  </span>
                </button>
              ))}
            </div>
          ))}
          {query.trim() && filtered.length === 0 ? (
            <p className="px-2 py-3 text-center text-xs text-muted-foreground">
              No models match “{query}”
            </p>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** Phone label for a thinking level: "medium" → "med", "xhigh" → "xhigh". */
function shortThinking(level: string): string {
  return level === "medium" ? "med" : level === "minimal" ? "min" : level;
}

function ContextMeter({ percent }: { percent: number }) {
  const clamped = Math.min(100, Math.max(0, Math.round(percent)));
  return (
    <span
      className={cn(
        "shrink-0 items-center gap-1 tabular-nums",
        // Phones only need the meter once the window is nearly full.
        clamped >= 80 ? "flex text-warning" : "hidden sm:flex",
      )}
      title={`${clamped}% of the context window used`}
    >
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

/** One chip that flips how a message sent mid-turn is delivered. */
function SendModeToggle({ mode }: { mode: "steer" | "queue" }) {
  const setSendMode = useApp((state) => state.setSendMode);
  const next = mode === "steer" ? "queue" : "steer";
  return (
    <Button
      aria-label={`Delivery: ${mode}. Switch to ${next}`}
      className="rounded-[var(--control-radius)] px-2 font-normal text-muted-foreground hover:text-foreground"
      onClick={() => setSendMode(next)}
      size="xs"
      title={
        mode === "steer"
          ? "Steer: delivered right after the current tool calls finish. Tap to queue instead."
          : "Queue: delivered once the agent finishes. Tap to steer instead."
      }
      variant="ghost"
    >
      {mode}
    </Button>
  );
}

function refreshAgent(sessionId: string) {
  window.dispatchEvent(
    new CustomEvent("pecan:agent-refresh", { detail: sessionId }),
  );
}

function isCurrentSession(sessionId: string) {
  const state = useApp.getState();
  return (
    state.thread?.summary.id === sessionId &&
    state.agent?.sessionId === sessionId
  );
}
