import { useEffect } from "react";

import { toast } from "sonner";

import { BootSplash } from "@/components/boot-splash";
import { Header } from "@/components/header";
import { GameSidebar } from "@/components/game-sidebar";
import { FeaturePane } from "@/components/feature-pane";
import { EmptyState } from "@/components/empty-state";
import { ErrorBoundary } from "@/components/error-boundary";
import { ThemeTransitionOverlay } from "@/components/theme-transition-overlay";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { useGames } from "@/hooks/use-games";
import { useProcessWatch } from "@/hooks/use-process-watch";
import { useActiveGame } from "@/hooks/use-active-game";
import { useSettings } from "@/hooks/use-settings";
import { useProductionHardening } from "@/hooks/use-production-hardening";
import { useAppStore } from "@/store/app-store";
import { events, ipc } from "@/lib/ipc";
import { playHotkeySound } from "@/lib/sounds";

export default function App() {
  useProductionHardening();
  useSettings();
  useGames();
  useProcessWatch();
  const { game } = useActiveGame();
  const backendConnected = useAppStore((s) => s.backendConnected);
  const setPreflight = useAppStore((s) => s.setPreflight);
  const setHeapScanProgress = useAppStore((s) => s.setHeapScanProgress);
  const setFreezeRuntime = useAppStore((s) => s.setFreezeRuntime);
  const setFreezeOn = useAppStore((s) => s.setFreezeOn);
  const setKeybindsForGame = useAppStore((s) => s.setKeybindsForGame);
  const clearGameRuntimeState = useAppStore((s) => s.clearGameRuntimeState);
  const games = useAppStore((s) => s.games);
  const attachState = useAppStore((s) => s.attachState);

  useEffect(() => {
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onPreflightChanged((e) => {
        if (!cancelled) setPreflight(e.gameId, e.report);
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, [setPreflight]);

  useEffect(() => {
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onHeapScanProgress((e) => {
        if (!cancelled) setHeapScanProgress(e);
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, [setHeapScanProgress]);

  // Freeze + read-probe runtime state subscription. Lives at App-level
  // (not inside `useFeatures`) so it's mounted from app launch — per-game
  // subscriptions would miss the first ~2s of events fired by the read
  // probes spawned during attach, since `tauri::Window::listen` is async
  // and Tauri doesn't replay missed events.
  useEffect(() => {
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onFeatureFreezeState((e) => {
        if (cancelled) return;
        setFreezeRuntime(e.gameId, e.featureId, {
          state: e.state,
          hint: e.hint,
        });
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, [setFreezeRuntime]);

  // Clear the heap-scan progress as soon as the attach flow leaves the
  // resolving phase. The backend doesn't emit a "done" event — the absence
  // of further progress events plus the attached/idle terminal state is the
  // signal.
  useEffect(() => {
    if (
      attachState.kind !== "attaching" &&
      attachState.kind !== "resolvingAobs" &&
      attachState.kind !== "finalizing"
    ) {
      setHeapScanProgress(null);
    }
  }, [attachState.kind, setHeapScanProgress]);

  // Hotkey-fired subscription: plays the activation sound + posts a
  // toast. Lives at App-level so the listener is mounted from launch
  // — keybinds work without ever opening the feature pane.
  useEffect(() => {
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onHotkeyFired((e) => {
        if (cancelled) return;
        const soundKind =
          e.action === "fire"
            ? "fire"
            : e.action === "toggle_on"
              ? "on"
              : "off";
        playHotkeySound(soundKind);
        const verb =
          e.action === "fire"
            ? "fired"
            : e.action === "toggle_on"
              ? "ON"
              : "OFF";
        toast.success(`${e.displayName} ${verb}`);
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, []);

  // Mirror the backend's freeze-on toggles into the store slice that
  // drives the SwitchControl. Hotkey toggles emit this; the click path
  // updates the slice directly.
  useEffect(() => {
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onFeatureFreezeToggled((e) => {
        if (cancelled) return;
        setFreezeOn(e.gameId, e.featureId, e.frozen);
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, [setFreezeOn]);

  // Hydrate ALL games' keybinds as soon as the games list is loaded —
  // not on attach. The user sees the trainer's feature pane (and the
  // chord badges next to each Switch) the moment they pick a game in
  // the sidebar, well before they hit Activate. Reading from the Rust
  // side is cheap (in-memory store; the JSON file was loaded once at
  // startup).
  useEffect(() => {
    if (!ipc.isTauri() || games.length === 0) return;
    let cancelled = false;
    for (const g of games) {
      void ipc
        .listKeybinds(g.id)
        .then((entries) => {
          if (!cancelled) setKeybindsForGame(g.id, entries);
        })
        .catch((e) => {
          // eslint-disable-next-line no-console
          console.error("[keybinds] hydrate failed", { game: g.id, err: e });
        });
    }
    return () => {
      cancelled = true;
    };
  }, [games, setKeybindsForGame]);

  // On attach: wipe any stale freeze-runtime state for this game so
  // we don't show "frozen=true" badges left over from the previous
  // attach session (the Rust freeze handles were dropped at detach
  // but the FE store still remembered the last reported state).
  useEffect(() => {
    if (attachState.kind !== "attached") return;
    clearGameRuntimeState(attachState.gameId);
  }, [attachState, clearGameRuntimeState]);

  return (
    <TooltipProvider>
      <div className="flex h-screen flex-col">
        <Header />
        <div className="flex min-h-0 flex-1">
          <GameSidebar />
          <div className="flex min-h-0 flex-1 flex-col">
            {!backendConnected ? (
              <div className="p-4">
                <Alert variant="destructive">
                  <AlertTitle>Backend not connected</AlertTitle>
                  <AlertDescription>
                    The Tauri backend isn't responding. If you're previewing the
                    UI without the desktop shell, this is expected.
                  </AlertDescription>
                </Alert>
              </div>
            ) : null}
            {game ? (
              <ErrorBoundary area={`Feature pane (${game.id})`}>
                <FeaturePane game={game} />
              </ErrorBoundary>
            ) : (
              <EmptyState
                title="Select a game"
                description="Pick a game from the sidebar to see its features."
              />
            )}
          </div>
        </div>
        <Toaster />
      </div>
      <ThemeTransitionOverlay />
      <BootSplash />
    </TooltipProvider>
  );
}
