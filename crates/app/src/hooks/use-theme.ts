import { useEffect } from "react";

import { ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";

const LEAVE_MS = 360;
const ENTER_MS = 520;

export function useTheme() {
  const settings = useAppStore((s) => s.settings);
  const setSettings = useAppStore((s) => s.setSettings);
  const transition = useAppStore((s) => s.themeTransition);
  const setTransition = useAppStore((s) => s.setThemeTransition);

  const themeMode = settings?.themeMode ?? "dark";

  useEffect(() => {
    const html = document.documentElement;
    if (themeMode === "dark") html.classList.add("dark");
    else html.classList.remove("dark");
  }, [themeMode]);

  const toggle = async () => {
    if (!settings || transition) return;
    const nextMode = themeMode === "dark" ? "light" : "dark";
    setTransition({ direction: nextMode === "dark" ? "to-dark" : "to-light" });

    await new Promise((r) => setTimeout(r, LEAVE_MS));

    const nextSettings = { ...settings, themeMode: nextMode as "light" | "dark" };
    setSettings(nextSettings);
    if (ipc.isTauri()) {
      try {
        await ipc.setSettings(nextSettings);
      } catch (_) {
        // ignore — local state still updated
      }
    }

    await new Promise((r) => setTimeout(r, ENTER_MS));
    setTransition(null);
  };

  return { themeMode, toggle, transitioning: transition !== null };
}
