import { useEffect, useRef, useState } from "react";

import { BrandMark } from "@/components/brand-mark";
import { useAppStore } from "@/store/app-store";
import { cn } from "@/lib/utils";

// Never flash the splash shorter than this — a sub-100ms blip is worse than
// no splash at all. Anything ≥ MIN_VISIBLE_MS feels like an intentional load
// screen rather than a half-rendered page.
const MIN_VISIBLE_MS = 480;

// If the backend never reports ready (no `tauri:dev` running, or a stuck
// startup), we don't want a permanently locked screen. After this, drop the
// splash and let the underlying error Alert tell the story.
const MAX_VISIBLE_MS = 4500;

const FADE_MS = 320;

export function BootSplash() {
  const games = useAppStore((s) => s.games);
  const backendConnected = useAppStore((s) => s.backendConnected);

  // Capture the mount timestamp once — used to enforce MIN_VISIBLE_MS so the
  // splash never appears as a one-frame flicker.
  const startedAtRef = useRef(performance.now());
  const [fading, setFading] = useState(false);
  const [unmounted, setUnmounted] = useState(false);

  useEffect(() => {
    const isContentReady = backendConnected && games.length > 0;

    let dismissTimer: number | undefined;
    const failsafe = window.setTimeout(() => setFading(true), MAX_VISIBLE_MS);

    if (isContentReady) {
      const elapsed = performance.now() - startedAtRef.current;
      const wait = Math.max(0, MIN_VISIBLE_MS - elapsed);
      dismissTimer = window.setTimeout(() => setFading(true), wait);
    }

    return () => {
      window.clearTimeout(failsafe);
      if (dismissTimer !== undefined) window.clearTimeout(dismissTimer);
    };
  }, [backendConnected, games.length]);

  // Once the fade kicks in, fully unmount after the transition so the splash
  // can't ever block clicks on the live UI underneath.
  useEffect(() => {
    if (!fading) return;
    const t = window.setTimeout(() => setUnmounted(true), FADE_MS);
    return () => window.clearTimeout(t);
  }, [fading]);

  if (unmounted) return null;

  return (
    <div
      data-slot="boot-splash"
      aria-hidden={fading}
      className={cn(
        "fixed inset-0 z-[60] grid place-items-center",
        "bg-[var(--color-background)]",
        "transition-opacity ease-out",
        fading ? "pointer-events-none opacity-0" : "opacity-100",
      )}
      style={{ transitionDuration: `${FADE_MS}ms` }}
    >
      <SplashBackdrop />
      <div className="relative z-10 flex flex-col items-center gap-5">
        <div
          className={cn(
            "grid h-14 w-14 place-items-center rounded-[10px]",
            "bg-[var(--color-foreground)] text-[var(--color-background)]",
            "shadow-[0_8px_30px_-12px_color-mix(in_oklch,var(--color-foreground)_55%,transparent)]",
            "ring-1 ring-[var(--color-foreground)]/15",
            "splash-pulse",
          )}
        >
          <BrandMark className="h-7 w-7" />
        </div>

        <div className="flex flex-col items-center gap-1.5">
          <span className="text-[15px] font-semibold tracking-tight">
            OpenForge
          </span>
          <span className="text-[10.5px] uppercase tracking-[0.22em] text-[var(--color-muted-foreground)]">
            warming up the forge…
          </span>
        </div>

        <div
          aria-hidden
          className={cn(
            "relative mt-1 h-[3px] w-40 overflow-hidden rounded-full",
            "bg-[color-mix(in_oklch,var(--color-foreground)_10%,transparent)]",
          )}
        >
          <div
            className={cn(
              "absolute inset-y-0 left-0 w-1/3 rounded-full",
              "bg-[var(--color-foreground)]/70",
              "splash-sweep",
            )}
          />
        </div>
      </div>
    </div>
  );
}

function SplashBackdrop() {
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-0 overflow-hidden"
    >
      <div
        className="absolute -left-24 top-1/3 h-80 w-80 rounded-full blur-3xl"
        style={{
          background:
            "radial-gradient(closest-side, color-mix(in oklch, var(--color-foreground) 7%, transparent), transparent 70%)",
        }}
      />
      <div
        className="absolute -right-24 bottom-1/3 h-80 w-80 rounded-full blur-3xl"
        style={{
          background:
            "radial-gradient(closest-side, color-mix(in oklch, var(--color-foreground) 6%, transparent), transparent 70%)",
        }}
      />
    </div>
  );
}
