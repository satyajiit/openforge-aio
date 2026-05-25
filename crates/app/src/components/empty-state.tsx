import * as React from "react";

import { cn } from "@/lib/utils";

export function EmptyState({
  className,
  title,
  description,
  children,
}: {
  className?: string;
  title: string;
  description?: string;
  children?: React.ReactNode;
}) {
  return (
    <div
      data-slot="empty-state"
      className={cn(
        "flex h-full w-full flex-col items-center justify-center gap-3 p-10 text-center",
        className,
      )}
    >
      <div
        aria-hidden
        className="h-px w-12 bg-[var(--color-foreground)]/25"
      />
      <p className="text-sm font-medium tracking-tightest">{title}</p>
      {description ? (
        <p className="max-w-sm text-xs leading-relaxed text-[var(--color-muted-foreground)] text-pretty">
          {description}
        </p>
      ) : null}
      {children}
    </div>
  );
}
