import { useEffect, useState } from "react";
import { Loader2, Power, PowerOff, ShieldAlert } from "lucide-react";

import { Button } from "@/components/ui/button";
import { useAttach } from "@/hooks/use-attach";
import { ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";
import { cn } from "@/lib/utils";
import type { GameMeta } from "@/types";

type CtaState = "needs_game" | "ready" | "attaching" | "active" | "needs_admin";

/**
 * The activate bar — the single most important affordance in the app.
 * Previous design used 100+px of vertical real estate; this one
 * compresses to one tight row (~48px) with the orbital live indicator
 * collapsed to a 28px dot. The visual hierarchy is now:
 *   status dot · headline + subtitle · CTA
 * all on one horizontal axis.
 */
export function ActivateBar({ game }: { game: GameMeta }) {
  const running = useAppStore((s) => s.runningGameIds.has(game.id));
  const attachState = useAppStore((s) => s.attachState);
  const { attach, detach } = useAttach();
  const [elevated, setElevated] = useState<boolean | null>(null);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    void ipc
      .isElevated()
      .then(setElevated)
      .catch(() => setElevated(null));
  }, []);

  const attached =
    attachState.kind === "attached" && attachState.gameId === game.id;
  const attaching =
    (attachState.kind === "attaching" || attachState.kind === "resolvingAobs") &&
    (attachState.kind === "attaching" || attachState.gameId === game.id);

  const needsAdmin = game.requiresAdmin && elevated === false;

  const state: CtaState = attached
    ? "active"
    : attaching
      ? "attaching"
      : needsAdmin
        ? "needs_admin"
        : running
          ? "ready"
          : "needs_game";

  const onClick = async () => {
    if (state === "needs_admin") {
      await ipc.relaunchAsAdmin().catch(() => {});
      return;
    }
    if (state === "active") {
      await detach();
      return;
    }
    if (state === "ready") {
      await attach(game.id);
    }
  };

  const detectedVersion =
    attachState.kind === "attached" && attachState.gameId === game.id
      ? attachState.detectedVersion
      : null;
  const progressLabel =
    attachState.kind === "resolvingAobs" && attachState.gameId === game.id
      ? `${attachState.resolved}/${attachState.total}`
      : null;
  const heapScan = useAppStore((s) => s.heapScanProgress);
  const heapScanLabel =
    heapScan && heapScan.gameId === game.id
      ? formatHeapScanLabel(heapScan.currentBytes, heapScan.totalBytes)
      : null;

  return (
    <section
      data-slot="activate-bar"
      data-state={state}
      className={cn(
        "group relative overflow-hidden rounded-[10px]",
        "border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-card)_70%,transparent)] backdrop-blur-md",
        "transition-colors duration-300",
        state === "active" && "border-[var(--color-foreground)]/35",
      )}
    >
      <AmbientGlow active={state === "active"} attaching={state === "attaching"} />

      <div className="relative flex items-center gap-3 px-3.5 py-2.5">
        <StatusDot state={state} />

        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <h2 className="truncate text-[13px] font-semibold leading-tight tracking-tight">
              <StateTitle state={state} />
            </h2>
            <span className="truncate text-[11.5px] leading-tight text-[var(--color-muted-foreground)]">
              <StateSubtitle
                state={state}
                gameName={game.displayName}
                version={detectedVersion}
                progress={progressLabel}
                heapScanLabel={heapScanLabel}
              />
            </span>
          </div>
          {heapScan && heapScan.gameId === game.id ? (
            <HeapScanBar current={heapScan.currentBytes} total={heapScan.totalBytes} />
          ) : null}
        </div>

        <CtaButton state={state} onClick={() => void onClick()} />
      </div>
    </section>
  );
}

function HeapScanBar({ current, total }: { current: number; total: number }) {
  const pct = total > 0 ? Math.min(100, (current / total) * 100) : 0;
  return (
    <div
      aria-hidden
      className="mt-1.5 h-[2px] w-full overflow-hidden rounded-full bg-[color-mix(in_oklch,var(--color-foreground)_8%,transparent)]"
    >
      <div
        className="h-full rounded-full bg-[var(--color-foreground)] transition-[width] duration-150 ease-out"
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

function formatHeapScanLabel(current: number, total: number): string {
  const fmt = (n: number) => {
    if (n >= 1_073_741_824) return `${(n / 1_073_741_824).toFixed(2)} GB`;
    if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(0)} MB`;
    if (n >= 1_024) return `${(n / 1_024).toFixed(0)} KB`;
    return `${n} B`;
  };
  return `${fmt(current)} / ${fmt(total)}`;
}

function StateTitle({ state }: { state: CtaState }) {
  switch (state) {
    case "active":
      return <>Trainer active</>;
    case "attaching":
      return <>Hooking in…</>;
    case "ready":
      return <>Ready to forge</>;
    case "needs_admin":
      return <>Admin rights required</>;
    case "needs_game":
      return <>Game not running</>;
  }
}

function StateSubtitle({
  state,
  gameName,
  version,
  progress,
  heapScanLabel,
}: {
  state: CtaState;
  gameName: string;
  version: string | null;
  progress: string | null;
  heapScanLabel: string | null;
}) {
  switch (state) {
    case "active":
      return (
        <>Cheats are live{version ? ` · ${version}` : ""}</>
      );
    case "attaching":
      if (heapScanLabel) return <>Scanning memory · {heapScanLabel}</>;
      return progress ? (
        <>Resolving · {progress}</>
      ) : (
        <>Opening a handle…</>
      );
    case "ready":
      return <>{gameName.split(":")[0]} is running — click to activate</>;
    case "needs_admin":
      return <>Restart OpenForge as admin to continue</>;
    case "needs_game":
      return <>Launch {gameName.split(":")[0]} and we'll wake up automatically</>;
  }
}

function CtaButton({
  state,
  onClick,
}: {
  state: CtaState;
  onClick: () => void;
}) {
  const disabled = state === "needs_game" || state === "attaching";

  const Icon =
    state === "active"
      ? PowerOff
      : state === "attaching"
        ? Loader2
        : state === "needs_admin"
          ? ShieldAlert
          : Power;

  const label =
    state === "active"
      ? "Deactivate"
      : state === "attaching"
        ? "Attaching"
        : state === "needs_admin"
          ? "Restart as admin"
          : state === "ready"
            ? "Activate"
            : "Awaiting game";

  return (
    <Button
      onClick={onClick}
      disabled={disabled}
      size="sm"
      variant={state === "active" ? "outline" : "default"}
      className={cn(
        "relative h-7 min-w-[110px] gap-1.5 px-3 text-[11.5px] font-semibold tracking-tight",
        "transition-all duration-300 ease-out",
        state === "ready" && "shadow-md shadow-[var(--color-foreground)]/15",
        state === "active" &&
          "border-[var(--color-foreground)]/40 hover:border-[var(--color-foreground)]/70",
      )}
    >
      <Icon
        className={cn(
          "h-3.5 w-3.5 transition-transform duration-300",
          state === "attaching" && "animate-spin",
          state === "ready" && "group-hover:translate-x-0.5",
        )}
        strokeWidth={2.25}
      />
      {label}
    </Button>
  );
}

/**
 * Compact state indicator. 28×28 instead of the previous orbital 56×56.
 * Renders a single dot inside a thin ring; the ring spins while attaching
 * and pulses while active. All the cues, none of the real estate.
 */
function StatusDot({ state }: { state: CtaState }) {
  const active = state === "active";
  const attaching = state === "attaching";
  return (
    <div
      aria-hidden
      data-state={state}
      className="relative grid h-6 w-6 shrink-0 place-items-center"
    >
      <span
        className={cn(
          "absolute inset-0 rounded-full border transition-all duration-500",
          active
            ? "border-[var(--color-foreground)]/40"
            : "border-[var(--color-border)]",
          attaching && "animate-spin border-dashed border-[var(--color-foreground)]/40",
        )}
        style={attaching ? { animationDuration: "1.5s" } : undefined}
      />
      <span
        className={cn(
          "relative h-2 w-2 rounded-full transition-all duration-500",
          active
            ? "bg-[var(--color-foreground)] shadow-[0_0_12px_2px_color-mix(in_oklch,var(--color-foreground)_28%,transparent)] animate-pulse"
            : attaching
              ? "bg-[var(--color-foreground)]/70"
              : state === "ready"
                ? "bg-[var(--color-foreground)]/85"
                : "bg-transparent ring-1 ring-[var(--color-muted-foreground)]/60",
        )}
      />
    </div>
  );
}

function AmbientGlow({
  active,
  attaching,
}: {
  active: boolean;
  attaching: boolean;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "pointer-events-none absolute -left-8 top-1/2 h-24 w-24 -translate-y-1/2 rounded-full blur-2xl transition-opacity duration-700",
        active
          ? "opacity-25 bg-[color-mix(in_oklch,var(--color-foreground)_18%,transparent)]"
          : attaching
            ? "opacity-15 bg-[color-mix(in_oklch,var(--color-foreground)_10%,transparent)]"
            : "opacity-0",
      )}
    />
  );
}
