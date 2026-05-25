import { Badge } from "@/components/ui/badge";
import { tierColor } from "@/lib/tier-colors";
import { cn } from "@/lib/utils";

export function TierTag({
  tier,
  size = "md",
  className,
}: {
  tier: string;
  size?: "sm" | "md";
  className?: string;
}) {
  const color = tierColor(tier);
  return (
    <Badge
      data-slot="tier-tag"
      data-tier={tier}
      variant="tier"
      shape="pill"
      className={cn(
        "gap-1.5",
        size === "sm" ? "px-2 py-0.5 text-[10px]" : "px-2.5 py-0.5 text-[11px]",
        className,
      )}
      style={{ backgroundColor: color.bg }}
    >
      <span
        aria-hidden
        className="inline-flex h-1.5 w-1.5 rounded-full"
        style={{ backgroundColor: color.fg }}
      />
      <span className="text-[var(--color-foreground)]">{tier}</span>
    </Badge>
  );
}
