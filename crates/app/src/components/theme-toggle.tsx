import { Moon, Sun } from "lucide-react";

import { useTheme } from "@/hooks/use-theme";
import { cn } from "@/lib/utils";

export function ThemeToggle() {
  const { themeMode, toggle } = useTheme();
  const isDark = themeMode === "dark";

  return (
    <button
      type="button"
      role="switch"
      aria-checked={isDark}
      aria-label={`Switch to ${isDark ? "light" : "dark"} mode`}
      data-slot="theme-toggle"
      data-mode={themeMode}
      onClick={() => void toggle()}
      className={cn(
        "group relative inline-flex h-7 w-[52px] shrink-0 cursor-pointer items-center",
        "rounded-full border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-card)_72%,transparent)] backdrop-blur-md",
        "shadow-[inset_0_1px_0_0_color-mix(in_oklch,var(--color-foreground)_4%,transparent)]",
        "transition-colors duration-300",
        "hover:border-[var(--color-foreground)]/30",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-ring)]/40 focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--color-background)]",
      )}
    >
      <Sun
        aria-hidden
        strokeWidth={2}
        className={cn(
          "pointer-events-none absolute left-[7px] top-1/2 h-3 w-3 -translate-y-1/2",
          "transition-opacity duration-300",
          isDark
            ? "text-[var(--color-muted-foreground)]/45"
            : "text-[var(--color-foreground)]/0",
        )}
      />
      <Moon
        aria-hidden
        strokeWidth={2}
        className={cn(
          "pointer-events-none absolute right-[7px] top-1/2 h-3 w-3 -translate-y-1/2",
          "transition-opacity duration-300",
          isDark
            ? "text-[var(--color-foreground)]/0"
            : "text-[var(--color-muted-foreground)]/45",
        )}
      />

      <span
        aria-hidden
        className={cn(
          "pointer-events-none absolute left-[2px] inset-y-0 my-auto grid h-6 w-6 place-items-center rounded-full",
          "bg-[var(--color-foreground)] text-[var(--color-background)]",
          "shadow-[0_1px_2px_0_color-mix(in_oklch,var(--color-foreground)_25%,transparent),0_0_0_1px_color-mix(in_oklch,var(--color-foreground)_8%,transparent)_inset]",
          "transition-transform duration-[420ms]",
          "[transition-timing-function:cubic-bezier(0.65,0,0.35,1)]",
          "will-change-transform",
        )}
        style={{
          transform: `translateX(${isDark ? "24px" : "0px"})`,
        }}
      >
        <Sun
          strokeWidth={2.25}
          className={cn(
            "h-[13px] w-[13px] [grid-area:1/1]",
            "transition-[opacity,transform] duration-[260ms] ease-out",
            isDark
              ? "scale-50 opacity-0"
              : "scale-100 opacity-100 delay-[120ms]",
          )}
        />
        <Moon
          strokeWidth={2.25}
          className={cn(
            "h-[13px] w-[13px] [grid-area:1/1]",
            "transition-[opacity,transform] duration-[260ms] ease-out",
            isDark
              ? "scale-100 opacity-100 delay-[120ms]"
              : "scale-50 opacity-0",
          )}
        />
      </span>
    </button>
  );
}
