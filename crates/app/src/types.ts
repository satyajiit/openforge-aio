export type ValueKind =
  | "bool"
  | "u8" | "u16" | "u32" | "u64"
  | "i8" | "i16" | "i32" | "i64"
  | "f32" | "f64";

export type Value =
  | { kind: "bool"; v: boolean }
  | { kind: "u8"; v: number }   | { kind: "u16"; v: number } | { kind: "u32"; v: number } | { kind: "u64"; v: number }
  | { kind: "i8"; v: number }   | { kind: "i16"; v: number } | { kind: "i32"; v: number } | { kind: "i64"; v: number }
  | { kind: "f32"; v: number }  | { kind: "f64"; v: number };

export type WriteStrategyKind = "one_shot" | "freeze" | "code_patch";

export type ValueRange = { min: number | null; max: number | null };

export type GameMeta = {
  id: string;
  displayName: string;
  tagline: string;
  processNames: string[];
  primaryModule: string | null;
  supportedVersions: string[];
  forbiddenServices: string[];
  requiresAdmin: boolean;
  sortOrder: number;
  iconDataUrl: string;
};

export type Preset = number | { label: string; value: number };

export type ControlSpec =
  | { kind: "switch" }
  | { kind: "input"; presets?: Preset[]; step?: number | null }
  | { kind: "slider"; min: number; max: number; step?: number | null };

export type FeatureCategory = "trainer" | "mod";

export type FeatureMeta = {
  id: string;
  displayName: string;
  tagline: string | null;
  icon: string | null;
  /** Optional CSS color for the icon stroke. When null, the UI falls
   *  back to a tier-derived hue (`tierColor(tier).fg`). */
  iconColor: string | null;
  tier: string;
  /** Top-level UI bucket — "trainer" (cheats) or "mod" (gameplay extras).
   * Drives the Trainer/Mods tab split inside a game's details pane. */
  category: FeatureCategory;
  kind: ValueKind;
  range: ValueRange;
  strategy: WriteStrategyKind;
  control: ControlSpec | null;
};

export type PreflightCheck = { id: string; ok: boolean };
export type PreflightReport = { checks: PreflightCheck[] };

export type AttachState =
  | { kind: "idle" }
  | { kind: "gameSelected"; gameId: string }
  | { kind: "gameRunning"; gameId: string }
  | { kind: "attaching"; gameId: string }
  | { kind: "resolvingAobs"; gameId: string; resolved: number; total: number }
  | { kind: "attached"; gameId: string; pid: number; detectedVersion: string }
  | { kind: "error"; message: string };

export type ResolutionStatus = "resolved" | "pending" | "failed";
export type FeatureResolution = {
  featureId: string;
  ok: boolean;
  error: string | null;
  /** Tri-state present on attach payload + every feature_resolved event.
   * `resolved`: addressable.
   * `pending`: live game state not loaded yet; backend retries every ~3s.
   * `failed`: permanent (config or build mismatch); needs user action. */
  status?: ResolutionStatus;
};
export type AttachInfo = { pid: number; detectedVersion: string; perFeatureResolution: FeatureResolution[] };

export type Settings = {
  schemaVersion: number;
  themeMode: "light" | "dark";
  lastActiveGame: string | null;
  hotkeysEnabled: boolean;
  telemetry: "off";
};

export type FeatureProfile = {
  value?: Value;
  frozen?: boolean;
  codePatchApplied?: boolean;
  autoApplyOnAttach?: boolean;
};

export type GameProfile = {
  schemaVersion: number;
  gameId: string;
  lastSeenVersion: string | null;
  featureState: Record<string, FeatureProfile>;
};

export type AttachStatusEvent = { state: AttachState; gameId?: string; reason?: string };
export type FeatureResolvedEvent = {
  gameId: string;
  featureId: string;
  ok: boolean;
  error?: string;
  status?: ResolutionStatus;
};
export type HeapScanProgressEvent = {
  gameId: string;
  featureId: string;
  currentBytes: number;
  totalBytes: number;
};
export type FeatureChangedEvent = { gameId: string; featureId: string; value: Value };

/** Health of a freeze loop. "active" = last tick wrote successfully;
 *  "waiting" = recent ticks failed (e.g. we're on the main menu and the
 *  live UObject hasn't been re-instantiated yet). The freeze switch stays
 *  ON in both cases — the loop self-heals automatically. */
export type FeatureFreezeState = "active" | "waiting";
export type FeatureFreezeStateEvent = {
  gameId: string;
  featureId: string;
  state: FeatureFreezeState;
  hint: string | null;
};
export type ProcessStateEvent = { running: string[] };
export type VersionWarningEvent = { gameId: string; detectedVersion: string; supported: string[] };
export type PreflightChangedEvent = { gameId: string; report: PreflightReport };

export type AppError = { kind: string; message: string };
