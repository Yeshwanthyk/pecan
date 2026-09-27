/**
 * Calm crash fallback for a subtree that threw during render. React error
 * boundaries must be class components (no hook equivalent exists yet).
 */
import { Component, type ReactNode } from "react";

import { Button } from "~/components/ui/button";
import { reportError } from "~/lib/errors";

type Props = {
  children: ReactNode;
  /** Change this to reset the boundary when the crashed subtree is remounted anyway (e.g. a new thread id). */
  resetKey?: string;
  /** Short label for what crashed, shown in the fallback ("This thread", "Pecan"). */
  label?: string;
};

type State = {
  error: Error | null;
};

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  override componentDidCatch(error: Error) {
    reportError(`${this.props.label ?? "Pecan"} crashed: ${error.message}`);
  }

  override componentDidUpdate(prevProps: Props) {
    if (this.state.error && prevProps.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  override render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div
        className="flex min-h-40 flex-1 items-center justify-center px-6 py-10 pb-safe"
        data-testid="error-boundary"
      >
        <div className="w-full max-w-sm rounded-xl border bg-card p-4 text-center shadow-xs">
          <p className="text-sm font-semibold tracking-tight">
            {this.props.label ?? "Something"} hit a snag
          </p>
          <p className="mt-1 text-xs text-muted-foreground">
            The page ran into an error while rendering. Trying again usually clears it.
          </p>
          <pre className="mt-3 max-h-24 overflow-auto rounded-md bg-muted px-2.5 py-2 text-left font-mono text-[11px] leading-4 break-words whitespace-pre-wrap text-muted-foreground">
            {error.message}
          </pre>
          <div className="mt-4 flex justify-center gap-2">
            <Button
              className="min-h-11 flex-1"
              data-testid="error-boundary-retry"
              onClick={() => this.setState({ error: null })}
              size="sm"
              variant="outline"
            >
              Try again
            </Button>
            <Button
              className="min-h-11 flex-1"
              data-testid="error-boundary-reload"
              onClick={() => location.reload()}
              size="sm"
            >
              Reload
            </Button>
          </div>
        </div>
      </div>
    );
  }
}
