import { AlertCircleIcon, InfoIcon, XIcon } from "lucide-react";
import { useEffect } from "react";

import { Button } from "~/components/ui/button";
import { cn } from "~/lib/utils";
import { ExtensionWidgetRenderer } from "~/components/extension-ui";
import { useApp, type ExtensionNotice, type ExtensionWidget } from "~/store";

/** Status key pi-subagents uses for its "N running" footer line. */
const SUBAGENT_STATUS = "subagents";
// Pi extensions write statuses for a terminal; drop SGR/CSI colour codes.
// oxlint-disable-next-line no-control-regex -- matching ESC is the point
const ANSI = /\u001b\[[0-9;?]*[ -/]*[@-~]/g;

function stripAnsi(text: string): string {
  return text.replace(ANSI, "");
}

const EMPTY_NOTICES: ExtensionNotice[] = [];
const EMPTY_STATUSES: Record<string, { key: string; text: string }> = {};
const EMPTY_WIDGETS: Record<string, ExtensionWidget> = {};

export function ExtensionChrome({
  sessionId,
  placement,
}: {
  sessionId: string;
  placement: "aboveEditor" | "belowEditor";
}) {
  const notices = useApp((state) => state.extensionNotices[sessionId] ?? EMPTY_NOTICES);
  const statuses = useApp((state) => state.extensionStatuses[sessionId] ?? EMPTY_STATUSES);
  const widgets = useApp((state) => state.extensionWidgets[sessionId] ?? EMPTY_WIDGETS);
  const activity = useApp((state) => state.subagentActivity[sessionId]);
  const title = useApp((state) => state.extensionTitles[sessionId]);
  const removeNotice = useApp((state) => state.removeExtensionNotice);

  useEffect(() => {
    const previous = document.title;
    document.title = title ? `${title} · pecan` : "pecan";
    return () => {
      document.title = previous;
    };
  }, [title]);

  const visibleWidgets = Object.values(widgets).filter((widget) => widget.placement === placement);
  // The agent tabs already show running children; drop the TUI's duplicate line.
  const visibleStatuses = Object.values(statuses)
    .filter((status) => !(activity && status.key === SUBAGENT_STATUS))
    .map((status) => ({ key: status.key, text: stripAnsi(status.text) }))
    .filter((status) => status.text.trim() !== "");
  const showChrome = placement === "aboveEditor" && (notices.length > 0 || visibleStatuses.length > 0);
  if (!showChrome && visibleWidgets.length === 0) return null;

  return (
    <div className="mx-auto flex w-full max-w-[46rem] flex-col gap-2 px-4 pb-1 md:px-6">
      {placement === "aboveEditor" ? (
        <>
          {visibleStatuses.length > 0 ? (
            <div
              aria-label="Extension status"
              className="flex min-w-0 items-center gap-3 px-1 text-[11px] leading-4 text-muted-foreground"
            >
              {visibleStatuses.map((status) => (
                <span className="min-w-0 truncate" key={status.key} title={status.text}>
                  {status.text}
                </span>
              ))}
            </div>
          ) : null}
          {notices.map((notice) => (
            <Notice key={notice.id} notice={notice} onDismiss={() => removeNotice(sessionId, notice.id)} />
          ))}
        </>
      ) : null}
      {visibleWidgets.map((widget) => (
        <ExtensionWidgetRenderer activity={activity} key={widget.key} widget={widget} />
      ))}
    </div>
  );
}

function Notice({ notice, onDismiss }: { notice: ExtensionNotice; onDismiss: () => void }) {
  return (
    <div
      className={cn(
        "flex items-start gap-2 rounded-lg border px-3 py-2 text-[13px]",
        notice.notifyType === "error" && "border-destructive/40 bg-destructive/10",
        notice.notifyType === "warning" && "border-warning/40 bg-warning/10",
        notice.notifyType === "info" && "border-border bg-card",
      )}
      role={notice.notifyType === "error" ? "alert" : "status"}
    >
      {notice.notifyType === "info" ? (
        <InfoIcon aria-hidden className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
      ) : (
        <AlertCircleIcon aria-hidden className="mt-0.5 size-3.5 shrink-0 text-warning" />
      )}
      <span className="min-w-0 flex-1 whitespace-pre-wrap">{notice.message}</span>
      <Button aria-label="Dismiss notification" className="-my-1 -me-1" onClick={onDismiss} size="icon-micro" variant="ghost">
        <XIcon />
      </Button>
    </div>
  );
}
