import { useEffect } from "react";

import { events, ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";

/**
 * Wire the running-game set to the backend's process watcher. Two layers:
 *
 *   - Push: subscribe to the `process_state` event. Backend emits whenever
 *     the running set changes (debounced 250ms). This is the primary channel
 *     and is instant.
 *
 *   - Pull: poll `get_process_state` every 2s as a fallback. Vite HMR can
 *     orphan event listeners across reloads (Tauri 2's event bridge keeps
 *     handlers attached to a dead JS context, so new emits go nowhere). The
 *     poll self-heals — if push delivery is missing emits, the next poll
 *     overwrites local state with the truth. Also covers the case where the
 *     subscription set up before the watcher's first tick fired.
 */
export function useProcessWatch() {
  const setRunning = useAppStore((s) => s.setRunning);
  const backendConnected = useAppStore((s) => s.backendConnected);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let unsub: (() => void) | undefined;
    let cancelled = false;
    let pollTimer: ReturnType<typeof setInterval> | undefined;

    void (async () => {
      try {
        await ipc.startWatcher();
        const initial = await ipc.getProcessState();
        if (!cancelled) setRunning(initial.running);
        unsub = await events.onProcessState((e) => {
          if (!cancelled) setRunning(e.running);
        });
        pollTimer = setInterval(() => {
          if (cancelled) return;
          ipc
            .getProcessState()
            .then((s) => {
              if (!cancelled) setRunning(s.running);
            })
            .catch(() => {
              // backend reloading; ignore — next tick will retry
            });
        }, 2000);
      } catch (e) {
        console.error("watcher init failed", e);
      }
    })();

    return () => {
      cancelled = true;
      if (unsub) unsub();
      if (pollTimer) clearInterval(pollTimer);
    };
  }, [setRunning, backendConnected]);
}
