//! Signature TOML schema. Parsed once per feature at startup; the resulting
//! [`SignatureSpec`] drives the declarative engine in `feature.rs`.

use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};
use crate::value::ValueKind;

#[derive(Debug, Clone, Deserialize)]
pub struct SignatureSpec {
    pub meta: Meta,
    #[serde(default)]
    pub value: Option<ValueSpec>,
    pub write: WriteSpec,
    /// AOB-based locator. Mutually exclusive with `heap_scan` — exactly one of
    /// the two must be present. Required for `code_patch` strategies (since
    /// they patch instruction bytes in `.text`).
    #[serde(default)]
    pub locator: Option<LocatorSpec>,
    /// Heap-scan locator. Used when the target is a dynamically-allocated
    /// object whose address shifts across game sessions (no static anchor).
    /// At attach time, the runtime scans all RW pages for a structural
    /// fingerprint value, validates each candidate, and picks the unique
    /// winner.
    #[serde(default)]
    pub heap_scan: Option<HeapScanSpec>,
    /// Reflection-based locator. The runtime asks the injected
    /// DLL to find a live UObject by class name + predicate, then resolves
    /// the named FProperty's offset from the UClass. No address caching
    /// across runs — re-discovery is a single GUObjectArray walk per
    /// session. Mutually exclusive with `[locator]` and `[heap_scan]`.
    #[serde(default)]
    pub reflection: Option<ReflectionSpec>,
    #[serde(default)]
    pub pointer_chain: Option<PointerChainSpec>,
    #[serde(default)]
    pub control: Option<ControlSpec>,
}

impl SignatureSpec {
    pub fn parse(toml_text: &str) -> RuntimeResult<Self> {
        toml::from_str(toml_text).map_err(|e| RuntimeError::SignatureParse(e.to_string()))
    }

    /// The kind exposed to the UI. CodePatch + SetProgressTags features
    /// are bool action triggers regardless of any (likely absent) `[value]`
    /// section.
    pub fn effective_kind(&self) -> ValueKind {
        match self.write {
            WriteSpec::CodePatch { .. }
            | WriteSpec::SetProgressTags { .. }
            | WriteSpec::CallInstanceUFunction { .. }
            | WriteSpec::TeleportToWaypoint { .. }
            | WriteSpec::FreezeForMatching { .. } => ValueKind::Bool,
            _ => self
                .value
                .as_ref()
                .map(|v| v.kind)
                .unwrap_or(ValueKind::I32),
        }
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        // SetProgressTags carries its own discovery (asset_class + predicate)
        // inside `[write.source]` and is action-triggered (no readable value).
        // Skip both the [value] requirement and the locator-block requirement
        // for this strategy.
        let is_set_progress_tags = matches!(self.write, WriteSpec::SetProgressTags { .. });

        if matches!(self.write, WriteSpec::OneShot | WriteSpec::Freeze { .. })
            && self.value.is_none()
        {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.meta.feature.clone(),
                reason: "[value] section is required for one_shot and freeze strategies".into(),
            });
        }
        // FreezeForMatching also carries its own discovery (class_path +
        // FQN filters in the write block). Treat it like SetProgressTags
        // — no [value]/locator block required at the SignatureSpec level.
        let is_freeze_for_matching = matches!(self.write, WriteSpec::FreezeForMatching { .. });
        if is_freeze_for_matching {
            if self.locator.is_some() || self.heap_scan.is_some() || self.reflection.is_some() {
                return Err(RuntimeError::SignatureInvalid {
                    feature: self.meta.feature.clone(),
                    reason: "freeze_for_matching carries its own discovery via `class_path` + \
                         FQN filters; remove [locator] / [heap_scan] / [reflection]"
                        .into(),
                });
            }
            if let WriteSpec::FreezeForMatching {
                class_path,
                field_offsets,
                ..
            } = &self.write
                && (class_path.trim().is_empty() || field_offsets.is_empty())
            {
                return Err(RuntimeError::SignatureInvalid {
                    feature: self.meta.feature.clone(),
                    reason: "freeze_for_matching requires non-empty `class_path` and at least one \
                         `field_offsets` entry"
                        .into(),
                });
            }
            return Ok(());
        }
        if !is_set_progress_tags {
            let locator_count = (self.locator.is_some() as u8)
                + (self.heap_scan.is_some() as u8)
                + (self.reflection.is_some() as u8);
            match locator_count {
                0 => {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason:
                            "exactly one of [locator], [heap_scan], or [reflection] is required"
                                .into(),
                    });
                }
                1 => {}
                _ => {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "[locator], [heap_scan], and [reflection] are mutually exclusive"
                            .into(),
                    });
                }
            }
        } else if self.locator.is_some() || self.heap_scan.is_some() || self.reflection.is_some() {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.meta.feature.clone(),
                reason: "set_progress_tags carries its own asset discovery in [write.source]; \
                         remove [locator] / [heap_scan] / [reflection]"
                    .into(),
            });
        }
        if matches!(self.write, WriteSpec::CodePatch { .. })
            && (self.heap_scan.is_some() || self.reflection.is_some())
        {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.meta.feature.clone(),
                reason: "code_patch requires an AOB [locator] (heap/reflection target objects, not .text)".into(),
            });
        }
        if let WriteSpec::CallInstanceUFunction { params_on, .. } = &self.write {
            let ref_spec =
                self.reflection
                    .as_ref()
                    .ok_or_else(|| RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason:
                            "call_instance_ufunction requires a [reflection] block — the chain's \
                         final hop's pointer slot is read to obtain the call target"
                                .into(),
                    })?;
            if ref_spec.deref.is_empty() {
                return Err(RuntimeError::SignatureInvalid {
                    feature: self.meta.feature.clone(),
                    reason: "call_instance_ufunction requires at least one [[reflection.deref]] \
                         entry — the final hop names the ObjectProperty pointing at the call target"
                        .into(),
                });
            }
            if parse_hex_payload(params_on).is_err() {
                return Err(RuntimeError::SignatureInvalid {
                    feature: self.meta.feature.clone(),
                    reason: format!(
                        "call_instance_ufunction: params_on `{params_on}` is not a valid \
                         hex string (whitespace-tolerant, two hex digits per byte)"
                    ),
                });
            }
        }
        if let WriteSpec::TeleportToWaypoint { .. } = &self.write
            && self.reflection.is_none()
        {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.meta.feature.clone(),
                reason: "teleport_to_waypoint requires a [reflection] block — the chain's \
                         final pointer slot is read to obtain the player character target"
                    .into(),
            });
        }
        // Pattern syntax is validated lazily at scan time by openforge-core.
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub feature: String,
    pub display_name: String,
    /// Short, voice-y line shown directly under the feature name in the UI.
    /// This is what a non-technical player reads; keep the dry write-up in
    /// `description`.
    #[serde(default)]
    pub tagline: Option<String>,
    /// Long-form, technical explanation. Surfaced only behind an info icon —
    /// not shown by default. Safe place for hex bytes, RIP details, etc.
    #[serde(default)]
    pub description: Option<String>,
    /// Either a `lucide-react` icon name (e.g. `"coins"`) or a path under the
    /// game's `assets/` directory (e.g. `"assets/icons/studs.png"`). The
    /// frontend decides how to render it.
    #[serde(default)]
    pub icon: Option<String>,
    /// Optional CSS color for the icon stroke (e.g. `"oklch(70% 0.13 25)"`,
    /// `"#facc15"`, `"tomato"`). When `None`, the frontend falls back to a
    /// tier-derived color. Any valid CSS color string is passed through
    /// verbatim — the frontend doesn't validate it, since contributors are
    /// the only authors and signature authoring tools can sanity-check.
    #[serde(default)]
    pub icon_color: Option<String>,
    #[serde(default = "default_tier")]
    pub tier: String,
    /// Top-level UI categorization. `"trainer"` (default) = the cheats
    /// (currencies, locks, action triggers). `"mod"` = cosmetic /
    /// gameplay-altering extras (dance mode, character swaps, etc.). The
    /// frontend renders two top-level tabs in the game details pane —
    /// Trainer and Mods — and groups features into them. Within each tab,
    /// the existing `tier` field still drives sub-grouping.
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub discovered_in_version: Option<String>,
    #[serde(default)]
    pub verified_versions: Vec<String>,
    #[serde(default)]
    pub discovered_on: Option<String>,
    #[serde(default)]
    pub discovered_by: Option<String>,
}

/// Optional UI control hint. Drives which widget the frontend renders,
/// independent of the write strategy. If absent, the frontend infers a
/// sensible default from the value kind + strategy.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlSpec {
    /// Plain on/off. Default for `code_patch`.
    Switch,
    /// Number entry with Apply. Default for `one_shot` and `freeze` on numeric kinds.
    Input {
        #[serde(default)]
        presets: Vec<Preset>,
        #[serde(default)]
        step: Option<f64>,
    },
    /// Min/max slider.
    Slider {
        min: f64,
        max: f64,
        #[serde(default)]
        step: Option<f64>,
    },
}

/// Quick-pick entry on an `Input` control. Two TOML shapes are accepted:
///
/// 1. Bare scalar — `presets = [0.5, 1.0, 2.0]`. Frontend renders the formatted
///    number as the button label. Used by every signature predating the
///    labeled-preset extension.
/// 2. Labeled object — `presets = [{ label = "Tame", value = 0.7 }, ...]`.
///    Frontend renders the label and the value becomes the applied value.
///    Use this when the preset names something (intensity, persona, mode)
///    rather than just a number.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Preset {
    Bare(f64),
    Labeled { label: String, value: f64 },
}

impl Preset {
    pub fn value(&self) -> f64 {
        match self {
            Preset::Bare(v) => *v,
            Preset::Labeled { value, .. } => *value,
        }
    }
}

fn default_tier() -> String {
    "general".into()
}

fn default_kind() -> String {
    // Backwards-compatible default: any legacy TOML (set_studs,
    // set_chips, lock_health, etc.) lands in the Trainer tab without
    // needing an edit.
    "trainer".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValueSpec {
    #[serde(rename = "type")]
    pub kind: ValueKind,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default = "default_display")]
    pub display: String,
    #[serde(default)]
    pub endian: Endian,
}

fn default_display() -> String {
    "decimal".into()
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Endian {
    #[default]
    Little,
    Big,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum WriteSpec {
    OneShot,
    Freeze {
        #[serde(default = "default_interval_ms")]
        interval_ms: u32,
        /// Optional constant to freeze at. When set, the trainer ignores
        /// the field's current value at toggle-on time and freezes to this
        /// instead. Used by `Infinite Health` to freeze `bCanBeDamaged` at
        /// `false` regardless of the player's current state (otherwise
        /// toggling on while alive would freeze at `true` — a no-op).
        ///
        /// Stored as `f64` for serde simplicity; coerced to the feature's
        /// declared [`ValueKind`] at runtime. `0.0` = `false` for `Bool`
        /// features, integer values for numeric features.
        #[serde(default)]
        freeze_value: Option<f64>,
    },
    CodePatch {
        original_bytes: String,
        patched_bytes: String,
    },
    /// Bulk-grant write: iterate a UE5 data-asset's `TArray` of progress
    /// entries, extract a registered `FGameplayTag` (FName) from each, and
    /// call a static UFunction per tag with `value`. This is the one-shot
    /// "Unlock All X" pattern that drives "Unlock All Skills",
    /// "Unlock All Fast Travel", and equivalents on other progress assets.
    ///
    /// Two source shapes are supported:
    ///   - `definitions`: TArray of 16-byte `{ClassPtr, DataPtr}` pairs
    ///     (UE5 `TArray<TObjectPtr<TtGameProgressDefinition>>`). Tag at
    ///     `DataPtr+0x4C`. Used by `PROG_*` (TtGameProgressDefinitionSet).
    ///   - `rules_values`: TArray of inline `TtGameProgressRule` (56-byte
    ///     stride). Tag at `Values[0]+0x00` where `Values` is the inner
    ///     `TArray` at `rule+0x18`. Used by `PROGR_*` (TtGameProgressRuleSet).
    ///
    /// The `Bool` feature-kind contract: `write(true)` fires the grant;
    /// `write(false)` is a no-op (we don't auto-revert grants — the player
    /// shouldn't lose their unlocks). Read always returns `false` (the
    /// per-tag query would be N round-trips; the UI renders this as an
    /// action button, not a toggle).
    SetProgressTags {
        /// Where to find the per-tag definition list.
        source: TagSourceSpec,
        /// Which static UFunction to call per tag.
        call: SetCallSpec,
        /// Value to pass as the third arg (`Value` ByteProperty) of
        /// `SetGameProgressValue`. For skill tags on this build, `2` =
        /// unlocked; `1` = locked. Embed in TOML so different feature
        /// instances (achievements, fast travel, etc.) can pick their own
        /// unlock value.
        value: u8,
        /// Optional display override for the status line. Used when the
        /// engine-side tag count includes internal/scripted entries that
        /// dwarf the player-visible count (fast travel: 65 internal tags vs
        /// 9 map terminals). The status_text reports against `total` and
        /// `label` instead of `tags.len()` / generic "unlocked".
        #[serde(default)]
        display: Option<DisplayOverrideSpec>,
    },
    /// On/off action: walks the `[reflection]` deref chain to find a live
    /// component (the final hop's resolved field must be an `ObjectProperty`,
    /// not a value-type field — the chain's last entry's *pointer slot* is
    /// read once more to yield the target object), then calls a UFunction
    /// on that target with one of two pre-encoded parameter blobs.
    ///
    /// Drives "Fly Mode" on LotDK: chain ends at `CharacterMovement`, target
    /// is the live `DinnerCharacterMovementComponent`, function is
    /// `SetMovementMode(NewMovementMode, NewCustomMode)`. Toggle ON sends
    /// `[5, 0]` (MOVE_Flying); toggle OFF sends `[1, 0]` (MOVE_Walking).
    ///
    /// `params_on` / `params_off` are hex strings (whitespace-tolerant) so
    /// signature authors can write `params_on = "05 00"` rather than
    /// hand-counting byte arrays.
    #[serde(rename = "call_instance_ufunction")]
    CallInstanceUFunction {
        /// UFunction name (case-insensitive) on the target object's UClass
        /// chain. Resolved fresh on every write — the DLL's per-class
        /// function-table cache makes this cheap after the first call.
        function: String,
        /// Hex-encoded parameter bytes for the `Bool(true)` write. Must
        /// match the UFunction's `ParmsSize` (the DLL zero-pads short
        /// payloads but refuses oversize — explicit padding is allowed).
        params_on: String,
        /// Hex-encoded parameter bytes for the `Bool(false)` write. For
        /// movement-mode toggles this is the "restore to default" payload
        /// (e.g. `"01 00"` = MOVE_Walking). A zero-byte string means
        /// "don't fire on toggle-off" — useful for one-shot effects that
        /// can't be reversed.
        #[serde(default)]
        params_off: String,
    },
    /// On-demand one-shot teleport: resolves the custom map pin's RelativeLocation,
    /// resolves the player pawn, and calls the native K2_TeleportTo UFunction on the pawn.
    #[serde(rename = "teleport_to_waypoint")]
    TeleportToWaypoint {
        /// Target map pin reflection block.
        waypoint: ReflectionSpec,
    },
    /// Bulk freeze: every tick, walk GUObjectArray for all live UObjects of
    /// `class_path` whose FQN passes the include/exclude filters, then write
    /// `value` at each one's `field_offsets`.
    ///
    /// Drives one-hit-kill: walk every `HealthAttributeSet`, exclude the
    /// player's and allies' (FQN contains `_Playable_`) and vehicles' (FQN
    /// contains `_VEH_`), then freeze `Health.CurrentValue` (+0x44 inside
    /// HAS) and `MaxHealth.CurrentValue` (+0x54) at `1.0`. The result:
    /// every enemy has 1 HP, any hit kills them.
    #[serde(rename = "freeze_for_matching")]
    FreezeForMatching {
        /// UClass FName to enumerate (case-insensitive). For one-hit-kill:
        /// `"HealthAttributeSet"`.
        class_path: String,
        /// Raw byte offsets from each matched UObject's base where the
        /// `value` gets written. Multiple offsets let a single tick write
        /// related fields together (e.g. both Health and MaxHealth's
        /// CurrentValue) without needing separate features.
        field_offsets: Vec<i64>,
        /// Constant to write at each `field_offsets[i]`. Currently always
        /// f32 — extend with a `value_kind` field if a future feature needs
        /// integer attributes.
        value: f32,
        /// Freeze cadence in ms. 100–250 is comfortable for one-hit-kill;
        /// faster eats IPC bandwidth (the walk returns dozens of matches
        /// per tick).
        #[serde(default = "default_freeze_for_matching_interval_ms")]
        interval_ms: u32,
        /// Cap on results from one `FindAllUObjects` call. Generous; the
        /// largest LotDK level observed ~100 HealthAttributeSet instances.
        #[serde(default = "default_freeze_for_matching_max_results")]
        max_results: u32,
        /// FQN filters. A match is kept iff its FQN contains **all**
        /// `include_fqn_contains` substrings (case-insensitive; AND) AND
        /// none of the `exclude_fqn_contains` substrings. Empty `include`
        /// = include everything.
        #[serde(default)]
        include_fqn_contains: Vec<String>,
        #[serde(default)]
        exclude_fqn_contains: Vec<String>,
    },
}

fn default_freeze_for_matching_interval_ms() -> u32 {
    250
}

fn default_freeze_for_matching_max_results() -> u32 {
    1024
}

/// Where the grant feature looks up its list of tags.
#[derive(Debug, Clone, Deserialize)]
pub struct TagSourceSpec {
    /// UClass of the data-asset to find via `find_uobject`.
    /// Either `"TtGameProgressDefinitionSet"` or `"TtGameProgressRuleSet"`.
    pub asset_class: String,
    /// Predicate to disambiguate when several assets share the class
    /// (typical: `{ kind = "contains", value = "PROG_Skills" }`).
    pub predicate: PredicateSpec,
    /// Schema discriminator + entry offsets. The two known shapes are
    /// embedded as enum variants; future shapes (e.g. a `[custom]` block
    /// with explicit stride/offset overrides) plug in here.
    pub mode: TagSourceMode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TagSourceMode {
    /// `TtGameProgressDefinitionSet::ProgressDefinitions` at `+0x50`.
    /// 16-byte entries (ClassPtr + DataPtr); tag at `DataPtr+0x4C`.
    Definitions,
    /// `TtGameProgressRuleSet::Rules` at `+0x30`. 56-byte entries
    /// (inline `TtGameProgressRule`); tag at `Values[0]+0x00`, where
    /// `Values` is the inner TArray at `rule+0x18`.
    RulesValues,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetCallSpec {
    /// UClass that owns the static UFunction (its CDO is the call target).
    /// For LotDK skill grants: `"TtGameProgressStatics"`.
    pub statics_class: String,
    /// UFunction name. For LotDK: `"SetGameProgressValue"`.
    pub function: String,
    /// UClass of any live UObject we can pass as `WorldContextObject`.
    /// The DLL finds the first live instance via the predicate below;
    /// any object with a valid `GetWorld()` works. For LotDK we use the
    /// live `TtGameProgressLiveData_2147480071` under
    /// `/Engine/Transient/`.
    pub world_context_class: String,
    /// Predicate for narrowing the world-context object to a live one
    /// (vs the CDO). Default: `{ kind = "contains", value = "/Engine/Transient/" }`.
    #[serde(default = "default_transient_predicate")]
    pub world_context_predicate: PredicateSpec,
}

fn default_transient_predicate() -> PredicateSpec {
    PredicateSpec::Contains("/Engine/Transient/".to_string())
}

/// Player-facing override for SetProgressTags status messages. Used when
/// the raw tag count is meaningless to the user (e.g. `PROG_FastTravelUnlock`
/// has 65 internal entries — story transitions, cutscene teleports — but the
/// player only sees ~9 map terminals). The status line reports
/// `min(current, total) / total {label}` instead of the raw `current / N`.
#[derive(Debug, Clone, Deserialize)]
pub struct DisplayOverrideSpec {
    /// Player-facing total. Caps the displayed numerator at this value so
    /// "65 unlocked" never appears when the player can only see 9.
    pub total: u32,
    /// Unit word inserted between the count and "unlocked". Examples:
    /// `"fast-travel terminals"`, `"skills"`. Singular or plural at the
    /// author's discretion — the message always renders with the count.
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unlock_all_skills_toml() {
        let toml = include_str!("../../games/batman-lod/signatures/unlock_all_skills.toml");
        let spec = SignatureSpec::parse(toml).expect("should parse");
        spec.validate().expect("should validate");
        match &spec.write {
            WriteSpec::SetProgressTags {
                source,
                call,
                value,
                ..
            } => {
                assert_eq!(source.asset_class, "TtGameProgressDefinitionSet");
                assert!(matches!(source.mode, TagSourceMode::Definitions));
                assert!(matches!(
                    source.predicate,
                    PredicateSpec::Contains(ref s) if s == "PROG_Skills"
                ));
                assert_eq!(call.statics_class, "TtGameProgressStatics");
                assert_eq!(call.function, "SetGameProgressValue");
                assert_eq!(call.world_context_class, "TtGameProgressLiveData");
                assert_eq!(*value, 2);
            }
            other => panic!("expected SetProgressTags, got {other:?}"),
        }
        assert_eq!(spec.effective_kind(), ValueKind::Bool);
    }

    #[test]
    fn parses_teleport_to_waypoint_toml() {
        let toml = include_str!("../../games/batman-lod/signatures/teleport_to_waypoint.toml");
        let spec = SignatureSpec::parse(toml).expect("should parse");
        spec.validate().expect("should validate");
        match &spec.write {
            WriteSpec::TeleportToWaypoint { waypoint } => {
                assert_eq!(waypoint.class_path, "CustomMapPinActor");
                assert_eq!(waypoint.property_name, "RootComponent");
                assert_eq!(waypoint.deref.len(), 1);
                assert_eq!(waypoint.deref[0].property_name, "RelativeLocation");
            }
            other => panic!("expected TeleportToWaypoint, got {other:?}"),
        }
        assert_eq!(spec.effective_kind(), ValueKind::Bool);
    }
}

fn default_interval_ms() -> u32 {
    250
}

/// Whitespace-tolerant hex-payload parser used by both `validate()` and the
/// runtime's UFunction call. Accepts pairs separated by any whitespace.
/// An empty string yields an empty `Vec` (legitimate: "don't fire on
/// toggle-off"). Returns `Err` with the offending token on the first
/// malformed pair.
pub fn parse_hex_payload(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        if tok.len() != 2 || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("bad hex byte `{tok}`"));
        }
        out.push(u8::from_str_radix(tok, 16).unwrap());
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocatorSpec {
    pub pattern: String,
    pub resolve: ResolveSpec,
    #[serde(default)]
    pub module: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolveSpec {
    Direct,
    Offset {
        delta: i64,
    },
    RipRelative {
        instruction_offset: usize,
        operand_size: usize,
    },
}

impl From<ResolveSpec> for openforge_core::Resolve {
    fn from(r: ResolveSpec) -> Self {
        match r {
            ResolveSpec::Direct => openforge_core::Resolve::Direct,
            ResolveSpec::Offset { delta } => openforge_core::Resolve::Offset { delta },
            ResolveSpec::RipRelative {
                instruction_offset,
                operand_size,
            } => openforge_core::Resolve::RipRelative {
                instruction_offset,
                operand_size,
            },
        }
    }
}

/// Heap-value-scan locator. The runtime walks every committed RW region
/// looking for a `u64` value matching `value` at `alignment`-aligned offsets.
/// Each match `M` is treated as a candidate; `M + field_offset` is the prospective
/// target address. Validators (zero, one, or many) filter candidates by reading
/// additional fields and range-checking them. If exactly one candidate passes,
/// that's the result.
///
/// Validators accept both the legacy single `[heap_scan.validator]` form and
/// the more general `[[heap_scan.validators]]` array form. The two are merged
/// internally.
#[derive(Debug, Clone, Deserialize)]
pub struct HeapScanSpec {
    /// Width of the needle value. Currently only `i64`/`u64` are supported.
    pub value_type: ValueKind,
    /// The value to search for, expressed as a signed integer for ergonomics
    /// (e.g. `9999999`). Negative values are sign-extended into 64 bits.
    pub value: i64,
    /// Offset added to each match address to land on the desired field.
    /// Can be negative.
    #[serde(default)]
    pub field_offset: i64,
    /// Alignment in bytes for candidate values (default 8 for u64).
    #[serde(default = "default_heap_alignment")]
    pub alignment: usize,
    /// Legacy single-validator form.
    #[serde(default)]
    pub validator: Option<HeapValidatorSpec>,
    /// New multi-validator form. All entries must pass.
    #[serde(default)]
    pub validators: Vec<HeapValidatorSpec>,
}

impl HeapScanSpec {
    /// Merge legacy `validator` + new `validators` into one list.
    pub fn all_validators(&self) -> Vec<&HeapValidatorSpec> {
        let mut out: Vec<&HeapValidatorSpec> = Vec::new();
        if let Some(v) = &self.validator {
            out.push(v);
        }
        out.extend(self.validators.iter());
        out
    }
}

fn default_heap_alignment() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeapValidatorSpec {
    /// What integer type to interpret the validator field as.
    pub field_type: ValueKind,
    /// Offset relative to the candidate result (`M + field_offset`).
    #[serde(default)]
    pub relative_offset: i64,
    /// Inclusive lower bound. Missing = no lower bound.
    #[serde(default)]
    pub min: Option<f64>,
    /// Inclusive upper bound. Missing = no upper bound.
    #[serde(default)]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PointerChainSpec {
    pub hops: Vec<HopSpec>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct HopSpec {
    pub deref: bool,
    pub offset: i64,
}

// ---------------------------------------------------------------------------
// [reflection] — UE5 reflection-driven discovery
// ---------------------------------------------------------------------------

/// Reflection-based locator. At attach + write time, the runtime calls
/// [`openforge_core::Ctx::find_uobject`] + [`openforge_core::Ctx::resolve_property`]
/// (which the IPC-backed `Ue5Session` proxies into the injected DLL) to find
/// the live target object and the named field's offset on its UClass. No
/// heap-scan fingerprints, no static address caching, no host-side
/// `WriteProcessMemory`.
///
/// **TOML shape**:
///
/// ```toml
/// [reflection]
/// class_path    = "DinnerCurrency_Studs"
/// property_name = "Total"
/// also_write    = ["Saved_Total"]  # optional — mirror writes to companion fields
///
/// [reflection.predicate]
/// kind  = "fqn_prefix"
/// value = "/Engine/Transient/"
///
/// # Optional dereference chain. Used by Lock Health:
/// #   1. Find DinnerPlayerState (the predicate above scopes it)
/// #   2. Resolve `PawnPrivate` (property_name) — an ObjectProperty
/// #   3. Deref the pointer; resolve `bCanBeDamaged` on the dereffed
/// #      object's UClass; freeze that.
/// [reflection.deref]
/// property_name = "bCanBeDamaged"
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ReflectionSpec {
    /// UClass FName as it appears in the GUObjectArray walk (no `U`/`A`
    /// prefix — UE5 strips those at FName interning). Case-insensitive.
    /// Examples: `"DinnerCurrency_Studs"`, `"DinnerPlayerState"`.
    pub class_path: String,
    /// Discriminator when multiple instances exist. Defaults to `Any`,
    /// which picks the first match in GUObjectArray order — that's usually
    /// the class-default object (`Default__X`) and almost never what you
    /// want. The canonical "find the live one" form is
    /// `{ kind = "fqn_prefix", value = "/Engine/Transient/" }`.
    #[serde(default)]
    pub predicate: PredicateSpec,
    /// Property name on the resolved class (or any super). Must exist on
    /// the class chain — a miss is a configuration error, not a transient
    /// state.
    pub property_name: String,
    /// Additional property names on the same class to mirror every write
    /// to. For `Set Studs`, writing `Total` alone leaves `Saved_Total`
    /// stale until the game's next save — declaring `also_write =
    /// ["Saved_Total"]` keeps both in sync immediately.
    #[serde(default)]
    pub also_write: Vec<String>,
    /// Pointer-dereference chain. Each entry is one pointer-follow step:
    /// the previous step's resolved field is read as an 8-byte object
    /// pointer, then the entry's `property_name` is resolved on the
    /// dereferenced object's UClass. The **last** entry's property is the
    /// actual value-type field that gets read/written; intermediate
    /// entries are always pointer hops.
    ///
    /// One-hop (Lock Health): `PlayerState.PawnPrivate → bCanBeDamaged` →
    /// `deref = [{ property_name = "bCanBeDamaged" }]`.
    ///
    /// Two-hop (movement mods): `PlayerState.PawnPrivate →
    /// Pawn.CharacterMovement → CMC.JumpZVelocity` →
    /// `deref = [{ property_name = "CharacterMovement" },
    ///           { property_name = "JumpZVelocity" }]`.
    ///
    /// TOML form: `[[reflection.deref]]` (array of tables). Empty Vec
    /// means no deref — read/write at the primary property's address
    /// directly.
    #[serde(default)]
    pub deref: Vec<ReflectionDerefSpec>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// Wire form of [`openforge_core::Predicate`] for the signature TOML
/// schema. Serializes as `{ kind = "...", value = "..." }`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PredicateSpec {
    #[default]
    Any,
    Exact(String),
    Contains(String),
    FqnPrefix(String),
}

impl PredicateSpec {
    /// Convert to the runtime-facing `Predicate` consumed by `Ctx::find_uobject`.
    pub fn to_core(&self) -> openforge_core::Predicate {
        match self {
            PredicateSpec::Any => openforge_core::Predicate::Any,
            PredicateSpec::Exact(s) => openforge_core::Predicate::Exact(s.clone()),
            PredicateSpec::Contains(s) => openforge_core::Predicate::Contains(s.clone()),
            PredicateSpec::FqnPrefix(s) => openforge_core::Predicate::FqnPrefix(s.clone()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReflectionDerefSpec {
    /// FProperty name to resolve on the dereferenced object's UClass.
    /// For non-final entries in the chain, this must be an ObjectProperty
    /// (pointer). For the final entry, this is the value-type field
    /// (Float, Bool, Byte, Int, etc.) being read/written.
    ///
    /// The dereffed object's class is read from its UObject header
    /// (`uobject_class_private` offset, baked into the DLL) at write time —
    /// it can change across writes if the parent's pointer is reseated
    /// (e.g. character switching in LotDK).
    pub property_name: String,
    /// Optional raw byte offset added to the resolved address after the
    /// property's offset is applied. Used to reach a primitive field
    /// embedded inside a `StructProperty` whose layout the reflection
    /// system doesn't introspect (UE5 lays `StructProperty` out as the
    /// raw struct memory, but FProperty walks only enumerate top-level
    /// fields — inner fields need a manual offset).
    ///
    /// Example: `Health` on `HealthAttributeSet` is an
    /// `FGameplayAttributeData` struct of layout
    /// `{ vptr(+0); BaseValue f32(+8); CurrentValue f32(+12) }`. To freeze
    /// `Health.CurrentValue`, set `inner_offset = 12` on the `Health` step.
    ///
    /// Only meaningful on the final entry — intermediate entries are
    /// pointer slots, not value containers; adjusting them by a raw byte
    /// offset would corrupt the pointer read.
    #[serde(default)]
    pub inner_offset: i64,
}
