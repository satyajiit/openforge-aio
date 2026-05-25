import { useCallback, useEffect } from "react";
import { toast } from "sonner";

import { events, ipc } from "@/lib/ipc";
import { useAppStore } from "@/store/app-store";
import type { FeatureMeta, FeatureResolution, Value } from "@/types";
import type { FreezeRuntime } from "@/store/app-store";

// Module-scoped stable references so zustand selectors never return a fresh
// empty array / record on each render. Returning a new `[]` from a selector
// is equivalent to "the value changed" under Object.is comparison and would
// trigger an infinite re-render loop.
const EMPTY_FEATURES: readonly FeatureMeta[] = Object.freeze([]);
const EMPTY_VALUES: Readonly<Record<string, Value>> = Object.freeze({});
const EMPTY_RESOLUTIONS: Readonly<Record<string, FeatureResolution>> =
  Object.freeze({});
const EMPTY_STATUS_TEXTS: Readonly<Record<string, string | null>> =
  Object.freeze({});
const EMPTY_FREEZE_RUNTIMES: Readonly<Record<string, FreezeRuntime>> =
  Object.freeze({});

export function useFeatures(gameId: string | null) {
  const features = useAppStore(
    (s) =>
      (gameId ? s.featuresByGame[gameId] : undefined) ??
      (EMPTY_FEATURES as FeatureMeta[]),
  );
  const values = useAppStore(
    (s) =>
      (gameId ? s.featureValues[gameId] : undefined) ??
      (EMPTY_VALUES as Record<string, Value>),
  );
  const resolutions = useAppStore(
    (s) =>
      (gameId ? s.featureResolutions[gameId] : undefined) ??
      (EMPTY_RESOLUTIONS as Record<string, FeatureResolution>),
  );
  const statusTexts = useAppStore(
    (s) =>
      (gameId ? s.featureStatusTexts[gameId] : undefined) ??
      (EMPTY_STATUS_TEXTS as Record<string, string | null>),
  );
  const freezeRuntimes = useAppStore(
    (s) =>
      (gameId ? s.freezeRuntimeByFeature[gameId] : undefined) ??
      (EMPTY_FREEZE_RUNTIMES as Record<string, FreezeRuntime>),
  );
  const setFeatureValue = useAppStore((s) => s.setFeatureValue);
  const setFeatureResolution = useAppStore((s) => s.setFeatureResolution);
  const setFeatureStatusText = useAppStore((s) => s.setFeatureStatusText);
  const setFreezeRuntime = useAppStore((s) => s.setFreezeRuntime);
  const attachState = useAppStore((s) => s.attachState);

  const attached =
    attachState.kind === "attached" && attachState.gameId === gameId;

  useEffect(() => {
    if (!gameId || !attached || !ipc.isTauri()) return;
    let cancelled = false;
    void (async () => {
      const ids = features.map((f) => f.id);
      if (ids.length === 0) return;
      try {
        const pairs = await ipc.readFeatures(gameId, ids);
        if (cancelled) return;
        for (const [id, v] of pairs) setFeatureValue(gameId, id, v);
      } catch (e) {
        console.error("readFeatures failed", e);
      }
      // Fetch the optional rich status string for each feature in
      // parallel. Backend returns null for features that don't override
      // `Feature::status_text` (the common case); we still call them all
      // since the per-feature cost when it's a no-op is negligible
      // (~1ms IPC round-trip with no DLL-side work).
      await Promise.allSettled(
        ids.map(async (id) => {
          try {
            const text = await ipc.featureStatusText(gameId, id);
            if (!cancelled) setFeatureStatusText(gameId, id, text);
          } catch {
            // Feature not resolved yet (pending) → status_text call fails;
            // just skip silently. Will retry after the next write.
          }
        }),
      );
    })();
    return () => {
      cancelled = true;
    };
  }, [gameId, attached, features, setFeatureValue, setFeatureStatusText]);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onFeatureChanged((e) => {
        if (!cancelled) setFeatureValue(e.gameId, e.featureId, e.value);
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, [setFeatureValue]);

  // Note: feature_freeze_state subscription moved to App.tsx — it must
  // be mounted from app launch to avoid losing the first ~2s of events
  // emitted by the read-probe loops spawned during attach.

  // Live updates to the per-feature resolution state — fires for both the
  // initial attach-time resolves and on-demand `retry_resolve` calls.
  useEffect(() => {
    if (!ipc.isTauri()) return;
    let unsub: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unsub = await events.onFeatureResolved((e) => {
        if (cancelled) return;
        setFeatureResolution(e.gameId, {
          featureId: e.featureId,
          ok: e.ok,
          error: e.error ?? null,
          // status is optional in the wire type — backend always sends it on
          // current code paths; fall back to the boolean for older events.
          status: e.status ?? (e.ok ? "resolved" : "failed"),
        });
      });
    })();
    return () => {
      cancelled = true;
      if (unsub) unsub();
    };
  }, [setFeatureResolution]);

  const write = useCallback(
    async (featureId: string, value: Value) => {
      if (!gameId || !ipc.isTauri()) return;
      try {
        const v = await ipc.writeFeature(gameId, featureId, value);
        setFeatureValue(gameId, featureId, v);
        // Refresh rich status (e.g. "X / 9 fast-travel terminals unlocked")
        // after a successful write. Fire-and-forget so the toast appears
        // immediately. We retry once after a short delay because bulk writes
        // (SetProgressTags fires N CallUFunction roundtrips) can saturate the
        // pipe — the first immediate Get may race a still-in-flight Set on
        // the DLL side and either error or read pre-grant state.
        const refreshStatus = (attempt: number) => {
          void ipc
            .featureStatusText(gameId, featureId)
            .then((text) => setFeatureStatusText(gameId, featureId, text))
            .catch((err) => {
              if (attempt === 0) {
                setTimeout(() => refreshStatus(1), 400);
              } else {
                // eslint-disable-next-line no-console
                console.error("[status refresh] failed after retry", {
                  featureId,
                  err,
                });
              }
            });
        };
        refreshStatus(0);
        toast.success("applied");
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error("[write] failure", { featureId, value, err: e });
        const err = e as { kind?: string; message?: string };
        toast.error(`write failed: ${err.message ?? JSON.stringify(e)}`);
      }
    },
    [gameId, setFeatureValue, setFeatureStatusText],
  );

  const setFreeze = useCallback(
    async (featureId: string, frozen: boolean) => {
      if (!gameId || !ipc.isTauri()) return;
      try {
        await ipc.setFreeze(gameId, featureId, frozen);
        // When the user toggles freeze OFF, the backend aborts the loop —
        // no terminal event is emitted, so clear our local runtime state
        // here. Toggle-ON state will be populated by the first
        // feature_freeze_state event from the new loop's first tick.
        if (!frozen) {
          setFreezeRuntime(gameId, featureId, null);
        }
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error("[setFreeze] failure", { featureId, frozen, err: e });
        const err = e as { kind?: string; message?: string };
        toast.error(`freeze toggle failed: ${err.message ?? JSON.stringify(e)}`);
      }
    },
    [gameId, setFreezeRuntime],
  );

  const setCodePatch = useCallback(
    async (featureId: string, applied: boolean) => {
      if (!gameId || !ipc.isTauri()) return;
      try {
        await ipc.setCodePatch(gameId, featureId, applied);
        // Mirror the backend's new state into the store so the Switch's
        // `checked` prop (read via isApplied(current)) reflects reality.
        // Without this, the Switch stays OFF even after a successful patch.
        setFeatureValue(gameId, featureId, { kind: "bool", v: applied });
        toast.success(applied ? "enabled" : "disabled");
      } catch (e) {
        const err = e as { kind?: string; message?: string };
        toast.error(`toggle failed: ${err.message ?? "unknown"}`);
      }
    },
    [gameId, setFeatureValue],
  );

  const retryResolve = useCallback(
    async (featureId: string) => {
      if (!gameId || !ipc.isTauri()) return;
      try {
        const res = await ipc.retryResolve(gameId, featureId);
        // Backend also emits feature_resolved; mirror immediately so the UI
        // updates without waiting for the event round-trip.
        setFeatureResolution(gameId, res);
        if (res.ok) {
          toast.success(`${featureId} resolved`);
        } else {
          toast.error(`${featureId}: ${res.error ?? "still not resolved"}`);
        }
      } catch (e) {
        const err = e as { kind?: string; message?: string };
        toast.error(`retry failed: ${err.message ?? "unknown"}`);
      }
    },
    [gameId, setFeatureResolution],
  );

  return {
    features,
    values,
    resolutions,
    statusTexts,
    freezeRuntimes,
    attached,
    write,
    setFreeze,
    setCodePatch,
    retryResolve,
  };
}
