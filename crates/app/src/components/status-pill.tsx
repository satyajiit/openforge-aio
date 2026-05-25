import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

type Props = {
  running: boolean;
  version?: string | null;
};

export function StatusPill({ running, version }: Props) {
  return (
    <Badge
      data-slot="status-pill"
      data-running={running}
      variant={running ? "status-running" : "status-offline"}
      shape="pill"
      className="gap-2 px-3 py-1 text-[11px]"
    >
      <span
        aria-hidden
        className={cn(
          "inline-flex h-1.5 w-1.5 rounded-full",
          running
            ? "bg-[var(--color-foreground)] shadow-[0_0_0_2px_color-mix(in_oklch,var(--color-foreground)_15%,transparent)]"
            : "ring-1 ring-[var(--color-muted-foreground)]/60",
        )}
      />
      <span className="uppercase tracking-[0.18em]">
        {running ? "running" : "offline"}
      </span>
      {running && version ? (
        <>
          <span aria-hidden className="text-[var(--color-border)]">·</span>
          <span className="tabular text-[var(--color-muted-foreground)]">
            {version}
          </span>
        </>
      ) : null}
    </Badge>
  );
}
