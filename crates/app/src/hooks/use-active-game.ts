import { useEffect } from "react";

import { ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";

export function useActiveGame() {
  const activeGameId = useAppStore((s) => s.activeGameId);
  const setActiveGame = useAppStore((s) => s.setActiveGame);
  const games = useAppStore((s) => s.games);
  const settings = useAppStore((s) => s.settings);
  const setFeatures = useAppStore((s) => s.setFeatures);
  const setPreflight = useAppStore((s) => s.setPreflight);
  const featuresByGame = useAppStore((s) => s.featuresByGame);

  useEffect(() => {
    if (!activeGameId || !ipc.isTauri()) return;
    let cancelled = false;
    void (async () => {
      if (!featuresByGame[activeGameId]) {
        try {
          const f = await ipc.listFeatures(activeGameId);
          if (!cancelled) setFeatures(activeGameId, f);
        } catch (e) {
          console.error("listFeatures failed", e);
        }
      }
      try {
        const r = await ipc.preflight(activeGameId);
        if (!cancelled) setPreflight(activeGameId, r);
      } catch (e) {
        console.error("preflight failed", e);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeGameId, setFeatures, setPreflight, featuresByGame, settings]);

  const game = games.find((g) => g.id === activeGameId) ?? null;
  return { activeGameId, game, setActiveGame };
}
