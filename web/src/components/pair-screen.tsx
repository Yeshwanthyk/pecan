/**
 * Shown instead of the app while this browser is unpaired: one field for the
 * code `pecan pair` prints on the host. Pairing sets an HttpOnly device
 * cookie, so nothing secret is ever stored in page-readable storage.
 */
import { useState } from "react";

import { api, ApiError } from "~/api/client";
import { BrandMark } from "~/components/app-sidebar";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { Spinner } from "~/components/ui/spinner";

/** Crockford base32 as the server accepts it, grouped `XXXX-XXXX` as typed. */
function formatCode(raw: string): string {
  const compact = raw.toUpperCase().replace(/[^0-9A-Z]/g, "").slice(0, 8);
  return compact.length > 4 ? `${compact.slice(0, 4)}-${compact.slice(4)}` : compact;
}

export function PairScreen({ onPaired }: { onPaired: () => void }) {
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const complete = code.replace("-", "").length === 8;

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await api.pair(code);
      onPaired();
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 401
          ? "That code is wrong or expired. Run pecan pair for a new one."
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <main
      className="flex min-h-svh items-center justify-center bg-background px-5 pb-[max(env(safe-area-inset-bottom),2rem)]"
      data-testid="pair-screen"
    >
      <form
        className="w-full max-w-sm animate-[rise-in_320ms_cubic-bezier(0.2,0,0,1)] motion-reduce:animate-none"
        onSubmit={(event) => {
          event.preventDefault();
          if (complete && !busy) void submit();
        }}
      >
        <BrandMark className="size-10 rounded-[11px] text-lg" />
        <h1 className="mt-5 text-xl font-semibold tracking-tight">Pair this device</h1>
        <p className="mt-1.5 text-sm leading-6 text-muted-foreground">
          On the computer running Pecan, run{" "}
          <code className="rounded bg-muted px-1 py-0.5 font-mono text-[13px] text-foreground">pecan pair</code>{" "}
          and enter the code it prints. Codes work once and expire in 10 minutes.
        </p>
        <label className="sr-only" htmlFor="pair-code">
          Pairing code
        </label>
        <div className="mt-6">
          <Input
            aria-invalid={error !== null}
            autoCapitalize="characters"
            autoComplete="one-time-code"
            autoCorrect="off"
            autoFocus
            className="h-12 font-mono text-xl tracking-[0.25em] [&_input]:h-full [&_input]:text-center [&_input]:leading-none [&_input]:uppercase"
            enterKeyHint="go"
            id="pair-code"
            inputMode="text"
            onChange={(event) => {
              setCode(formatCode(event.target.value));
              setError(null);
            }}
            placeholder="XXXX-XXXX"
            spellCheck={false}
            value={code}
          />
        </div>
        {error ? (
          <p className="mt-2 text-sm text-destructive" role="alert">
            {error}
          </p>
        ) : null}
        <Button className="mt-4 h-11 w-full" disabled={!complete || busy} size="lg" type="submit">
          {busy ? <Spinner className="size-4" /> : null}
          Pair
        </Button>
      </form>
    </main>
  );
}
