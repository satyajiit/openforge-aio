import { Toaster as SonnerToaster } from "sonner";

export function Toaster() {
  return (
    <SonnerToaster
      position="bottom-right"
      toastOptions={{
        classNames: {
          toast:
            "bg-[var(--color-card)] text-[var(--color-card-foreground)] border border-[var(--color-border)] rounded-[var(--radius)] shadow-sm",
          title: "text-sm font-medium",
          description: "text-xs text-[var(--color-muted-foreground)]",
        },
      }}
      data-slot="sonner-toaster"
    />
  );
}

export { toast } from "sonner";
