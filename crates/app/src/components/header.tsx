import { Github, Search, X, Youtube } from "lucide-react";
import { type ChangeEvent, useEffect, useRef } from "react";

import { Button } from "@/components/ui/button";
import { BrandMark } from "@/components/brand-mark";
import { ThemeToggle } from "@/components/theme-toggle";
import { TrafficLights } from "@/components/window-chrome";
import { useAppStore } from "@/store/app-store";
import { cn } from "@/lib/utils";

/**
 * Unified titlebar — replaces the OS title bar. Windows convention
 * places window controls on the right edge, so the layout is:
 *
 *   ┌──────────────────────────────────────────────────────────────────┐
 *   │ OpenForge mark │   search   │ status │ YT │ ⌥ │ ☼ │ ─ ▢ ×        │
 *   └──────────────────────────────────────────────────────────────────┘
 *
 * The entire strip is a drag region except for interactive controls.
 */
export function Header() {
  const backendConnected = useAppStore((s) => s.backendConnected);

  return (
    <header
      data-tauri-drag-region
      data-slot="app-header"
      className={cn(
        "titlebar-surface relative z-30 flex shrink-0 items-center gap-4 px-4",
        "select-none",
      )}
      style={{ height: "var(--titlebar-height)" }}
    >
      <Brand />

      <div data-tauri-drag-region className="flex flex-1 justify-center">
        <GameSearch />
      </div>

      <div className="flex items-center gap-2">
        <BackendIndicator connected={backendConnected} />
        <div className="flex items-center gap-0.5">
          <Button
            variant="ghost"
            size="icon"
            aria-label="Subscribe on YouTube"
            title="Subscribe to @GamesPatch on YouTube"
            className="group/yt relative h-7 w-7"
            onClick={() => {
              window.open(
                "https://www.youtube.com/@GamesPatch?sub_confirmation=1",
                "_blank",
                "noopener,noreferrer",
              );
            }}
          >
            <Youtube
              className="h-3.5 w-3.5 transition-colors group-hover/yt:text-[var(--tl-close)]"
              strokeWidth={2}
            />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            aria-label="GitHub"
            title="GitHub"
            className="h-7 w-7"
            onClick={() => {
              window.open(
                "https://github.com/GamesPatch/openforge",
                "_blank",
                "noopener,noreferrer",
              );
            }}
          >
            <Github className="h-3.5 w-3.5" strokeWidth={2} />
          </Button>
          <ThemeToggle />
        </div>

        <span
          aria-hidden
          className="mx-1 h-4 w-px shrink-0 bg-[color-mix(in_oklch,var(--color-foreground)_12%,transparent)]"
        />

        <TrafficLights />
      </div>
    </header>
  );
}

function Brand() {
  return (
    <div
      data-tauri-drag-region
      className="flex items-center gap-2.5 pointer-events-none"
    >
      <div className="grid h-6 w-6 place-items-center rounded-[5px] bg-[var(--color-foreground)] text-[var(--color-background)] shadow-sm ring-1 ring-[var(--color-foreground)]/10">
        <BrandMark className="h-[14px] w-[14px]" />
      </div>
      <span className="tracking-tightest text-[13px] font-semibold leading-none">
        OpenForge
      </span>
    </div>
  );
}

function GameSearch() {
  const query = useAppStore((s) => s.gameSearchQuery);
  const setQuery = useAppStore((s) => s.setGameSearchQuery);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        inputRef.current?.focus();
      }
      if (e.key === "Escape" && document.activeElement === inputRef.current) {
        setQuery("");
        inputRef.current?.blur();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setQuery]);

  return (
    <div
      data-slot="game-search"
      className={cn(
        "relative hidden h-7 w-80 items-center md:flex",
        "rounded-md border border-[var(--color-border)]",
        "bg-[color-mix(in_oklch,var(--color-card)_60%,transparent)] backdrop-blur-md",
        "transition-colors focus-within:border-[var(--color-foreground)]/40",
      )}
    >
      <Search
        className="pointer-events-none absolute left-2.5 h-3 w-3 text-[var(--color-muted-foreground)]"
        strokeWidth={2.25}
      />
      <input
        ref={inputRef}
        type="text"
        placeholder="Search games"
        value={query}
        onChange={(e: ChangeEvent<HTMLInputElement>) => setQuery(e.target.value)}
        className={cn(
          "h-full w-full bg-transparent pl-7 pr-14 text-[12px] outline-none",
          "placeholder:text-[var(--color-muted-foreground)]",
        )}
      />
      {query ? (
        <button
          type="button"
          onClick={() => setQuery("")}
          aria-label="Clear search"
          className="absolute right-1.5 grid h-4 w-4 place-items-center rounded-full text-[var(--color-muted-foreground)] transition-colors hover:bg-[var(--color-accent)] hover:text-[var(--color-foreground)]"
        >
          <X className="h-2.5 w-2.5" strokeWidth={2.5} />
        </button>
      ) : (
        <span
          aria-hidden
          className={cn(
            "absolute right-2.5 inline-flex items-center gap-0.5 text-[9px]",
            "text-[var(--color-muted-foreground)]",
          )}
        >
          <kbd className="rounded-sm border border-[var(--color-border)] px-1 py-px">
            Ctrl
          </kbd>
          <kbd className="rounded-sm border border-[var(--color-border)] px-1 py-px">
            K
          </kbd>
        </span>
      )}
    </div>
  );
}

function BackendIndicator({ connected }: { connected: boolean }) {
  return (
    <span
      data-slot="backend-indicator"
      data-connected={connected}
      title={connected ? "Backend connected" : "Backend offline"}
      className={cn(
        "hidden h-6 items-center gap-1.5 rounded-full border border-[var(--color-border)] px-2 text-[9.5px] uppercase tracking-[0.18em] md:inline-flex",
        "bg-[color-mix(in_oklch,var(--color-card)_60%,transparent)]",
        connected
          ? "text-[var(--color-foreground)]"
          : "text-[var(--color-muted-foreground)]",
      )}
    >
      <span
        aria-hidden
        className={cn(
          "inline-flex h-1.5 w-1.5 rounded-full",
          connected
            ? "bg-[var(--color-foreground)] shadow-[0_0_0_2px_color-mix(in_oklch,var(--color-foreground)_15%,transparent)]"
            : "ring-1 ring-[var(--color-muted-foreground)]/70",
        )}
      />
      {connected ? "ready" : "offline"}
    </span>
  );
}
