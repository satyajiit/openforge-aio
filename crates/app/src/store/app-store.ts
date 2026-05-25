import { create } from "zustand";

import type {
  AttachState,
  FeatureFreezeState,
  FeatureMeta,
  FeatureResolution,
  GameMeta,
  GameProfile,
  LuaScript,
  LuaValidation,
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
  /** Per-feature freeze ON/OFF state. Lifted out of FeatureCard's local
   * state so a hotkey toggle (which dispatches on the Rust side and
   * never goes through the React click handler) can still drive the
   * Switch's `checked` prop. Set by both the FE click path and the
   * `feature_freeze_toggled` event the backend emits from the hotkey
   * dispatch. Absent entries = freeze OFF. */
  freezeOnByFeature: Record<string, Record<string, boolean>>;
  /** Per-game keybinds, mirrored from the Rust side. Populated on
   * attach via `ipc.listKeybinds(gameId)`. Drives the chord badge on
   * the FeatureCard's Assign affordance. */
  keybindsByGame: Record<string, Record<string, string>>;
  settings: Settings | null;
  profileByGame: Record<string, GameProfile>;
  backendConnected: boolean;
  gameSearchQuery: string;
  heapScanProgress: HeapScanProgressState | null;
  themeTransition: ThemeTransition | null;

  /** Per-game Lua script listings, keyed by source. The two sources are
   * tracked separately so the source toggle can render an instant count
   * without re-filtering on every render. Populated lazily when the user
   * opens the Lua tab for a game. */
  luaScriptsByGame: Record<string, { user: LuaScript[]; community: LuaScript[] }>;
  /** Currently-selected script slug in the Lua editor, scoped per game.
   * `null` = nothing selected (show empty-state). The source is tracked
   * separately as `luaSelectedSourceByGame` so re-selecting the same slug
   * across sources stays unambiguous. */
  luaSelectedByGame: Record<string, { source: "user" | "community"; slug: string } | null>;
  /** Latest parse-validation result, keyed by `<gameId>:<source>:<slug>`.
   * Used both for the inline status pill and CodeMirror's lint gutter. */
  luaValidationByScript: Record<string, LuaValidation>;
  /** Per-script in-memory edit buffer. Decouples the editor's working copy
   * from the on-disk script so the "Save" CTA is meaningful (dirty
   * indicator + Cmd-S). Keyed identically to `luaValidationByScript`. */
  luaDraftByScript: Record<string, string>;
  /** Per-script Lua execution state: console buffer, running flag, last
   *  error. Keyed identically to `luaValidationByScript`
   *  (`${gameId}:${source}:${slug}`). Lines are capped at
   *  `LUA_CONSOLE_MAX_LINES` (10_000) — older lines are dropped FIFO. */
  luaRunByScript: Record<string, LuaRunState>;

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
  setFreezeOn: (gameId: string, featureId: string, on: boolean) => void;
  setKeybindsForGame: (gameId: string, entries: { featureId: string; chord: string }[]) => void;
  setKeybind: (gameId: string, featureId: string, chord: string | null) => void;
  clearGameRuntimeState: (gameId: string) => void;
  setSettings: (s: Settings) => void;
  setProfile: (gameId: string, p: GameProfile) => void;
  setBackendConnected: (b: boolean) => void;
  setGameSearchQuery: (q: string) => void;
  setHeapScanProgress: (p: HeapScanProgressState | null) => void;
  setThemeTransition: (t: ThemeTransition | null) => void;

  setLuaScripts: (gameId: string, scripts: LuaScript[]) => void;
  upsertLuaScript: (gameId: string, script: LuaScript) => void;
  removeLuaScript: (gameId: string, source: "user" | "community", slug: string) => void;
  setLuaSelected: (
    gameId: string,
    selection: { source: "user" | "community"; slug: string } | null,
  ) => void;
  setLuaValidation: (key: string, v: LuaValidation) => void;
  setLuaDraft: (key: string, code: string) => void;
  clearLuaDraft: (key: string) => void;

  appendLuaLines: (key: string, lines: LuaConsoleLine[]) => void;
  setLuaRunning: (
    key: string,
    running: boolean,
    name: string | null,
    lastError: string | null,
  ) => void;
  clearLuaOutput: (key: string) => void;
};

/** One line in the console panel. Matches `LuaOutputLineDto` from the wire
 *  but lives in store so the FE can also synthesize entries (e.g. a local
 *  "stopped by user" marker) without needing a backend round-trip. */
export type LuaConsoleLine = {
  level: "info" | "warn" | "error";
  message: string;
  timestampMs: number;
};

export type LuaRunState = {
  running: boolean;
  lastError: string | null;
  /** `Date.now()` at `setLuaRunning(true, …)`; `null` when never run. */
  startedAt: number | null;
  lines: LuaConsoleLine[];
};

/** Hard cap on per-script console buffer. A chatty script that produces
 *  > 10k lines drops the oldest in O(1) via `Array.prototype.slice(-N)`. */
export const LUA_CONSOLE_MAX_LINES = 10_000;

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
  freezeOnByFeature: {},
  keybindsByGame: {},
  settings: null,
  profileByGame: {},
  backendConnected: false,
  gameSearchQuery: "",
  heapScanProgress: null,
  themeTransition: null,
  luaScriptsByGame: {},
  luaSelectedByGame: {},
  luaValidationByScript: {},
  luaDraftByScript: {},
  luaRunByScript: {},

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
  setFreezeOn: (gameId, featureId, on) =>
    set((state) => {
      const game = { ...(state.freezeOnByFeature[gameId] ?? {}) };
      if (on) game[featureId] = true;
      else delete game[featureId];
      return {
        freezeOnByFeature: { ...state.freezeOnByFeature, [gameId]: game },
      };
    }),
  setKeybindsForGame: (gameId, entries) =>
    set((state) => {
      const map: Record<string, string> = {};
      for (const e of entries) map[e.featureId] = e.chord;
      return { keybindsByGame: { ...state.keybindsByGame, [gameId]: map } };
    }),
  setKeybind: (gameId, featureId, chord) =>
    set((state) => {
      const game = { ...(state.keybindsByGame[gameId] ?? {}) };
      if (chord === null) delete game[featureId];
      else game[featureId] = chord;
      return { keybindsByGame: { ...state.keybindsByGame, [gameId]: game } };
    }),
  clearGameRuntimeState: (gameId) =>
    set((state) => {
      const fr = { ...state.freezeRuntimeByFeature };
      delete fr[gameId];
      const fo = { ...state.freezeOnByFeature };
      delete fo[gameId];
      return { freezeRuntimeByFeature: fr, freezeOnByFeature: fo };
    }),
  setSettings: (s) => set({ settings: s }),
  setProfile: (gameId, p) =>
    set((state) => ({ profileByGame: { ...state.profileByGame, [gameId]: p } })),
  setBackendConnected: (b) => set({ backendConnected: b }),
  setGameSearchQuery: (q) => set({ gameSearchQuery: q }),
  setHeapScanProgress: (p) => set({ heapScanProgress: p }),
  setThemeTransition: (t) => set({ themeTransition: t }),

  setLuaScripts: (gameId, scripts) =>
    set((state) => {
      const user: LuaScript[] = [];
      const community: LuaScript[] = [];
      for (const s of scripts) {
        if (s.source === "user") user.push(s);
        else community.push(s);
      }
      return {
        luaScriptsByGame: {
          ...state.luaScriptsByGame,
          [gameId]: { user, community },
        },
      };
    }),
  upsertLuaScript: (gameId, script) =>
    set((state) => {
      const existing = state.luaScriptsByGame[gameId] ?? { user: [], community: [] };
      const bucket = script.source === "user" ? "user" : "community";
      const next = existing[bucket].filter((s) => s.slug !== script.slug);
      next.push(script);
      next.sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
      return {
        luaScriptsByGame: {
          ...state.luaScriptsByGame,
          [gameId]: { ...existing, [bucket]: next },
        },
      };
    }),
  removeLuaScript: (gameId, source, slug) =>
    set((state) => {
      const existing = state.luaScriptsByGame[gameId] ?? { user: [], community: [] };
      const next = existing[source].filter((s) => s.slug !== slug);
      return {
        luaScriptsByGame: {
          ...state.luaScriptsByGame,
          [gameId]: { ...existing, [source]: next },
        },
      };
    }),
  setLuaSelected: (gameId, selection) =>
    set((state) => ({
      luaSelectedByGame: { ...state.luaSelectedByGame, [gameId]: selection },
    })),
  setLuaValidation: (key, v) =>
    set((state) => ({
      luaValidationByScript: { ...state.luaValidationByScript, [key]: v },
    })),
  setLuaDraft: (key, code) =>
    set((state) => ({
      luaDraftByScript: { ...state.luaDraftByScript, [key]: code },
    })),
  clearLuaDraft: (key) =>
    set((state) => {
      const next = { ...state.luaDraftByScript };
      delete next[key];
      return { luaDraftByScript: next };
    }),

  appendLuaLines: (key, lines) =>
    set((state) => {
      const prev = state.luaRunByScript[key] ?? {
        running: false,
        lastError: null,
        startedAt: null,
        lines: [],
      };
      const merged =
        prev.lines.length + lines.length <= LUA_CONSOLE_MAX_LINES
          ? [...prev.lines, ...lines]
          : [...prev.lines, ...lines].slice(-LUA_CONSOLE_MAX_LINES);
      return {
        luaRunByScript: {
          ...state.luaRunByScript,
          [key]: { ...prev, lines: merged },
        },
      };
    }),

  setLuaRunning: (key, running, _name, lastError) =>
    set((state) => {
      const prev = state.luaRunByScript[key] ?? {
        running: false,
        lastError: null,
        startedAt: null,
        lines: [],
      };
      const next: LuaRunState = running
        ? {
            // Fresh run: clear buffers so the console starts empty.
            running: true,
            lastError: null,
            startedAt: Date.now(),
            lines: [],
          }
        : {
            // Stop / error: keep the lines so the user can scroll back.
            ...prev,
            running: false,
            lastError,
          };
      return {
        luaRunByScript: { ...state.luaRunByScript, [key]: next },
      };
    }),

  clearLuaOutput: (key) =>
    set((state) => {
      const prev = state.luaRunByScript[key];
      if (!prev) return {};
      return {
        luaRunByScript: {
          ...state.luaRunByScript,
          [key]: { ...prev, lines: [], lastError: null },
        },
      };
    }),
}));
