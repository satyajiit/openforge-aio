import { useEffect } from "react";

import { ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";

export function useGames() {
  const games = useAppStore((s) => s.games);
  const setGames = useAppStore((s) => s.setGames);
  const backendConnected = useAppStore((s) => s.backendConnected);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let cancelled = false;
    void (async () => {
      try {
        const g = await ipc.listGames();
        if (!cancelled) setGames(g);
      } catch (e) {
        console.error("listGames failed", e);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [setGames, backendConnected]);

  return games;
}
