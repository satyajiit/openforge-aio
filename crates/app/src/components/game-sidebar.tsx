import { useMemo } from "react";
import { Plus, SearchX } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { GameIcon } from "@/components/game-icon";
import { useAppStore } from "@/store/app-store";
import { cn } from "@/lib/utils";
import type { GameMeta } from "@/types";

import { AddGameDialog } from "@/components/add-game-dialog";

export function GameSidebar() {
  const games = useAppStore((s) => s.games);
  const running = useAppStore((s) => s.runningGameIds);
  const activeGameId = useAppStore((s) => s.activeGameId);
  const setActiveGame = useAppStore((s) => s.setActiveGame);
  const query = useAppStore((s) => s.gameSearchQuery);
  const setQuery = useAppStore((s) => s.setGameSearchQuery);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return games;
    return games.filter((g) =>
      [g.id, g.displayName, g.tagline, ...g.processNames]
        .join(" ")
        .toLowerCase()
        .includes(q),
    );
  }, [games, query]);

  return (
    <aside
      data-slot="game-sidebar"
      className={cn(
        "glass-sidebar flex h-full w-72 shrink-0 flex-col",
        "border-r border-[var(--color-sidebar-border)]",
      )}
    >
      <SidebarHeader
        gamesCount={games.length}
        runningCount={running.size}
        filteredCount={filtered.length}
        hasQuery={query.length > 0}
      />

      <ScrollArea className="flex-1">
        <ul data-slot="game-list" className="flex flex-col gap-0.5 px-2 py-2">
          {filtered.map((g) => (
            <GameRow
              key={g.id}
              game={g}
              isRunning={running.has(g.id)}
              isActive={activeGameId === g.id}
              onClick={() => setActiveGame(g.id)}
            />
          ))}
          {filtered.length === 0 ? (
            query.length > 0 ? (
              <NoMatchMessage query={query} onClear={() => setQuery("")} />
            ) : (
              <EmptySidebarMessage />
            )
          ) : null}
        </ul>
      </ScrollArea>

      <SidebarFooter />
    </aside>
  );
}

function SidebarHeader({
  gamesCount,
  runningCount,
  filteredCount,
  hasQuery,
}: {
  gamesCount: number;
  runningCount: number;
  filteredCount: number;
  hasQuery: boolean;
}) {
  return (
    <div
      data-slot="sidebar-header"
      className={cn(
        "flex items-end justify-between gap-2 px-4 pb-3 pt-4",
        "border-b border-[var(--color-sidebar-border)]",
      )}
    >
      <div className="flex flex-col gap-0.5">
        <span className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--color-muted-foreground)]">
          Library
        </span>
        <span className="tabular text-xs text-[var(--color-foreground)]">
          {hasQuery ? (
            <>
              <span className="font-medium">{filteredCount}</span>
              <span className="text-[var(--color-muted-foreground)]">
                {" / "}
                {gamesCount} match
              </span>
            </>
          ) : (
            <>
              <span className="font-medium">{gamesCount}</span>{" "}
              {gamesCount === 1 ? "title" : "titles"}
              <span className="text-[var(--color-muted-foreground)]">
                {" · "}
                {runningCount} running
              </span>
            </>
          )}
        </span>
      </div>
      <span className="tabular text-[10px] text-[var(--color-muted-foreground)]">
        v0.1.0
      </span>
    </div>
  );
}

function SidebarFooter() {
  return (
    <div className="border-t border-[var(--color-sidebar-border)] p-2">
      <AddGameDialog
        trigger={
          <Button
            variant="ghost"
            size="sm"
            className="w-full justify-start gap-2 text-xs"
          >
            <Plus className="h-3.5 w-3.5" strokeWidth={2.25} />
            Add a game
          </Button>
        }
      />
    </div>
  );
}

function EmptySidebarMessage() {
  return (
    <li
      data-slot="sidebar-empty"
      className="mx-1 rounded-[var(--radius)] border border-dashed border-[var(--color-sidebar-border)]/70 px-3 py-6 text-center text-xs leading-relaxed text-[var(--color-muted-foreground)]"
    >
      No games registered yet.
      <br />
      Use the scaffolder to add one.
    </li>
  );
}

function NoMatchMessage({
  query,
  onClear,
}: {
  query: string;
  onClear: () => void;
}) {
  return (
    <li
      data-slot="sidebar-no-match"
      className="mx-1 flex flex-col items-center gap-2 rounded-[var(--radius)] border border-dashed border-[var(--color-sidebar-border)]/70 px-3 py-6 text-center text-xs leading-relaxed text-[var(--color-muted-foreground)]"
    >
      <SearchX className="h-4 w-4" strokeWidth={2} />
      <span>
        No games match
        <span className="ml-1 font-medium text-[var(--color-foreground)]">
          "{query}"
        </span>
      </span>
      <button
        type="button"
        onClick={onClear}
        className="text-[11px] underline-offset-2 hover:underline"
      >
        Clear search
      </button>
    </li>
  );
}

function GameRow({
  game,
  isRunning,
  isActive,
  onClick,
}: {
  game: GameMeta;
  isRunning: boolean;
  isActive: boolean;
  onClick: () => void;
}) {
  const version = game.supportedVersions[0];

  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        title={game.displayName}
        data-slot="game-row"
        data-active={isActive}
        data-running={isRunning}
        className={cn(
          "group relative flex w-full items-center gap-3 rounded-[var(--radius)] px-2.5 py-2.5 text-left transition-colors",
          "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--color-sidebar-ring)]",
          isActive
            ? "bg-[var(--color-sidebar-accent)] text-[var(--color-sidebar-accent-foreground)]"
            : "hover:bg-[var(--color-sidebar-accent)]/55",
        )}
      >
        {isActive ? (
          <span
            aria-hidden
            className="absolute inset-y-2 left-0 w-[2px] rounded-full bg-[var(--color-foreground)]"
          />
        ) : null}

        <GameIcon game={game} isActive={isActive} size="sm" />

        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="line-clamp-2 text-[13px] font-medium leading-snug text-pretty">
            {game.displayName}
          </span>
          <div className="flex items-center gap-1.5 text-[10px] text-[var(--color-muted-foreground)]">
            <span
              className={cn(
                "inline-flex h-1.5 w-1.5 shrink-0 rounded-full transition-colors",
                isRunning
                  ? "bg-[var(--color-foreground)] shadow-[0_0_0_2px_color-mix(in_oklch,var(--color-foreground)_15%,transparent)]"
                  : "ring-1 ring-[var(--color-muted-foreground)]/60",
              )}
              aria-hidden
            />
            <span className="uppercase tracking-[0.12em]">
              {isRunning ? "running" : "offline"}
            </span>
            {version ? (
              <>
                <span aria-hidden className="text-[var(--color-border)]">
                  ·
                </span>
                <span className="tabular">{version}</span>
              </>
            ) : null}
          </div>
        </div>
      </button>
    </li>
  );
}

