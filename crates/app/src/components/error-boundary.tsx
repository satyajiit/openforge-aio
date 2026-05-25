import { Component, type ErrorInfo, type ReactNode } from "react";

import { cn } from "@/lib/utils";

type Props = {
  children: ReactNode;
  /** Optional label shown above the error message. */
  area?: string;
};

type State = {
  error: Error | null;
  componentStack: string | null;
};

/**
 * Visible error boundary — when something downstream throws, render the
 * error and the React component stack right where the failure occurred so
 * we never silently turn the whole pane black.
 *
 * React 19's default behavior on uncaught errors is to unmount the nearest
 * boundary's subtree; without a boundary, that's the entire tree. The result
 * is a window that shows only `body`'s background colour — easy to mistake
 * for a CSS / launcher / theme bug.
 */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    // Surface in DevTools too, in case the user is debugging.
    // eslint-disable-next-line no-console
    console.error("[OpenForge] uncaught:", error, info);
    this.setState({ componentStack: info.componentStack ?? null });
  }

  private reset = (): void => {
    this.setState({ error: null, componentStack: null });
  };

  override render(): ReactNode {
    if (!this.state.error) return this.props.children;

    const { error, componentStack } = this.state;

    return (
      <div
        data-slot="error-boundary"
        className={cn(
          "flex h-full w-full items-start justify-center overflow-auto p-6",
        )}
      >
        <div
          className={cn(
            "max-w-3xl w-full rounded-[var(--radius)] border border-[var(--color-destructive)]/70",
            "bg-[var(--color-card)] p-4 shadow-sm",
          )}
        >
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <h2 className="text-sm font-semibold tracking-tightest">
              {this.props.area ?? "Render error"}
            </h2>
            <button
              type="button"
              onClick={this.reset}
              className="text-[11px] underline-offset-2 hover:underline"
            >
              try again
            </button>
          </div>
          <p className="mt-2 text-xs text-[var(--color-foreground)]">
            {error.name}: {error.message}
          </p>
          {componentStack ? (
            <pre
              className={cn(
                "mt-3 max-h-[60vh] overflow-auto rounded-[var(--radius)] border border-[var(--color-border)]/60",
                "bg-[var(--color-background)] p-3 text-[10px] text-[var(--color-muted-foreground)]",
              )}
            >
              {componentStack.trim()}
            </pre>
          ) : null}
          {error.stack ? (
            <pre
              className={cn(
                "mt-2 max-h-[40vh] overflow-auto rounded-[var(--radius)] border border-[var(--color-border)]/60",
                "bg-[var(--color-background)] p-3 text-[10px] text-[var(--color-muted-foreground)]",
              )}
            >
              {error.stack.split("\n").slice(0, 30).join("\n")}
            </pre>
          ) : null}
        </div>
      </div>
    );
  }
}
