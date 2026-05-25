// Lua Scripts tab — placeholder. The Lua runtime isn't wired yet; this
// surfaces the UI slot today and will become the Monaco editor + run
// controls once the runtime lands.

import { Code2 } from "lucide-react";

import { EmptyState } from "@/components/empty-state";

export function LuaScriptsTab({ gameId: _gameId }: { gameId: string }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-6">
      <div className="max-w-md text-center">
        <div
          aria-hidden
          className="mb-4 inline-flex h-10 w-10 items-center justify-center rounded-lg bg-[var(--color-muted)] text-[var(--color-muted-foreground)]"
        >
          <Code2 className="h-5 w-5" />
        </div>
        <EmptyState
          title="Coming soon"
          description="Run custom Lua scripts inside the trainer for one-off experiments — read or write any address, call any UFunction, query reflection state. Lands after the flagship feature pack ships."
        />
      </div>
    </div>
  );
}
