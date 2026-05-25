import { useEffect, useState } from "react";

import { cn } from "@/lib/utils";
import type { GameMeta } from "@/types";

type Size = "sm" | "md";

const SIZE_CLASSES: Record<Size, string> = {
  sm: "h-9 w-9",
  md: "h-12 w-12",
};

/**
 * Async-loading square icon for a game.
 *
 * - Decodes the image off the main thread (`decoding="async"`) so the UI never
 *   janks while a multi-MB data URL paints.
 * - Shows a shimmer placeholder until the image is fully decoded; falls back
 *   to the first letter of the display name when the game has no icon or the
 *   image fails to decode.
 * - Caches the "decoded once" decision per data URL via a module-scoped Set so
 *   re-mounts (sidebar scroll, tab switches) don't reflash the loader.
 */
export function GameIcon({
  game,
  isActive,
  size = "sm",
}: {
  game: GameMeta;
  isActive?: boolean;
  size?: Size;
}) {
  const hasIcon = Boolean(game.iconDataUrl);
  const [loaded, setLoaded] = useState(() =>
    hasIcon ? DECODED_CACHE.has(game.iconDataUrl) : false,
  );
  const [failed, setFailed] = useState(false);

  // Reset state if the underlying icon source changes.
  useEffect(() => {
    if (!hasIcon) return;
    if (DECODED_CACHE.has(game.iconDataUrl)) {
      setLoaded(true);
      setFailed(false);
      return;
    }
    setLoaded(false);
    setFailed(false);
    let cancelled = false;
    const img = new Image();
    img.decoding = "async";
    img.src = game.iconDataUrl;
    void img
      .decode()
      .then(() => {
        if (cancelled) return;
        DECODED_CACHE.add(game.iconDataUrl);
        setLoaded(true);
      })
      .catch(() => {
        if (cancelled) return;
        setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [game.iconDataUrl, hasIcon]);

  const sizeClass = SIZE_CLASSES[size];

  if (!hasIcon || failed) {
    return (
      <Fallback
        letter={game.displayName.charAt(0).toUpperCase()}
        sizeClass={sizeClass}
        isActive={isActive ?? false}
      />
    );
  }

  return (
    <div
      data-slot="game-icon"
      className={cn(
        "relative shrink-0 overflow-hidden rounded-[calc(var(--radius)+1px)]",
        "border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-card)_55%,transparent)]",
        sizeClass,
      )}
    >
      {!loaded ? (
        <span
          aria-hidden
          data-slot="game-icon-skeleton"
          className={cn(
            "absolute inset-0",
            "bg-[linear-gradient(90deg,transparent,color-mix(in_oklch,var(--color-foreground)_12%,transparent),transparent)]",
            "bg-[length:200%_100%] animate-[shimmer_1.4s_ease-in-out_infinite]",
          )}
        />
      ) : null}
      <img
        src={game.iconDataUrl}
        alt=""
        loading="lazy"
        decoding="async"
        draggable={false}
        className={cn(
          "h-full w-full object-cover transition-opacity duration-300",
          loaded ? "opacity-100" : "opacity-0",
        )}
        onLoad={() => {
          DECODED_CACHE.add(game.iconDataUrl);
          setLoaded(true);
        }}
        onError={() => setFailed(true)}
      />
    </div>
  );
}

function Fallback({
  letter,
  sizeClass,
  isActive,
}: {
  letter: string;
  sizeClass: string;
  isActive: boolean;
}) {
  return (
    <div
      aria-hidden
      data-slot="game-icon-fallback"
      className={cn(
        "grid shrink-0 place-items-center rounded-[calc(var(--radius)+1px)] border text-sm font-semibold",
        "transition-colors",
        sizeClass,
        isActive
          ? "border-[var(--color-foreground)]/30 bg-[var(--color-card)]/60 text-[var(--color-foreground)]"
          : "border-[var(--color-border)] bg-[color-mix(in_oklch,var(--color-card)_70%,transparent)] text-[var(--color-muted-foreground)]",
      )}
    >
      {letter}
    </div>
  );
}

// Module-scoped cache of data URLs we have already decoded once. Prevents the
// shimmer from re-flashing on re-mount (sidebar rerenders, route changes).
const DECODED_CACHE: Set<string> = new Set();
