import * as React from "react";
import * as SwitchPrimitives from "@radix-ui/react-switch";

import { cn } from "@/lib/utils";

const Switch = React.forwardRef<
  React.ElementRef<typeof SwitchPrimitives.Root>,
  React.ComponentPropsWithoutRef<typeof SwitchPrimitives.Root>
>(({ className, ...props }, ref) => (
  <SwitchPrimitives.Root
    ref={ref}
    data-slot="switch"
    className={cn(
      "peer relative inline-flex h-[22px] w-[40px] shrink-0 cursor-pointer items-center rounded-full",
      "border transition-colors duration-150",
      "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-2",
      "focus-visible:ring-[var(--color-ring)] focus-visible:ring-offset-[var(--color-background)]",
      "disabled:cursor-not-allowed disabled:opacity-50",
      // Off — clearly visible track in both themes
      "data-[state=unchecked]:border-[color-mix(in_oklch,var(--color-foreground)_18%,transparent)]",
      "data-[state=unchecked]:bg-[color-mix(in_oklch,var(--color-foreground)_12%,transparent)]",
      // On — solid foreground fill
      "data-[state=checked]:border-[var(--color-foreground)]",
      "data-[state=checked]:bg-[var(--color-foreground)]",
      "data-[state=checked]:shadow-[0_0_0_2px_color-mix(in_oklch,var(--color-foreground)_18%,transparent)]",
      className,
    )}
    {...props}
  >
    <SwitchPrimitives.Thumb
      data-slot="switch-thumb"
      className={cn(
        "pointer-events-none block h-[16px] w-[16px] rounded-full shadow-md",
        "transition-transform duration-200 ease-out will-change-transform",
        // Off — high-contrast thumb on top of the muted track
        "data-[state=unchecked]:bg-[var(--color-foreground)]",
        "data-[state=unchecked]:translate-x-[3px]",
        // On — light thumb on dark fill
        "data-[state=checked]:bg-[var(--color-background)]",
        "data-[state=checked]:translate-x-[20px]",
      )}
    />
  </SwitchPrimitives.Root>
));
Switch.displayName = SwitchPrimitives.Root.displayName;

export { Switch };
