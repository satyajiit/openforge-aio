import { useEffect } from "react";

import { ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";

export function useSettings() {
  const settings = useAppStore((s) => s.settings);
  const setSettings = useAppStore((s) => s.setSettings);
  const setBackendConnected = useAppStore((s) => s.setBackendConnected);
  const setActiveGame = useAppStore((s) => s.setActiveGame);

  useEffect(() => {
    if (!ipc.isTauri()) {
      setBackendConnected(false);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const s = await ipc.getSettings();
        if (cancelled) return;
        setSettings(s);
        setBackendConnected(true);
        if (s.lastActiveGame) setActiveGame(s.lastActiveGame);
      } catch (e) {
        console.error("getSettings failed", e);
        setBackendConnected(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [setSettings, setBackendConnected, setActiveGame]);

  return settings;
}
