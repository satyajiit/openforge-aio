import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { cn } from "@/lib/utils";

/**
 * Window controls — three colored dots in macOS' visual language, but
 * laid out in Windows order (minimize · maximize · close, with close
 * at the rightmost edge) since the rest of the OS convention sits there.
 * The dots reveal − ▢ × glyphs on hover. Backed by Tauri's window
 * plugin (minimize / toggleMaximize / close). When the WebView isn't
 * inside a Tauri shell (Vite dev preview), the buttons render but
 * no-op rather than throw.
 *
 * The buttons themselves do NOT carry data-tauri-drag-region; the
 * containing header element does, and clicks on the buttons take
 * precedence over the drag handler because they're not the drag-region
 * element themselves.
 */
export function TrafficLights() {
  const [hovering, setHovering] = useState(false);

  const close = () => void getCurrentWindow().close().catch(() => {});
  const minimize = () => void getCurrentWindow().minimize().catch(() => {});
  const toggleMaximize = () =>
    void getCurrentWindow().toggleMaximize().catch(() => {});

  return (
    <div
      data-slot="window-controls"
      onMouseEnter={() => setHovering(true)}
      onMouseLeave={() => setHovering(false)}
      className="group/tl flex items-center gap-2"
      aria-label="Window controls"
    >
      <Light
        color="var(--tl-min)"
        glyph={MinGlyph}
        revealed={hovering}
        ariaLabel="Minimize"
        onClick={minimize}
      />
      <Light
        color="var(--tl-max)"
        glyph={MaxGlyph}
        revealed={hovering}
        ariaLabel="Maximize"
        onClick={toggleMaximize}
      />
      <Light
        color="var(--tl-close)"
        glyph={CloseGlyph}
        revealed={hovering}
        ariaLabel="Close"
        onClick={close}
      />
    </div>
  );
}

function Light({
  color,
  glyph: Glyph,
  revealed,
  ariaLabel,
  onClick,
}: {
  color: string;
  glyph: React.FC;
  revealed: boolean;
  ariaLabel: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={ariaLabel}
      title={ariaLabel}
      className={cn(
        "relative grid h-3 w-3 place-items-center rounded-full",
        "transition-[transform,box-shadow] duration-150 ease-out",
        "active:scale-90",
        "focus-visible:outline-none focus-visible:ring-2",
      )}
      style={{
        backgroundColor: color,
        boxShadow: `inset 0 0 0 0.5px color-mix(in oklch, black 25%, transparent),
                    0 0 0 0.5px color-mix(in oklch, black 8%, transparent)`,
      }}
    >
      <span
        aria-hidden
        className={cn(
          "pointer-events-none grid place-items-center transition-opacity duration-100",
          revealed ? "opacity-90" : "opacity-0",
        )}
        style={{ color: "color-mix(in oklch, black 70%, transparent)" }}
      >
        <Glyph />
      </span>
    </button>
  );
}

function CloseGlyph() {
  return (
    <svg width="6" height="6" viewBox="0 0 6 6" fill="none">
      <path
        d="M1 1 L5 5 M5 1 L1 5"
        stroke="currentColor"
        strokeWidth="1.25"
        strokeLinecap="round"
      />
    </svg>
  );
}

function MinGlyph() {
  return (
    <svg width="6" height="6" viewBox="0 0 6 6" fill="none">
      <path
        d="M1 3 H5"
        stroke="currentColor"
        strokeWidth="1.25"
        strokeLinecap="round"
      />
    </svg>
  );
}

function MaxGlyph() {
  // Hollow square — Windows' canonical "maximize" affordance. Stroke
  // weight matches the close × and minimize − so the three glyphs sit
  // visually balanced when the row is hovered.
  return (
    <svg width="7" height="7" viewBox="0 0 7 7" fill="none">
      <rect
        x="1.25"
        y="1.25"
        width="4.5"
        height="4.5"
        rx="0.5"
        stroke="currentColor"
        strokeWidth="1"
        fill="none"
      />
    </svg>
  );
}

/** Tracks whether the window is currently maximized so callers can swap
 *  layout tweaks (e.g. soften the outer shadow when borderless full-screen). */
export function useIsMaximized() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const w = getCurrentWindow();
    void w
      .isMaximized()
      .then((m) => {
        if (!cancelled) setMaximized(m);
      })
      .catch(() => {});
    const unlistenPromise = w.onResized(async () => {
      const m = await w.isMaximized().catch(() => false);
      if (!cancelled) setMaximized(m);
    });
    return () => {
      cancelled = true;
      void unlistenPromise.then((u) => u()).catch(() => {});
    };
  }, []);

  return maximized;
}
