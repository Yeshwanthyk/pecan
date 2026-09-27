/** App-wide toasts: errors that would otherwise vanish, announced politely. */
import { CircleAlertIcon, InfoIcon, XIcon } from "lucide-react";
import { useEffect } from "react";

import { cn } from "~/lib/utils";
import { useApp, type Toast } from "~/store";

const TOAST_MS = 6000;

export function Toaster() {
  const toasts = useApp((state) => state.toasts);
  return (
    <div
      aria-live="polite"
      className="pointer-events-none fixed inset-x-0 top-2 z-50 flex flex-col items-center gap-2 px-3 sm:top-auto sm:bottom-4 sm:items-end sm:px-4"
      data-testid="toaster"
    >
      {toasts.map((toast) => (
        <ToastCard key={toast.id} toast={toast} />
      ))}
    </div>
  );
}

function ToastCard({ toast }: { toast: Toast }) {
  const dismiss = useApp((state) => state.dismissToast);
  useEffect(() => {
    const timer = setTimeout(() => dismiss(toast.id), TOAST_MS);
    return () => clearTimeout(timer);
  }, [dismiss, toast.id]);
  const Icon = toast.tone === "error" ? CircleAlertIcon : InfoIcon;
  return (
    <div
      className={cn(
        "pointer-events-auto flex w-full max-w-sm animate-toast-in items-start gap-2.5 rounded-lg border bg-popover py-2.5 pr-2 pl-3 text-sm text-popover-foreground shadow-lg",
        toast.tone === "error" && "border-destructive/40",
      )}
      data-testid="toast"
      role={toast.tone === "error" ? "alert" : "status"}
    >
      <Icon
        aria-hidden
        className={cn(
          "mt-0.5 size-4 shrink-0",
          toast.tone === "error" ? "text-destructive" : "text-muted-foreground",
        )}
      />
      <p className="min-w-0 flex-1 break-words leading-5">{toast.message}</p>
      <button
        aria-label="Dismiss"
        className="-my-1 flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
        onClick={() => dismiss(toast.id)}
        type="button"
      >
        <XIcon className="size-3.5" />
      </button>
    </div>
  );
}
