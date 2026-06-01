//! UE5 engine-specific signature schema + struct-layout constants.
//!
//! The generic signature core ([`crate::signature`]) is engine-neutral: it
//! defines the `WriteSpec` enum (whose strategy strings stay stable on disk)
//! and the engine-agnostic locator blocks. The UE5-reflection-specific *types*
//! those engine variants reference — `ReflectionSpec`, `PredicateSpec`,
//! `TagSource*`, `SetCallSpec`, `DisplayOverrideSpec` — are defined here so the
//! core stops owning UE5 specifics. They are re-exported from `signature.rs`
//! verbatim, so the on-disk schema (and the golden snapshot) is byte-identical.
//!
//! The `SPT_*` constants are the `set_progress_tags` struct-layout magic
//! numbers — properties of UE5 + this game's TT class layouts, not
//! user-configurable, so they live in code rather than in TOML.

use serde::Deserialize;

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

/// Where the grant feature looks up its list of tags.
#[derive(Debug, Clone, Deserialize)]
pub struct TagSourceSpec {
    /// UClass of the data-asset (for `Definitions` / `RulesValues` modes)
    /// OR the UClass of the per-instance meta-data objects to enumerate
    /// (for `AllUObjectsOfClass` mode).
    pub asset_class: String,
    /// Predicate to disambiguate when several assets share the class
    /// (typical: `{ kind = "contains", value = "PROG_Skills" }`). Ignored
    /// for the `AllUObjectsOfClass` mode — every loaded instance is
    /// considered (filtered by `exclude_name_substrings` instead).
    #[serde(default)]
    pub predicate: PredicateSpec,
    /// Schema discriminator + entry offsets. New shapes plug in as enum
    /// variants.
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
    /// Walk GUObjectArray for every live UObject of `asset_class`. Each
    /// instance carries an `FGameplayTag`/`FName` (8 bytes) in a property
    /// named `tag_property_name`; that's the per-entry tag. Instances
    /// whose short object-name contains any of `exclude_name_substrings`
    /// are filtered out — used for `DinnerCharacterMetaData` to skip
    /// `_Goon`, `_Quest`, `_Civilian`, etc. variants of player outfits.
    ///
    /// Drives `mod_unlock_all_outfits`: walks every loaded
    /// `DinnerCharacterMetaData`, filters to player-relevant outfits,
    /// reads each one's `ProgressTag`, and calls SetGameProgressValue.
    #[serde(rename = "all_uobjects_of_class")]
    AllUObjectsOfClass {
        /// FProperty name to resolve on the asset's UClass chain. The
        /// resolved offset reads 8 bytes (FName layout: u32 index + u32
        /// number) as the tag.
        tag_property_name: String,
        /// Case-insensitive substring filters against each instance's
        /// short object name (the part after the last `/` or `.` in the
        /// FQN). Any match excludes the instance.
        #[serde(default)]
        exclude_name_substrings: Vec<String>,
    },
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

// ---------------------------------------------------------------------------
// set_progress_tags layout/protocol constants
// ---------------------------------------------------------------------------
//
// Properties of UE5 + this game's TT struct layouts, not user-configurable, so
// they live in code rather than in the TOML.

pub const SPT_RULES_ARRAY_OFFSET: u64 = 0x30;
pub const SPT_DEFS_ARRAY_OFFSET: u64 = 0x50;
pub const SPT_RULE_STRIDE: usize = 56;
pub const SPT_RULE_VALUES_OFFSET: usize = 0x18;
pub const SPT_DEF_ENTRY_STRIDE: usize = 16;
pub const SPT_DEF_ENTRY_DATA_PTR_OFFSET: usize = 8;
pub const SPT_DEF_INSTANCE_TAG_OFFSET: usize = 0x4C;
pub const SPT_FNAME_SIZE: usize = 8;
pub const SPT_SET_PARMS_SIZE: usize = 0x31; // 49 bytes — SetGameProgressValue
pub const SPT_GET_PARMS_SIZE: usize = 0x11; // 17 bytes — GetGameProgressValue
pub const SPT_PARM_WORLDCTX: usize = 0x00;
pub const SPT_PARM_TAG: usize = 0x08;
pub const SPT_PARM_VALUE: usize = 0x10;
