/**
 * Global host for live extension dialogs (select / confirm / input / editor).
 *
 * pi emits these as `extension_ui_request` events; the server records the
 * blocking ones and the client keeps them in `pendingAsks` keyed by session,
 * correlating every card here by the exact pi request id. Answering or
 * cancelling sends an `extension_ui_response` with that same id. The host
 * renders above the composer (never inside transcript cards) and hides once
 * the thread settles.
 */
import { XIcon } from "lucide-react";
import { useState } from "react";
import { api } from "~/api/client";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { Spinner } from "~/components/ui/spinner";
import { Textarea } from "~/components/ui/textarea";
import { syncPendingAsks } from "~/events";
import { useApp, type DialogOption, type PendingAsk } from "~/store";

/** Stable empty list so the selector never returns a fresh reference. */
const NO_ASKS: PendingAsk[] = [];

export function ExtensionHost({
  sessionId,
  settled,
}: {
  sessionId: string;
  settled: boolean;
}) {
  const asks = useApp((state) => state.pendingAsks[sessionId] ?? NO_ASKS);
  if (asks.length === 0 || settled) return null;
  return (
    <div className="mx-auto w-full max-w-[46rem] px-4 pb-1 md:px-6">
      <div className="flex flex-col gap-2">
        {asks.map((ask) => (
          <DialogCard key={ask.id} ask={ask} sessionId={sessionId} />
        ))}
      </div>
    </div>
  );
}

const METHOD_LABELS: Record<PendingAsk["method"], string> = {
  select: "Select an option",
  confirm: "Confirmation",
  input: "Enter a value",
  editor: "Edit text",
};

type AnswerPayload = { cancelled?: boolean; confirmed?: boolean; value?: string };

function DialogCard({ ask, sessionId }: { ask: PendingAsk; sessionId: string }) {
  const removePendingAsk = useApp((state) => state.removePendingAsk);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function answer(payload: AnswerPayload) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.respond(sessionId, ask.id, payload);
      removePendingAsk(sessionId, ask.id);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(`Answer wasn't sent: ${message}`);
      window.dispatchEvent(new CustomEvent("pecan:error", { detail: message }));
      // The ask may be stale (worker restarted); re-sync from the server.
      void syncPendingAsks(sessionId);
      setBusy(false);
    }
  }

  return (
    <div
      className="rounded-xl border border-warning/40 bg-warning/[0.06] p-3.5 text-foreground"
      role="dialog"
      aria-label={ask.title ?? METHOD_LABELS[ask.method]}
    >
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="text-[11px] font-medium text-warning">
            {busy ? (
              <span className="flex items-center gap-1.5">
                <Spinner className="size-3" /> Sending…
              </span>
            ) : (
              METHOD_LABELS[ask.method]
            )}
          </div>
          {ask.title ? (
            <p className="mt-1 text-sm font-medium">{ask.title}</p>
          ) : null}
        </div>
        <Button
          aria-label="Cancel this request"
          className="-mt-1 -me-1 shrink-0"
          disabled={busy}
          onClick={() => void answer({ cancelled: true })}
          size="icon-micro"
          title="Cancel"
          variant="ghost"
        >
          <XIcon />
        </Button>
      </div>
      <DialogBody answer={answer} ask={ask} busy={busy} />
      {error ? (
        <p className="mt-2 border-t pt-2 text-xs text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function DialogBody({
  ask,
  busy,
  answer,
}: {
  ask: PendingAsk;
  busy: boolean;
  answer: (payload: AnswerPayload) => void;
}) {
  if (ask.method === "confirm") {
    return (
      <div className="mt-2 flex flex-col gap-2">
        {ask.message ? (
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            {ask.message}
          </p>
        ) : null}
        <div className="flex gap-2">
          <Button disabled={busy} onClick={() => answer({ confirmed: true })} size="xs">
            Yes
          </Button>
          <Button
            disabled={busy}
            onClick={() => answer({ confirmed: false })}
            size="xs"
            variant="ghost"
          >
            No
          </Button>
        </div>
      </div>
    );
  }
  if (ask.method === "select") {
    return (
      <ul className="mt-2 flex flex-col gap-1">
        {(ask.options ?? []).map((option, index) => (
          <li key={optionKey(option, index)}>
            <button
              className="w-full rounded-lg border bg-card px-3 py-1.5 text-left text-[13px] transition-colors hover:border-input hover:bg-accent disabled:opacity-50"
              disabled={busy}
              onClick={() => answer({ value: optionValue(option) })}
              type="button"
            >
              {optionLabel(option)}
            </button>
          </li>
        ))}
      </ul>
    );
  }
  if (ask.method === "input") {
    return (
      <TextAnswer
        busy={busy}
        onAnswer={(value) => answer({ value })}
        placeholder={ask.placeholder}
      />
    );
  }
  return (
    <EditorAnswer busy={busy} onAnswer={(value) => answer({ value })} prefill={ask.prefill} />
  );
}

function TextAnswer({
  busy,
  onAnswer,
  placeholder,
}: {
  busy: boolean;
  onAnswer: (value: string) => void;
  placeholder?: string;
}) {
  const [value, setValue] = useState("");
  const trimmed = value.trim();
  return (
    <form
      className="mt-2 flex gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (trimmed) onAnswer(trimmed);
      }}
    >
      <Input
        className="h-8 flex-1 text-[13px]"
        disabled={busy}
        onChange={(event) => setValue(event.target.value)}
        placeholder={placeholder || "Type your answer…"}
        value={value}
      />
      <Button disabled={busy || !trimmed} size="xs" type="submit">
        Send
      </Button>
    </form>
  );
}

function EditorAnswer({
  busy,
  onAnswer,
  prefill,
}: {
  busy: boolean;
  onAnswer: (value: string) => void;
  prefill?: string;
}) {
  const [value, setValue] = useState(prefill ?? "");
  return (
    <form
      className="mt-2 flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        onAnswer(value);
      }}
    >
      <Textarea
        className="min-h-24 text-[13px]"
        disabled={busy}
        onChange={(event) => setValue(event.target.value)}
        placeholder="Type or edit…"
        value={value}
      />
      <div className="flex justify-end gap-2">
        <Button disabled={busy} size="xs" type="submit">
          Save
        </Button>
      </div>
    </form>
  );
}

/** Exact wire value for one select choice: string options answer as-is,
 * object options answer with `value` (falling back to the label). */
function optionValue(option: DialogOption): string {
  return typeof option === "string" ? option : (option.value ?? option.label ?? "");
}

/** Display label for one select choice. */
function optionLabel(option: DialogOption): string {
  return typeof option === "string" ? option : (option.label ?? option.value ?? "…");
}

function optionKey(option: DialogOption, index: number): string {
  return `${optionLabel(option)}-${index}`;
}