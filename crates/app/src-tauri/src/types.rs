//! IPC DTOs.

use serde::Serialize;

use openforge_runtime::{ControlSpec, Value, ValueKind, WriteStrategyKind};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameMeta {
    pub id: String,
    pub display_name: String,
    pub tagline: String,
    pub process_names: Vec<String>,
    pub primary_module: Option<String>,
    pub supported_versions: Vec<String>,
    pub forbidden_services: Vec<String>,
    pub requires_admin: bool,
    pub sort_order: i32,
    pub icon_data_url: String,
}

impl From<openforge_runtime::GameMeta> for GameMeta {
    fn from(m: openforge_runtime::GameMeta) -> Self {
        Self {
            id: m.id,
            display_name: m.display_name,
            tagline: m.tagline,
            process_names: m.process_names,
            primary_module: m.primary_module,
            supported_versions: m.supported_versions,
            forbidden_services: m.forbidden_services,
            requires_admin: m.requires_admin,
            sort_order: m.sort_order,
            icon_data_url: m.icon_data_url,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureMeta {
    pub id: String,
    pub display_name: String,
    pub tagline: Option<String>,
    pub icon: Option<String>,
    /// CSS color string for the icon stroke. `None` lets the frontend
    /// fall back to a tier-derived hue.
    pub icon_color: Option<String>,
    pub tier: String,
    /// Top-level UI category: `"trainer"` (default) or `"mod"`. Drives which
    /// top-level tab the feature lands in. Sourced from `meta.kind` in the
    /// signature TOML.
    pub category: String,
    pub kind: ValueKind,
    pub range: ValueRange,
    pub strategy: WriteStrategyKind,
    pub control: Option<ControlSpec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValueRange {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightCheck {
    pub id: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachInfo {
    pub pid: u32,
    pub detected_version: String,
    pub per_feature_resolution: Vec<FeatureResolution>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureResolution {
    pub feature_id: String,
    /// `true` iff the feature is fully Resolved (has an addressable target).
    /// Kept for back-compat with frontend code that branches on resolution
    /// success; new code should prefer `status` for the tri-state.
    pub ok: bool,
    pub error: Option<String>,
    /// Tri-state: `"resolved"` (addressable), `"pending"` (transient miss,
    /// will auto-retry), `"failed"` (permanent — e.g. bad TOML or class
    /// missing on this build). Frontend renders different hints per state.
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStateEvent {
    pub running: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachStatusEvent {
    pub state: AttachStatePayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AttachStatePayload {
    Idle,
    GameSelected {
        game_id: String,
    },
    GameRunning {
        game_id: String,
    },
    Attaching {
        game_id: String,
    },
    ResolvingAobs {
        game_id: String,
        resolved: usize,
        total: usize,
    },
    Finalizing {
        game_id: String,
    },
    Attached {
        game_id: String,
        pid: u32,
        detected_version: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureResolvedEvent {
    pub game_id: String,
    pub feature_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `"resolved"` | `"pending"` | `"failed"`. Always present on events
    /// always emitted now; older event sites that still use the
    /// `..Default::default()` shorthand may omit it (treat as `"failed"`
    /// when `ok=false`, `"resolved"` when `ok=true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureChangedEvent {
    pub game_id: String,
    pub feature_id: String,
    pub value: Value,
}

/// Runtime health of a freeze loop. Emitted on transitions
/// (active ↔ waiting) so the UI can render a loader badge while a
/// reflection-backed freeze (e.g. Lock Health) is patiently re-resolving
/// across a level transition or main-menu round-trip. Not emitted on every
/// tick — only when the state changes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureFreezeStateEvent {
    pub game_id: String,
    pub feature_id: String,
    /// `"active"` = last tick wrote successfully; freeze is holding.
    /// `"waiting"` = recent ticks failed; freeze loop is still running and
    /// will resume automatically once the world reloads (PlayerState etc.
    /// becomes available again).
    pub state: String,
    /// One-line hint surfaced next to the loader for "waiting". `None`
    /// when active. Currently only "Waiting for the level to load…" but
    /// could differentiate per error class later.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightChangedEvent {
    pub game_id: String,
    pub report: PreflightReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionWarningEvent {
    pub game_id: String,
    pub detected_version: String,
    pub supported: Vec<String>,
}

/// Heap-scan progress tick. Fired by the backend ~20×/sec during a
/// `heap_value_scan` resolve so the UI can render a real progress bar
/// instead of a vague spinner.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeapScanProgressEvent {
    pub game_id: String,
    pub feature_id: String,
    pub current_bytes: u64,
    pub total_bytes: u64,
}

// ---- Keybind DTOs ---------------------------------------------------

/// One row in the per-game keybind table returned by `list_keybinds`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindEntry {
    pub feature_id: String,
    pub chord: String,
}

/// Outcome of a `set_keybind` call. The conflict variant lets the
/// frontend show the "already mapped to X — override?" dialog without
/// a second round-trip.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SetKeybindResult {
    /// Saved AND registered with the OS. `chord` is the canonicalized
    /// form (what gets shown in the UI badge).
    Ok { chord: String },
    /// `chord` is already bound to `existing_feature_id` in `existing_game_id`.
    /// FE shows the override prompt; on confirm, FE calls
    /// `set_keybind` again with `override = true`.
    Conflict {
        chord: String,
        existing_game_id: String,
        existing_feature_id: String,
        existing_display_name: String,
    },
    /// The chord couldn't be parsed into a valid Tauri shortcut.
    Invalid { reason: String },
    /// The chord parsed OK and we tried to install it, but the OS
    /// refused — typically because another app (or WebView2's
    /// built-in devtools shortcut for plain `F12`) already owns it.
    /// Nothing was persisted; FE prompts for a different chord.
    OsRegisterFailed { chord: String, reason: String },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    ToggleOn,
    ToggleOff,
    Fire,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyFiredEvent {
    pub game_id: String,
    pub feature_id: String,
    pub display_name: String,
    pub action: HotkeyAction,
}

/// Emitted when freeze is toggled by a path that doesn't otherwise
/// surface the new state to the FE (currently: the global-hotkey
/// dispatch). The FE's freeze-on store slice listens for this so the
/// SwitchControl's checked state stays in sync.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureFreezeToggledEvent {
    pub game_id: String,
    pub feature_id: String,
    pub frozen: bool,
}

// ---- Lua runtime events (protocol v5) ------------------------------------

/// Single line of captured `print()` (or runtime-emitted) output from a
/// running Lua script. Wire shape matches `openforge_ue5_protocol::LuaOutputLine`
/// but is camelCased + lowercased-level for the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaOutputLineDto {
    /// `"info" | "warn" | "error"`.
    pub level: String,
    pub message: String,
    pub timestamp_ms: u64,
}

/// Emitted continuously while a Lua script is running. Each event carries
/// up to ~64 lines drained from the DLL's ring buffer in one polling tick
/// (250 ms cadence). The frontend's `lua-console` subscribes to
/// `lua_output` and appends to a per-script buffer in zustand.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaOutputEvent {
    pub game_id: String,
    /// `"user"` or `"community"` — the source bucket the script lives in.
    pub source: String,
    pub slug: String,
    pub lines: Vec<LuaOutputLineDto>,
}

/// Emitted on Lua VM lifecycle transitions: starts (`running = true`),
/// clean stops (`running = false, last_error = None`), and runtime errors
/// (`running = false, last_error = Some(traceback)`). Frontend uses this
/// to flip the Run/Stop button and surface error toasts.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaScriptStatusEvent {
    pub game_id: String,
    pub source: String,
    pub slug: String,
    pub running: bool,
    pub name: Option<String>,
    pub last_error: Option<String>,
}

pub fn feature_meta_from(f: &dyn openforge_runtime::Feature) -> FeatureMeta {
    FeatureMeta {
        id: f.id().to_string(),
        display_name: f.display_name().to_string(),
        tagline: f.tagline().map(str::to_string),
        icon: f.icon().map(str::to_string),
        icon_color: f.icon_color().map(str::to_string),
        tier: f.tier().to_string(),
        category: f.category().to_string(),
        kind: f.kind(),
        range: ValueRange {
            min: f.range().min,
            max: f.range().max,
        },
        strategy: f.write_strategy(),
        control: f.control().cloned(),
    }
}
