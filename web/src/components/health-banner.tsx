/** Up-front "Pi cannot run" notice, instead of discovering it through a
 * session that never starts. Rechecks whenever live updates reconnect. */
import { TriangleAlertIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { api } from "~/api/client";
import type { Health } from "~/api/types";
import { Button } from "~/components/ui/button";
import { useApp } from "~/store";

export function HealthBanner() {
  const connected = useApp((state) => state.connected);
  const [health, setHealth] = useState<Health | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    if (!connected) return;
    let live = true;
    api
      .health()
      .then((next) => live && setHealth(next))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [connected]);

  if (!health || health.status === "ok") return null;
  const headline = health.status === "missing" ? "Pi is not installed on this machine" : "Pi is not working";
  return (
    <div
      className="flex items-start gap-2.5 border-b border-warning/40 bg-warning/10 px-3 py-2.5 text-sm"
      data-testid="health-banner"
      role="alert"
    >
      <TriangleAlertIcon aria-hidden className="mt-0.5 size-4 shrink-0 text-warning" />
      <div className="min-w-0 flex-1">
        <p className="font-medium">{headline}</p>
        {health.detail ? (
          <p className="mt-0.5 break-words text-xs text-muted-foreground">{health.detail}</p>
        ) : null}
      </div>
      <Button
        className="min-h-9 shrink-0"
        disabled={checking}
        onClick={() => {
          setChecking(true);
          api
            .health(true)
            .then(setHealth)
            .catch(() => undefined)
            .finally(() => setChecking(false));
        }}
        size="sm"
        variant="outline"
      >
        {checking ? "Checking…" : "Recheck"}
      </Button>
    </div>
  );
}
