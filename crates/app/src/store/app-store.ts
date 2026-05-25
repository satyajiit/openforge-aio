import { create } from "zustand";

import type {
  AttachState,
  FeatureFreezeState,
  FeatureMeta,
  FeatureResolution,
  GameMeta,
  GameProfile,
  PreflightReport,
  Settings,
  Value,
} from "@/types";

export type FreezeRuntime = {
  state: FeatureFreezeState;
  hint: string | null;
};

export type HeapScanProgressState = {
  gameId: string;
  featureId: string;
  currentBytes: number;
  totalBytes: number;
};

export type ThemeTransition = {
  /** Direction the user is heading; drives which icon leaves and which arrives. */
  direction: "to-dark" | "to-light";
};

export type AppStore = {
  games: GameMeta[];
  runningGameIds: Set<string>;
  activeGameId: string | null;
  attachState: AttachState;
  preflightByGame: Record<string, PreflightReport>;
  featuresByGame: Record<string, FeatureMeta[]>;
  featureValues: Record<string, Record<string, Value>>;
  // Per-feature resolution state. Populated from the attach payload and
  // updated by `feature_resolved` events (initial attach + on-demand Retry).
  featureResolutions: Record<string, Record<string, FeatureResolution>>;
  /** Per-feature rich status string (e.g. "20 / 30 unlocked"). Populated
   * on-demand by `useFeatures` after attach + after writes. Only features
   * that override `Feature::status_text` produce an entry; the rest stay
   * absent so the UI hides the row. */
  featureStatusTexts: Record<string, Record<string, string | null>>;
  /** Per-feature freeze-loop health (active vs waiting). Only populated for
   * features whose freeze loop is currently running; cleared when the user
   * toggles freeze OFF. UI uses this to render a "retrying…" badge on
   * reflection-backed freezes during world transitions. */
  freezeRuntimeByFeature: Record<string, Record<string, FreezeRuntime>>;
  settings: Settings | null;
  profileByGame: Record<string, GameProfile>;
  backendConnected: boolean;
  gameSearchQuery: string;
  heapScanProgress: HeapScanProgressState | null;
  themeTransition: ThemeTransition | null;

  setGames: (g: GameMeta[]) => void;
  setRunning: (ids: string[]) => void;
  setActiveGame: (id: string | null) => void;
  setAttachState: (s: AttachState) => void;
  setPreflight: (gameId: string, r: PreflightReport) => void;
  setFeatures: (gameId: string, f: FeatureMeta[]) => void;
  setFeatureValue: (gameId: string, featureId: string, v: Value) => void;
  setFeatureResolutions: (gameId: string, list: FeatureResolution[]) => void;
  setFeatureResolution: (gameId: string, res: FeatureResolution) => void;
  setFeatureStatusText: (gameId: string, featureId: string, text: string | null) => void;
  setFreezeRuntime: (gameId: string, featureId: string, rt: FreezeRuntime | null) => void;
  setSettings: (s: Settings) => void;
  setProfile: (gameId: string, p: GameProfile) => void;
  setBackendConnected: (b: boolean) => void;
  setGameSearchQuery: (q: string) => void;
  setHeapScanProgress: (p: HeapScanProgressState | null) => void;
  setThemeTransition: (t: ThemeTransition | null) => void;
};

export const useAppStore = create<AppStore>((set) => ({
  games: [],
  runningGameIds: new Set(),
  activeGameId: null,
  attachState: { kind: "idle" },
  preflightByGame: {},
  featuresByGame: {},
  featureValues: {},
  featureResolutions: {},
  featureStatusTexts: {},
  freezeRuntimeByFeature: {},
  settings: null,
  profileByGame: {},
  backendConnected: false,
  gameSearchQuery: "",
  heapScanProgress: null,
  themeTransition: null,

  setGames: (g) => set({ games: g }),
  setRunning: (ids) => set({ runningGameIds: new Set(ids) }),
  setActiveGame: (id) => set({ activeGameId: id }),
  setAttachState: (s) => set({ attachState: s }),
  setPreflight: (gameId, r) =>
    set((state) => ({ preflightByGame: { ...state.preflightByGame, [gameId]: r } })),
  setFeatures: (gameId, f) =>
    set((state) => ({ featuresByGame: { ...state.featuresByGame, [gameId]: f } })),
  setFeatureValue: (gameId, featureId, v) =>
    set((state) => {
      const game = state.featureValues[gameId] ?? {};
      return {
        featureValues: {
          ...state.featureValues,
          [gameId]: { ...game, [featureId]: v },
        },
      };
    }),
  setFeatureResolutions: (gameId, list) =>
    set((state) => {
      const map: Record<string, FeatureResolution> = {};
      for (const r of list) map[r.featureId] = r;
      return {
        featureResolutions: { ...state.featureResolutions, [gameId]: map },
      };
    }),
  setFeatureResolution: (gameId, res) =>
    set((state) => {
      const game = state.featureResolutions[gameId] ?? {};
      return {
        featureResolutions: {
          ...state.featureResolutions,
          [gameId]: { ...game, [res.featureId]: res },
        },
      };
    }),
  setFeatureStatusText: (gameId, featureId, text) =>
    set((state) => {
      const game = state.featureStatusTexts[gameId] ?? {};
      return {
        featureStatusTexts: {
          ...state.featureStatusTexts,
          [gameId]: { ...game, [featureId]: text },
        },
      };
    }),
  setFreezeRuntime: (gameId, featureId, rt) =>
    set((state) => {
      const game = { ...(state.freezeRuntimeByFeature[gameId] ?? {}) };
      if (rt === null) {
        delete game[featureId];
      } else {
        game[featureId] = rt;
      }
      return {
        freezeRuntimeByFeature: {
          ...state.freezeRuntimeByFeature,
          [gameId]: game,
        },
      };
    }),
  setSettings: (s) => set({ settings: s }),
  setProfile: (gameId, p) =>
    set((state) => ({ profileByGame: { ...state.profileByGame, [gameId]: p } })),
  setBackendConnected: (b) => set({ backendConnected: b }),
  setGameSearchQuery: (q) => set({ gameSearchQuery: q }),
  setHeapScanProgress: (p) => set({ heapScanProgress: p }),
  setThemeTransition: (t) => set({ themeTransition: t }),
}));
