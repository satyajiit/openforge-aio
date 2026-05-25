import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AttachInfo,
  AttachStatusEvent,
  FeatureChangedEvent,
  FeatureFreezeStateEvent,
  FeatureFreezeToggledEvent,
  FeatureMeta,
  FeatureResolution,
  FeatureResolvedEvent,
  GameMeta,
  GameProfile,
  HeapScanProgressEvent,
  HotkeyFiredEvent,
  KeybindEntry,
  LuaScript,
  LuaSource,
  LuaValidation,
  PreflightChangedEvent,
  PreflightReport,
  ProcessStateEvent,
  SetKeybindResult,
  Settings,
  Value,
  VersionWarningEvent,
} from "@/types";

const isTauri = (): boolean => {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
};

async function call<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!isTauri()) {
    throw new Error(`Tauri backend not available (called ${name})`);
  }
  return invoke<T>(name, args);
}

export const ipc = {
  isTauri,
  listGames: () => call<GameMeta[]>("list_games"),
  listFeatures: (gameId: string) => call<FeatureMeta[]>("list_features", { gameId }),
  startWatcher: () => call<void>("start_watcher"),
  stopWatcher: () => call<void>("stop_watcher"),
  getProcessState: () => call<ProcessStateEvent>("get_process_state"),
  preflight: (gameId: string) => call<PreflightReport>("preflight", { gameId }),
  attach: (gameId: string) => call<AttachInfo>("attach", { gameId }),
  detach: () => call<void>("detach"),
  readFeature: (gameId: string, featureId: string) =>
    call<Value>("read_feature", { gameId, featureId }),
  readFeatures: (gameId: string, featureIds: string[]) =>
    call<[string, Value][]>("read_features", { gameId, featureIds }),
  writeFeature: (gameId: string, featureId: string, value: Value) =>
    call<Value>("write_feature", { gameId, featureId, value }),
  /** Optional rich status string for a feature (e.g. "20 / 30 unlocked"
   * for the SetProgressTags-class features). `null` when the feature
   * doesn't override status_text. Backend call is on-demand: not safe to
   * call from a tight loop. */
  featureStatusText: (gameId: string, featureId: string) =>
    call<string | null>("feature_status_text", { gameId, featureId }),
  setFreeze: (gameId: string, featureId: string, frozen: boolean) =>
    call<void>("set_freeze", { gameId, featureId, frozen }),
  retryResolve: (gameId: string, featureId: string) =>
    call<FeatureResolution>("retry_resolve", { gameId, featureId }),
  setCodePatch: (gameId: string, featureId: string, applied: boolean) =>
    call<void>("set_code_patch", { gameId, featureId, applied }),
  isElevated: () => call<boolean>("is_elevated"),
  relaunchAsAdmin: () => call<void>("relaunch_as_admin"),
  getSettings: () => call<Settings>("get_settings"),
  setSettings: (settings: Settings) => call<void>("set_settings", { settings }),
  getProfile: (gameId: string) => call<GameProfile>("get_profile", { gameId }),
  saveProfile: (gameId: string, profile: GameProfile) =>
    call<void>("save_profile", { gameId, profile }),
  openLogFolder: () => call<void>("open_log_folder"),
  listKeybinds: (gameId: string) => call<KeybindEntry[]>("list_keybinds", { gameId }),
  setKeybind: (
    gameId: string,
    featureId: string,
    chord: string,
    overrideConflict: boolean,
  ) =>
    call<SetKeybindResult>("set_keybind", {
      gameId,
      featureId,
      chord,
      overrideConflict,
    }),
  clearKeybind: (gameId: string, featureId: string) =>
    call<void>("clear_keybind", { gameId, featureId }),
  checkKeybindConflict: (chord: string) =>
    call<KeybindEntry | null>("check_keybind_conflict", { chord }),

  // ---- Lua scripts -------------------------------------------------
  listLuaScripts: (gameId: string) =>
    call<LuaScript[]>("list_lua_scripts", { gameId }),
  readLuaScript: (gameId: string, source: LuaSource, slug: string) =>
    call<string>("read_lua_script", { gameId, source, slug }),
  saveUserLuaScript: (
    gameId: string,
    slug: string,
    name: string,
    code: string,
  ) =>
    call<LuaScript>("save_user_lua_script", { gameId, slug, name, code }),
  deleteUserLuaScript: (gameId: string, slug: string) =>
    call<void>("delete_user_lua_script", { gameId, slug }),
  validateLuaScript: (code: string) =>
    call<LuaValidation>("validate_lua_script", { code }),
  refreshCommunityLuaIndex: (gameId: string) =>
    call<LuaScript[]>("refresh_community_lua_index", { gameId }),
  installCommunityLuaScript: (gameId: string, slug: string) =>
    call<LuaScript>("install_community_lua_script", { gameId, slug }),
  runLuaScript: (gameId: string, source: LuaSource, slug: string) =>
    call<void>("run_lua_script", { gameId, source, slug }),
};

async function subscribe<T>(eventName: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (!isTauri()) {
    return () => {};
  }
  return listen<T>(eventName, (e) => cb(e.payload));
}

export const events = {
  onProcessState: (cb: (e: ProcessStateEvent) => void) =>
    subscribe<ProcessStateEvent>("process_state", cb),
  onAttachStatus: (cb: (e: AttachStatusEvent) => void) =>
    subscribe<AttachStatusEvent>("attach_status", cb),
  onFeatureResolved: (cb: (e: FeatureResolvedEvent) => void) =>
    subscribe<FeatureResolvedEvent>("feature_resolved", cb),
  onFeatureChanged: (cb: (e: FeatureChangedEvent) => void) =>
    subscribe<FeatureChangedEvent>("feature_changed", cb),
  onFeatureFreezeState: (cb: (e: FeatureFreezeStateEvent) => void) =>
    subscribe<FeatureFreezeStateEvent>("feature_freeze_state", cb),
  onPreflightChanged: (cb: (e: PreflightChangedEvent) => void) =>
    subscribe<PreflightChangedEvent>("preflight_changed", cb),
  onVersionWarning: (cb: (e: VersionWarningEvent) => void) =>
    subscribe<VersionWarningEvent>("version_warning", cb),
  onHeapScanProgress: (cb: (e: HeapScanProgressEvent) => void) =>
    subscribe<HeapScanProgressEvent>("heap_scan_progress", cb),
  onHotkeyFired: (cb: (e: HotkeyFiredEvent) => void) =>
    subscribe<HotkeyFiredEvent>("hotkey_fired", cb),
  onFeatureFreezeToggled: (cb: (e: FeatureFreezeToggledEvent) => void) =>
    subscribe<FeatureFreezeToggledEvent>("feature_freeze_toggled", cb),
};
