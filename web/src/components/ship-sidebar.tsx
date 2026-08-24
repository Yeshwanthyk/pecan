/** Review and execute the checkout-wide commit, push, and pull-request lifecycle. */
import {
  CheckIcon,
  CircleAlertIcon,
  ExternalLinkIcon,
  FileDiffIcon,
  GitBranchIcon,
  GitCommitIcon,
  LoaderCircleIcon,
  PackageCheckIcon,
  UploadCloudIcon,
} from "lucide-react";
import { useEffect, useState } from "react";

import { ApiError, api } from "~/api/client";
import type { ShipPlan, ShipResult } from "~/api/types";
import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import {
  Sheet,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetPanel,
  SheetPopup,
  SheetTitle,
} from "~/components/ui/sheet";
import { Textarea } from "~/components/ui/textarea";
import { useApp } from "~/store";

export default function ShipSidebar({
  sessionId,
  open,
  onOpenChange,
  onReviewDiff,
}: {
  sessionId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onReviewDiff: () => void;
}) {
  const [plan, setPlan] = useState<ShipPlan | null>(null);
  const [branch, setBranch] = useState("");
  const [commitMessage, setCommitMessage] = useState("");
  const [pullRequestTitle, setPullRequestTitle] = useState("");
  const [pullRequestBody, setPullRequestBody] = useState("");
  const [loading, setLoading] = useState(false);
  const [shipping, setShipping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ShipResult | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setResult(null);
    void api
      .shipPlan(sessionId)
      .then((next) => {
        if (cancelled) return;
        setPlan(next);
        setBranch(next.branch === next.baseBranch ? next.suggestedBranch : (next.branch ?? ""));
        setCommitMessage(next.commitMessage);
        setPullRequestTitle(next.pullRequestTitle);
        setPullRequestBody(next.pullRequestBody);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, sessionId]);

  async function ship() {
    if (!plan || shipping || plan.blockers.length > 0) return;
    setShipping(true);
    setError(null);
    try {
      const shipped = await api.ship(sessionId, {
        branch,
        baseBranch: plan.baseBranch,
        commitMessage,
        pullRequestTitle,
        pullRequestBody,
        reviewToken: plan.reviewToken,
      });
      setResult(shipped);
      window.dispatchEvent(new CustomEvent("pecan:refresh"));
      const updated = await api.thread(sessionId);
      if (useApp.getState().thread?.summary.id === sessionId) {
        useApp.getState().setThread(updated);
      }
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 401
          ? "Ship access expired when Pecan restarted. Reopen the Ship-enabled URL printed by pecan serve."
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
      void api.shipPlan(sessionId).then(setPlan).catch(() => undefined);
    } finally {
      setShipping(false);
    }
  }

  const existingPullRequest = result?.pullRequest ?? plan?.pullRequest ?? null;
  const needsBranch = plan?.branch === plan?.baseBranch;
  const buttonLabel = existingPullRequest
    ? plan?.hasChanges || (plan?.unpushedCount ?? 0) > 0
      ? "Commit & push"
      : "Settle & view PR"
    : "Commit all, push & create PR";

  return (
    <Sheet onOpenChange={onOpenChange} open={open}>
      <SheetPopup className="w-full max-w-none sm:w-[30rem] sm:max-w-none" side="right">
        <SheetHeader className="shrink-0 gap-1 border-b px-4 py-4 pe-12">
          <div className="flex items-center gap-2">
            <PackageCheckIcon className="size-4 text-muted-foreground" />
            <SheetTitle className="text-base">Ship this work</SheetTitle>
          </div>
          <SheetDescription className="text-xs leading-5">
            Review the checkout, commit every local change, push the branch, and ensure a GitHub
            pull request exists. The thread settles only after every step succeeds.
          </SheetDescription>
        </SheetHeader>

        <SheetPanel className="space-y-5 px-4 py-4">
          {loading ? <ShipLoading /> : null}
          {plan && !loading ? (
            <>
              <div className="grid grid-cols-3 gap-px overflow-hidden rounded-lg border bg-border text-center">
                <Metric label="Changes" value={String(plan.changedFiles)} />
                <Metric label="Ahead" value={String(plan.aheadCount)} />
                <Metric label="Base" value={plan.baseBranch} mono />
              </div>
              {plan.changedFiles > 0 ? (
                <Button
                  className="w-full justify-between px-3"
                  onClick={onReviewDiff}
                  size="sm"
                  variant="outline"
                >
                  <span className="flex items-center gap-2">
                    <FileDiffIcon /> Review all {plan.changedFiles} changes
                  </span>
                  <span className="text-[11px] font-normal text-muted-foreground">Workspace diff</span>
                </Button>
              ) : null}

              {plan.blockers.length > 0 ? (
                <div className="rounded-lg border border-warning/30 bg-warning/8 p-3">
                  <div className="flex items-start gap-2 text-sm font-medium">
                    <CircleAlertIcon className="mt-0.5 size-4 shrink-0 text-warning" />
                    <span>Ship needs attention</span>
                  </div>
                  <ul className="mt-2 space-y-1 pl-6 text-xs leading-5 text-muted-foreground">
                    {plan.blockers.map((blocker) => (
                      <li key={blocker}>{blocker}</li>
                    ))}
                  </ul>
                </div>
              ) : null}

              <section>
                <p className="mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">
                  Ship path
                </p>
                <div className="rounded-lg border px-3">
                  {plan.steps.map((step) => (
                    <div className="flex min-h-10 items-center gap-2 border-b last:border-b-0" key={step.id}>
                      <StepIcon id={step.id} />
                      <span className="text-sm">{step.label}</span>
                      <span className="ms-auto text-[11px] text-muted-foreground">
                        {step.required ? "Required" : "Already done"}
                      </span>
                    </div>
                  ))}
                </div>
              </section>

              {existingPullRequest && !plan.hasChanges && plan.unpushedCount === 0 ? (
                <PullRequestCard pullRequest={existingPullRequest} />
              ) : (
                <div className="space-y-3">
                  <Field label={needsBranch ? "New feature branch" : "Branch"}>
                    <Input
                      disabled={!needsBranch}
                      nativeInput
                      onChange={(event) => setBranch(event.currentTarget.value)}
                      value={branch}
                    />
                  </Field>
                  {plan.hasChanges ? (
                    <Field label="Commit message">
                      <Input
                        nativeInput
                        onChange={(event) => setCommitMessage(event.currentTarget.value)}
                        value={commitMessage}
                      />
                    </Field>
                  ) : null}
                  {!existingPullRequest ? (
                    <>
                      <Field label="Pull request title">
                        <Input
                          nativeInput
                          onChange={(event) => setPullRequestTitle(event.currentTarget.value)}
                          value={pullRequestTitle}
                        />
                      </Field>
                      <Field label="Pull request body">
                        <Textarea
                          onChange={(event) => setPullRequestBody(event.currentTarget.value)}
                          value={pullRequestBody}
                        />
                      </Field>
                    </>
                  ) : null}
                </div>
              )}

              <p className="rounded-lg bg-muted/60 px-3 py-2.5 text-xs leading-5 text-muted-foreground">
                This includes <strong className="font-medium text-foreground">all changes</strong> in
                the workspace, including files not created by this thread. No model is used to write
                these defaults.
              </p>
            </>
          ) : null}
          {error ? (
            <div className="rounded-lg border border-destructive/25 bg-destructive/8 p-3 text-xs leading-5 text-destructive">
              {error}
            </div>
          ) : null}
        </SheetPanel>

        <SheetFooter className="shrink-0 px-4 py-3">
          {result ? (
            <Button
              onClick={() => window.open(result.pullRequest.url, "_blank", "noopener,noreferrer")}
              size="default"
            >
              <ExternalLinkIcon /> View PR #{result.pullRequest.number}
            </Button>
          ) : (
            <Button
              disabled={
                loading ||
                shipping ||
                !plan ||
                plan.blockers.length > 0 ||
                !branch.trim() ||
                (plan.hasChanges && !commitMessage.trim()) ||
                (!existingPullRequest && !pullRequestTitle.trim())
              }
              onClick={() => void ship()}
              size="default"
            >
              {shipping ? <LoaderCircleIcon className="animate-spin" /> : <PackageCheckIcon />}
              {shipping ? "Shipping…" : buttonLabel}
            </Button>
          )}
        </SheetFooter>
      </SheetPopup>
    </Sheet>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1.5">
      <span className="text-xs font-medium">{label}</span>
      {children}
    </label>
  );
}

function Metric({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="min-w-0 bg-background px-2 py-2.5">
      <p className="text-[10px] uppercase tracking-[0.1em] text-muted-foreground">{label}</p>
      <p className={`mt-0.5 truncate text-sm font-medium ${mono ? "font-mono" : "tabular-nums"}`}>
        {value}
      </p>
    </div>
  );
}

function StepIcon({ id }: { id: string }) {
  if (id === "branch") return <GitBranchIcon className="size-4 text-muted-foreground" />;
  if (id === "commit") return <GitCommitIcon className="size-4 text-muted-foreground" />;
  if (id === "push") return <UploadCloudIcon className="size-4 text-muted-foreground" />;
  return <PackageCheckIcon className="size-4 text-muted-foreground" />;
}

function PullRequestCard({ pullRequest }: { pullRequest: ShipResult["pullRequest"] }) {
  return (
    <div className="rounded-lg border bg-muted/30 p-3">
      <div className="flex items-start gap-2">
        <CheckIcon className="mt-0.5 size-4 shrink-0 text-success" />
        <div className="min-w-0">
          <p className="text-xs font-medium">Pull request #{pullRequest.number}</p>
          <p className="mt-0.5 truncate text-sm">{pullRequest.title}</p>
        </div>
      </div>
    </div>
  );
}

function ShipLoading() {
  return (
    <div aria-label="Loading Ship plan" className="space-y-3">
      <div className="h-16 animate-pulse rounded-lg bg-muted" />
      <div className="h-40 animate-pulse rounded-lg bg-muted/70" />
      <div className="h-24 animate-pulse rounded-lg bg-muted/50" />
    </div>
  );
}
