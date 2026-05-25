import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center gap-1 border px-2 py-0.5 text-xs font-medium",
  {
    variants: {
      variant: {
        default:
          "border-transparent bg-[var(--color-primary)] text-[var(--color-primary-foreground)]",
        outline:
          "border-[var(--color-border)] text-[var(--color-foreground)]",
        muted:
          "border-transparent bg-[var(--color-muted)] text-[var(--color-muted-foreground)]",
        success:
          "border-[var(--color-border)] text-[var(--color-foreground)]",
        destructive:
          "border-[var(--color-destructive)] text-[var(--color-foreground)]",
        "status-running":
          "border-[var(--color-foreground)]/35 bg-[var(--color-card)]/55 backdrop-blur-md text-[var(--color-foreground)]",
        "status-offline":
          "border-[var(--color-border)] bg-[var(--color-card)]/55 backdrop-blur-md text-[var(--color-muted-foreground)]",
        tier: "border-[var(--color-border)] text-[var(--color-foreground)]",
      },
      shape: {
        default: "rounded-[var(--radius)]",
        pill: "rounded-full",
      },
    },
    defaultVariants: { variant: "default", shape: "default" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, shape, ...props }: BadgeProps) {
  return (
    <span
      data-slot="badge"
      className={cn(badgeVariants({ variant, shape }), className)}
      {...props}
    />
  );
}
