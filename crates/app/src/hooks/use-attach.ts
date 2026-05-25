import { useCallback, useEffect } from "react";
import { toast } from "sonner";

import { events, ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";

/**
 * Attach lifecycle. The backend pushes transitions via the `attach_status`
 * event (used for progress + backend-initiated states like auto-detach on
 * process exit). But Vite HMR can orphan event listeners, so we ALSO drive
 * the store directly from the IPC return values of attach()/detach(). That
 * way the UI is correct the moment the IPC resolves, regardless of whether
 * the event made it through.
 */
export function useAttach() {
  const attachState = useAppStore((s) => s.attachState);
  const setAttachState = useAppStore((s) => s.setAttachState);
  const setFeatureResolutions = useAppStore((s) => s.setFeatureResolutions);
  const backendConnected = useAppStore((s) => s.backendConnected);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let cancelled = false;
    let unsubAttach: (() => void) | undefined;
    void (async () => {
      unsubAttach = await events.onAttachStatus((e) => {
        // TEMP: trace attach_status events to diagnose early-spinner-clear.
        // eslint-disable-next-line no-console
        console.log(
          `[attach-trace ${new Date().toISOString()}] attach_status event:`,
          e.state.kind,
          e,
        );
        if (!cancelled) setAttachState(e.state);
        if (e.reason === "process_exit") {
          toast("Game closed. Detached.");
        }
      });
    })();
    return () => {
      cancelled = true;
      if (unsubAttach) unsubAttach();
    };
  }, [setAttachState, backendConnected]);

  const attach = useCallback(
    async (gameId: string) => {
      if (!ipc.isTauri()) return;
      // Optimistic: show attaching state immediately so the UI is responsive
      // while the backend resolves AOBs.
      // eslint-disable-next-line no-console
      console.log(
        `[attach-trace ${new Date().toISOString()}] attach() calling setAttachState attaching`,
      );
      setAttachState({ kind: "attaching", gameId });
      try {
        const info = await ipc.attach(gameId);
        // eslint-disable-next-line no-console
        console.log(
          `[attach-trace ${new Date().toISOString()}] ipc.attach resolved; setting attached`,
        );
        setAttachState({
          kind: "attached",
          gameId,
          pid: info.pid,
          detectedVersion: info.detectedVersion,
        });
        setFeatureResolutions(gameId, info.perFeatureResolution);
        const failures = info.perFeatureResolution.filter((r) => !r.ok);
        if (failures.length === 0) {
          toast.success(`Attached to ${gameId}`);
        } else {
          // Don't spam one toast per failure — many of these are transient
          // (scene-dependent objects). Surface as a single soft toast that
          // points the user at the per-feature Retry button.
          const n = failures.length;
          toast(`${n} feature${n === 1 ? "" : "s"} not yet resolved`, {
            description:
              "Open the relevant in-game screen, then click Retry on the card.",
            duration: 8_000,
          });
          for (const f of failures) {
            console.warn(`[attach] ${f.featureId} unresolved:`, f.error);
          }
        }
      } catch (e) {
        setAttachState({ kind: "idle" });
        const err = e as { kind?: string; message?: string };
        toast.error(`Attach failed: ${err.message ?? "unknown"}`);
      }
    },
    [setAttachState, setFeatureResolutions],
  );

  const detach = useCallback(async () => {
    if (!ipc.isTauri()) return;
    try {
      await ipc.detach();
    } catch (e) {
      console.error(e);
    } finally {
      // Always reset to idle on detach attempt, even if the IPC errored
      // (the backend's attached state may already be cleared).
      setAttachState({ kind: "idle" });
    }
  }, [setAttachState]);

  return { attachState, attach, detach };
}
