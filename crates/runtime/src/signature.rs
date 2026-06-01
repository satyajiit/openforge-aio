//! Signature TOML schema. Parsed once per feature at startup; the resulting
//! [`SignatureSpec`] drives the declarative engine in `feature.rs`.

use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};
use crate::value::ValueKind;

// The UE5-reflection-specific schema types live in the engine-namespaced
// module `crate::engines::ue5`; the generic signature core re-exports them
// verbatim so every existing path (`crate::signature::ReflectionSpec`, the
// runtime lib re-exports, feature.rs imports) keeps resolving and the on-disk
// schema + golden snapshot stay byte-identical.
pub use crate::engines::ue5::{
    DisplayOverrideSpec, PredicateSpec, ReflectionDerefSpec, ReflectionSpec, SetCallSpec,
    TagSourceMode, TagSourceSpec,
};

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
        Self::parse_with(toml_text, crate::format::ConfigFormat::Toml)
    }

    /// Parse from `src` in the given [`crate::format::ConfigFormat`]. The
    /// format-agnostic entry point; `parse` is the TOML-pinned convenience
    /// wrapper. Behavior for TOML is byte-for-byte identical to the previous
    /// `toml::from_str` path (same `RuntimeError::SignatureParse` error).
    pub fn parse_with(src: &str, fmt: crate::format::ConfigFormat) -> RuntimeResult<Self> {
        crate::format::parse_str(src, fmt).map_err(RuntimeError::SignatureParse)
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
            | WriteSpec::FreezeForMatching { .. }
            | WriteSpec::PlayAnimMontageForMatching { .. } => ValueKind::Bool,
            _ => self
                .value
                .as_ref()
                .map(|v| v.kind)
                .unwrap_or(ValueKind::I32),
        }
    }

    /// The engine this signature requires, if it uses engine-specific capabilities.
    /// `None` means engine-agnostic (plain memory write/freeze/heap_scan/code_patch),
    /// usable on any engine. UE5-reflection features return Some(Ue5).
    pub fn required_engine(&self) -> Option<crate::manifest::EngineKind> {
        use crate::manifest::EngineKind;
        if self.reflection.is_some() {
            return Some(EngineKind::Ue5);
        }
        match self.write {
            WriteSpec::SetProgressTags { .. }
            | WriteSpec::CallInstanceUFunction { .. }
            | WriteSpec::TeleportToWaypoint { .. }
            | WriteSpec::FreezeForMatching { .. }
            | WriteSpec::PlayAnimMontageForMatching { .. } => Some(EngineKind::Ue5),
            _ => None,
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
                class_name_substrings,
                field_offsets,
                value,
                value_bytes,
                ..
            } = &self.write
            {
                if field_offsets.is_empty() {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "freeze_for_matching requires at least one `field_offsets` entry"
                            .into(),
                    });
                }
                let has_class_path = !class_path.trim().is_empty();
                let has_substrings = !class_name_substrings.is_empty();
                if has_class_path == has_substrings {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason:
                            "freeze_for_matching requires exactly one of `class_path` (exact match) \
                             or `class_name_substrings` (substring match)"
                                .into(),
                    });
                }
                let has_value = value.is_some();
                let has_value_bytes = value_bytes.is_some();
                if has_value == has_value_bytes {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "freeze_for_matching requires exactly one of `value` (f32) or \
                             `value_bytes` (hex string)"
                            .into(),
                    });
                }
                if let Some(hex) = value_bytes
                    && parse_hex_payload(hex).is_err()
                {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "freeze_for_matching: `value_bytes` is not valid whitespace-tolerant hex"
                            .into(),
                    });
                }
            }
            return Ok(());
        }
        // PlayAnimMontageForMatching also self-discovers via class filters +
        // resolves AnimMontage assets by name at runtime. No
        // [locator]/[heap_scan]/[reflection] expected.
        let is_play_anim_montage =
            matches!(self.write, WriteSpec::PlayAnimMontageForMatching { .. });
        if is_play_anim_montage {
            if self.locator.is_some() || self.heap_scan.is_some() || self.reflection.is_some() {
                return Err(RuntimeError::SignatureInvalid {
                    feature: self.meta.feature.clone(),
                    reason: "play_anim_montage_for_matching carries its own discovery via \
                             `class_path` + `montage_names`; remove [locator] / [heap_scan] / [reflection]"
                        .into(),
                });
            }
            if let WriteSpec::PlayAnimMontageForMatching {
                class_path,
                class_name_substrings,
                montage_names,
                max_distance_u,
                ..
            } = &self.write
            {
                let has_class_path = !class_path.trim().is_empty();
                let has_substrings = !class_name_substrings.is_empty();
                if has_class_path == has_substrings {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "play_anim_montage_for_matching requires exactly one of \
                                 `class_path` or `class_name_substrings`"
                            .into(),
                    });
                }
                if montage_names.is_empty() {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "play_anim_montage_for_matching requires at least one entry in `montage_names`".into(),
                    });
                }
                if !(*max_distance_u > 0.0 && max_distance_u.is_finite()) {
                    return Err(RuntimeError::SignatureInvalid {
                        feature: self.meta.feature.clone(),
                        reason: "play_anim_montage_for_matching: `max_distance_u` must be finite and > 0".into(),
                    });
                }
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
    /// One-shot action button. Single click fires `write(Bool(true))`;
    /// there's no "off" state because the feature can't be reversed (the
    /// player has it now). Used by `SetProgressTags` features
    /// (Unlock All Skills / Outfits / Fast Travel) where the previous
    /// `Switch` rendering implied a reversible toggle that the strategy
    /// can't actually deliver.
    Button {
        /// Optional label override for the button text. When `None`, the
        /// frontend defaults to `"Apply"`.
        #[serde(default)]
        label: Option<String>,
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
        /// Glacier DLL-freeze only: instead of writing a constant, copy the f32
        /// sibling field at `resolved_addr + this` into the freeze target each
        /// tick (e.g. current := max for difficulty-agnostic god mode). When
        /// set, the app's Glacier `set_freeze` drives the DLL `StartFreeze` op
        /// with this `source_offset`; the generic constant-write freeze loop
        /// (and every UE5 freeze) ignores it.
        #[serde(default)]
        freeze_copy_offset: Option<i64>,
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
        /// UClass FName to enumerate (case-insensitive, exact match). For
        /// one-hit-kill: `"HealthAttributeSet"`. Leave empty when using
        /// `class_name_substrings` instead.
        #[serde(default)]
        class_path: String,
        /// Class-name substring matcher. Use when the target population
        /// spans many BP subclasses with a shared naming convention (e.g.
        /// `["_Goon_C", "_SWAT_", "Arkham", "TwoFace"]` catches every goon
        /// class). Exactly one of `class_path` or `class_name_substrings`
        /// must be set. Routes through host-side `walk_objects` + filter,
        /// independent of the DLL's exact-match `find_all_uobjects`.
        #[serde(default)]
        class_name_substrings: Vec<String>,
        /// Raw byte offsets from each matched UObject's base where the
        /// write happens. Multiple offsets let a single tick write related
        /// fields together (Health + MaxHealth CurrentValue) without
        /// needing separate features. With `value_bytes`, the byte
        /// sequence is written at each offset (single offset is the norm).
        field_offsets: Vec<i64>,
        /// Constant f32 to write at each `field_offsets[i]`. Mutually
        /// exclusive with `value_bytes`. Use this for numeric attributes
        /// (health, time-dilation, etc.).
        #[serde(default)]
        value: Option<f32>,
        /// Whitespace-tolerant hex string. Decoded to a byte sequence that
        /// gets written starting at each `field_offsets[i]`. Use for non-
        /// numeric writes — FNames, FGameplayTags, struct-pair stamps.
        /// Length is unconstrained (must match the field's actual size).
        /// Mutually exclusive with `value`.
        #[serde(default)]
        value_bytes: Option<String>,
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
    /// Bulk-call a UFunction (PlayAnimMontage) on every live UObject that
    /// matches the class filter, cycling through a list of UAnimMontage
    /// assets. Drives `mod_npc_dance`: every NPC near the player plays a
    /// random dance/taunt animation while the toggle is on.
    ///
    /// Mechanics:
    /// 1. On first tick, resolve every `montage_names` entry via
    ///    `find_uobject(class_path="AnimMontage", predicate=exact)` and
    ///    cache the resolved addresses on the feature.
    /// 2. Find the player's pawn via `BP_DinnerPlayerState_C.PawnPrivate`
    ///    and read its world location.
    /// 3. Walk every UObject matching `class_path` / `class_name_substrings`,
    ///    apply FQN filters, and compute distance to the player's pawn.
    ///    NPCs farther than `max_distance_u` are skipped (those LOD-promoted
    ///    Actors don't have a live AnimInstance and PlayAnimMontage on them
    ///    can crash the game).
    /// 4. For each survivor, build a 24-byte PlayAnimMontage param blob with
    ///    the next montage in the rotation (per-NPC index + tick counter so
    ///    neighbours don't sync) and `call_ufunction(npc, npc.class,
    ///    "PlayAnimMontage", ..)`.
    ///
    /// Toggle OFF stops the freeze loop. No restore is needed — montages
    /// naturally play out and the AnimInstance returns to its blendspace.
    #[serde(rename = "play_anim_montage_for_matching")]
    PlayAnimMontageForMatching {
        /// UClass FName for exact-match enumeration (rare). Use
        /// `class_name_substrings` instead for BP populations.
        #[serde(default)]
        class_path: String,
        /// Class-name substrings to match (case-insensitive). Same semantics
        /// as `FreezeForMatching::class_name_substrings`.
        #[serde(default)]
        class_name_substrings: Vec<String>,
        /// UObject names to look up under `AnimMontage`. Resolved at first
        /// tick via `find_uobject(class_path="AnimMontage", predicate=exact)`
        /// and cached for the session. The cache is per-DeclarativeFeature
        /// instance; addresses are valid for the game session.
        montage_names: Vec<String>,
        /// Tick cadence in ms. Should be roughly the average montage length
        /// (most LEGO dance/taunt montages are 2-4s) so a fresh montage
        /// starts as the previous one fades.
        #[serde(default = "default_play_anim_montage_interval_ms")]
        interval_ms: u32,
        /// Cap on `FindAllUObjects` results per tick.
        #[serde(default = "default_freeze_for_matching_max_results")]
        max_results: u32,
        /// Max distance from the player's pawn (UE units, 1u ≈ 1cm) for an
        /// NPC to be eligible. NPCs beyond this are LOD'd out — they're
        /// Mass entities without a real AnimInstance, and PlayAnimMontage on
        /// them can crash the game.
        #[serde(default = "default_play_anim_montage_max_distance_u")]
        max_distance_u: f64,
        /// FQN filters, same semantics as `FreezeForMatching`.
        #[serde(default)]
        include_fqn_contains: Vec<String>,
        #[serde(default)]
        exclude_fqn_contains: Vec<String>,
        /// PlayRate forwarded into PlayAnimMontage. 1.0 = native speed,
        /// 1.5 = sped up, etc.
        #[serde(default = "default_play_anim_montage_play_rate")]
        play_rate: f32,
        /// Where the player's pawn lives. PlayAnimMontage targets are
        /// distance-filtered against this pawn's K2_GetActorLocation.
        #[serde(default = "default_play_anim_montage_player_state_class")]
        player_state_class: String,
        /// Property on the player-state class that points at the live pawn.
        /// For LotDK: `PawnPrivate` (inherited from APlayerState).
        #[serde(default = "default_play_anim_montage_player_pawn_property")]
        player_pawn_property: String,
    },
}

fn default_freeze_for_matching_interval_ms() -> u32 {
    250
}

fn default_play_anim_montage_interval_ms() -> u32 {
    2500
}

fn default_play_anim_montage_max_distance_u() -> f64 {
    1500.0
}

fn default_play_anim_montage_play_rate() -> f32 {
    1.0
}

fn default_play_anim_montage_player_state_class() -> String {
    "BP_DinnerPlayerState_C".to_string()
}

fn default_play_anim_montage_player_pawn_property() -> String {
    "PawnPrivate".to_string()
}

fn default_freeze_for_matching_max_results() -> u32 {
    1024
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

    #[test]
    fn required_engine_reflection_is_ue5() {
        use crate::manifest::EngineKind;
        let toml = r#"
            [meta]
            feature = "lock_health"
            display_name = "Lock Health"

            [value]
            type = "bool"

            [write]
            strategy = "freeze"

            [reflection]
            class_path = "DinnerPlayerState"
            property_name = "PawnPrivate"

            [reflection.predicate]
            kind = "fqn_prefix"
            value = "/Engine/Transient/"

            [[reflection.deref]]
            property_name = "bCanBeDamaged"
        "#;
        let spec = SignatureSpec::parse(toml).expect("should parse");
        spec.validate().expect("should validate");
        assert_eq!(spec.required_engine(), Some(EngineKind::Ue5));
    }

    #[test]
    fn required_engine_heap_scan_one_shot_is_none() {
        let toml = r#"
            [meta]
            feature = "set_currency"
            display_name = "Set Currency"

            [value]
            type = "i32"

            [write]
            strategy = "one_shot"

            [heap_scan]
            value_type = "i64"
            value = 9999999
        "#;
        let spec = SignatureSpec::parse(toml).expect("should parse");
        spec.validate().expect("should validate");
        assert_eq!(spec.required_engine(), None);
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
