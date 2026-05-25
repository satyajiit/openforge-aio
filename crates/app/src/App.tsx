import { useEffect } from "react";

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
import { useAppStore } from "@/store/app-store";
import { events } from "@/lib/ipc";

export default function App() {
  useSettings();
  useGames();
  useProcessWatch();
  const { game } = useActiveGame();
  const backendConnected = useAppStore((s) => s.backendConnected);
  const setPreflight = useAppStore((s) => s.setPreflight);
  const setHeapScanProgress = useAppStore((s) => s.setHeapScanProgress);
  const setFreezeRuntime = useAppStore((s) => s.setFreezeRuntime);
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
    if (attachState.kind !== "attaching" && attachState.kind !== "resolvingAobs") {
      setHeapScanProgress(null);
    }
  }, [attachState.kind, setHeapScanProgress]);

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
    </TooltipProvider>
  );
}
