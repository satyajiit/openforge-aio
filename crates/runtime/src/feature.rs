//! `Feature` trait + `DeclarativeFeature` engine + `Tier`.

use openforge_core::{Ctx, Pattern, Result as CoreResult, ctx::read_pointer};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};
use crate::signature::{
    ControlSpec, HeapScanSpec, HeapValidatorSpec, ReflectionSpec, SignatureSpec, WriteSpec,
};
use crate::value::{Value, ValueKind, ValueRange};

/// Offset from a UObject base to its `UClass*`. Stable UE4/UE5 invariant
/// (`UObjectBase::ClassPrivate`); cross-checked against the LotDK UE4SS
/// log at `references/ue4ss/UE4SS.log:86`. If a future game's UE build
/// shifts this, promote it to a method on `Ctx`.
const UOBJECT_CLASS_PRIVATE_OFFSET: usize = 0x10;

pub type Tier = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteStrategyKind {
    OneShot,
    Freeze,
    CodePatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureMeta {
    pub id: String,
    pub display_name: String,
    /// Player-facing one-liner. Distinct from `description`, which is the
    /// long technical write-up shown only behind an info icon.
    pub tagline: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    /// Optional CSS color for the icon stroke. Passed through verbatim to
    /// the frontend; `None` falls back to a tier-derived hue.
    pub icon_color: Option<String>,
    pub tier: Tier,
    /// Top-level UI category — `"trainer"` (cheats) or `"mod"` (extras).
    /// Drives the Trainer/Mods top tab split in the game details pane.
    pub category: String,
    pub kind: ValueKind,
    pub range: ValueRange,
    pub strategy: WriteStrategyKind,
    pub control: Option<ControlSpec>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PreflightCheck {
    pub id: &'static str,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightReport {
    pub checks: Vec<PreflightCheck>,
}

impl PreflightReport {
    pub fn ok() -> Self {
        Self { checks: Vec::new() }
    }

    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

/// Every cheat in OpenForge implements this trait. Most are
/// [`DeclarativeFeature`] (driven entirely by a signature TOML). The trait is
/// kept open so custom Rust impls can handle the rare cases declarative can't
/// (bitfields, atomic multi-writes, gameplay-state-aware behavior).
pub trait Feature: Send + Sync + 'static {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn tagline(&self) -> Option<&str> {
        None
    }
    fn description(&self) -> Option<&str> {
        None
    }
    fn icon(&self) -> Option<&str> {
        None
    }
    /// CSS color for the icon stroke. `None` defers to the frontend's
    /// tier-derived fallback. Set this from a signature TOML
    /// (`[meta] icon_color = "..."`) when the tier's default hue doesn't
    /// fit the feature.
    fn icon_color(&self) -> Option<&str> {
        None
    }
    fn control(&self) -> Option<&ControlSpec> {
        None
    }
    fn tier(&self) -> &str {
        "general"
    }
    fn kind(&self) -> ValueKind;
    fn range(&self) -> ValueRange {
        ValueRange::default()
    }
    fn write_strategy(&self) -> WriteStrategyKind;
    /// Default freeze cadence in milliseconds. Declarative features override
    /// this from their `[write].interval_ms` field.
    fn freeze_interval_ms(&self) -> u32 {
        250
    }

    /// Optional fixed freeze target. When `Some`, the trainer freezes to
    /// this value instead of capturing the field's current value at toggle
    /// time. Returned as an `f64` so the caller (which knows the feature's
    /// declared `kind()`) can coerce to the right `Value` variant.
    ///
    /// Used by `Infinite Health` (always freeze `bCanBeDamaged` at `false`)
    /// and by future "max-out" lock features where capturing the current
    /// value would defeat the lock's purpose.
    fn freeze_value(&self) -> Option<f64> {
        None
    }

    fn resolve(&self, ctx: &dyn Ctx) -> CoreResult<usize>;
    fn read(&self, ctx: &dyn Ctx, addr: usize) -> RuntimeResult<Value>;
    fn write(&self, ctx: &dyn Ctx, addr: usize, v: Value) -> RuntimeResult<()>;

    /// Snapshot the current bytes at every address this feature would write
    /// to, so the trainer can restore them later (e.g. when a freeze toggle
    /// is switched off and the user expects "back to default").
    ///
    /// Default reads `kind().size()` bytes at `addr`. Features that write to
    /// more than one address per tick (reflection with `also_write`) override
    /// this to concatenate primary + companion bytes.
    fn snapshot(&self, ctx: &dyn Ctx, addr: usize) -> RuntimeResult<Vec<u8>> {
        let size = self.kind().size();
        let mut buf = vec![0u8; size];
        ctx.read_bytes(addr, &mut buf).map_err(RuntimeError::from)?;
        Ok(buf)
    }

    /// Write a previously-captured `snapshot` back to the same address(es).
    /// Pairs with [`snapshot`]; the byte layout is whatever `snapshot`
    /// returned for the same feature. Default writes the buffer back at
    /// `addr`. Features with companion fields override to dispatch slices to
    /// each address in the same order `snapshot` concatenated them.
    fn restore(&self, ctx: &dyn Ctx, addr: usize, snapshot: &[u8]) -> RuntimeResult<()> {
        ctx.write_bytes(addr, snapshot)
            .map_err(RuntimeError::from)?;
        Ok(())
    }

    /// Whether the trainer's background read-probe loop should poll this
    /// feature's value every few seconds. Defaults to `true` — appropriate
    /// for primitive reads (memory loads).
    ///
    /// Override and return `false` for features whose `read()` does heavy
    /// work on the game thread (e.g. SetProgressTags reads dozens of
    /// `GetGameProgressValue` UFunction calls per invocation, which would
    /// stall the game frame if polled). Such features still resolve at
    /// attach time and refresh `status_text` on-demand after writes.
    fn supports_probe(&self) -> bool {
        true
    }

    /// Cheap re-check of a previously-resolved address. Returns `true` if the
    /// address still looks correct — used by the trainer's address cache to
    /// skip a full re-scan on re-attach.
    ///
    /// The default is conservative: assume the cache is stale unless the
    /// feature opts in. `DeclarativeFeature` overrides this to re-run heap
    /// validators (cheap) or to trust AOB hits (always good for the process
    /// lifetime).
    fn quick_check(&self, _ctx: &dyn Ctx, _addr: usize) -> bool {
        false
    }

    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            tagline: self.tagline().map(str::to_string),
            description: self.description().map(str::to_string),
            icon: self.icon().map(str::to_string),
            icon_color: self.icon_color().map(str::to_string),
            tier: self.tier().to_string(),
            category: self.category().to_string(),
            kind: self.kind(),
            range: self.range(),
            strategy: self.write_strategy(),
            control: self.control().cloned(),
        }
    }

    /// Top-level UI category: `"trainer"` or `"mod"`. Drives the
    /// Trainer/Mods top tab split. Defaults to `"trainer"` for backward
    /// compatibility with legacy signatures.
    fn category(&self) -> &str {
        "trainer"
    }

    /// Optional human-readable status line rendered under the FeatureCard
    /// tagline. Use for features whose state is richer than the single
    /// `Value` returned by `read()` — e.g. "20 / 30 skills unlocked" for
    /// SetProgressTags, or "Inventory: 7 items locked" for future bulk
    /// features. Default: no extra status (UI hides the row).
    ///
    /// May perform IPC; called on-demand (attach + after writes) rather
    /// than on a poll loop. Returning `Err` is fine — the UI just hides
    /// the row in that case.
    fn status_text(&self, _ctx: &dyn Ctx, _addr: usize) -> RuntimeResult<Option<String>> {
        Ok(None)
    }
}

/// Source for a build-time-embedded declarative feature. A game crate's
/// `build.rs` emits a `&[DeclFeatureSrc]` slice containing one entry per
/// `signatures/*.toml`.
#[derive(Debug, Clone, Copy)]
pub struct DeclFeatureSrc {
    pub name: &'static str,
    pub toml: &'static str,
}

/// A feature whose behavior is entirely described by a parsed [`SignatureSpec`].
///
/// `pattern` is `Some` for AOB-based signatures and `None` for heap-scan
/// signatures (where there's nothing to scan in `.text`).
pub struct DeclarativeFeature {
    pub spec: SignatureSpec,
    pub pattern: Option<Pattern>,
    /// Reflection cache. Populated lazily on the first `resolve()`
    /// for features whose signature carries a `[reflection]` block. The
    /// trainer drops the `DeclarativeFeature` on detach, so cache lifetime
    /// is bounded by the session — exactly when the resolved addresses
    /// remain valid.
    reflection_cache: Mutex<Option<ReflectionCache>>,
}

/// Resolved addresses for one reflection-based feature. Held inside the
/// `DeclarativeFeature` so `write()` can mirror the primary value to any
/// companion (`also_write`) fields without re-querying the DLL each tick.
#[derive(Debug, Clone)]
struct ReflectionCache {
    /// Live UObject we resolved against. Used to detect cache staleness on
    /// subsequent read/write — when the world reloads (level transition,
    /// main-menu round-trip), the old object is freed and a new one takes
    /// its place at a different address. We probe `obj_addr + class_private`
    /// before each access; a mismatch with `class_addr` triggers a re-walk.
    obj_addr: u64,
    /// The cached UObject's UClass pointer. The validity check above
    /// compares this to whatever currently lives at `obj_addr + 0x10`.
    class_addr: u64,
    /// Property offset within the UObject. Stable across game launches
    /// (it's baked into the UClass layout), so cache survives re-resolves
    /// of the same class. Currently captured for forensics + future use
    /// (e.g. recomputing `primary` after a cheap re-find without paying
    /// the property-lookup cost again).
    #[allow(dead_code)]
    primary_offset: u32,
    /// Object addr + primary property offset. Doubles as the root of the
    /// deref chain when `derefs` is non-empty: each write starts here, reads
    /// 8 bytes as the first pointer, then follows `derefs` step by step.
    /// When `derefs` is empty, `resolve()` returns this directly and
    /// `write()` receives it via the trainer's address cache rather than
    /// reading it back from here.
    #[allow(dead_code)]
    primary: usize,
    /// Addresses for each `also_write` property on the same class. The
    /// same coerced value is mirrored here on every `write()`.
    companions: Vec<usize>,
    /// Pointer-deref chain configured via `[[reflection.deref]]` entries.
    /// Each entry walks one pointer hop; the final entry's property is the
    /// value-type field that gets read/written. Empty = no deref (write at
    /// `primary` directly).
    ///
    /// One-hop: Lock Health — PlayerState.PawnPrivate → bCanBeDamaged
    /// (single entry: `bCanBeDamaged`).
    ///
    /// Two-hop: movement mods — PlayerState.PawnPrivate →
    /// Pawn.CharacterMovement → CMC.JumpZVelocity
    /// (entries: `CharacterMovement`, `JumpZVelocity`).
    derefs: Vec<DerefStep>,
}

#[derive(Debug, Clone)]
struct DerefStep {
    /// Property name to resolve on whatever class the *previous* pointer
    /// turns out to be. Storing the name (not a resolved offset) is by
    /// design: the same parent property can point to different concrete
    /// subclasses across game sessions or character switches; the per-class
    /// offset cache in `Ue5Session::resolve_property` makes the second
    /// lookup free.
    property_name: String,
    /// Raw byte offset added after the property's resolved offset. Only
    /// meaningful on the final hop — used to reach a primitive inside a
    /// `StructProperty` (e.g. `Health.CurrentValue` at +12 inside
    /// `FGameplayAttributeData`).
    inner_offset: i64,
}

impl DeclarativeFeature {
    pub fn from_toml(toml_text: &'static str) -> RuntimeResult<Self> {
        let spec = SignatureSpec::parse(toml_text)?;
        spec.validate()?;
        let pattern = match &spec.locator {
            Some(loc) => Some(Pattern::parse(&loc.pattern).map_err(RuntimeError::from)?),
            None => None,
        };
        Ok(Self {
            spec,
            pattern,
            reflection_cache: Mutex::new(None),
        })
    }

    fn resolve_via_reflection(
        &self,
        ctx: &dyn Ctx,
        ref_spec: &ReflectionSpec,
    ) -> RuntimeResult<usize> {
        let pred = ref_spec.predicate.to_core();
        let (obj_addr, class_addr) = ctx
            .find_uobject(&ref_spec.class_path, &pred)
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "reflection: no live UObject of class `{}` (predicate {:?}). \
                     If the game is on the main menu, start a level and retry.",
                    ref_spec.class_path, ref_spec.predicate
                ),
            })?;
        let primary_resolved = ctx
            .resolve_property(class_addr, &ref_spec.property_name)
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "reflection: property `{}` not found on class `{}` chain — \
                     check the signature TOML against a `discover` dump",
                    ref_spec.property_name, ref_spec.class_path
                ),
            })?;
        let mut primary_addr = (obj_addr as usize).saturating_add(primary_resolved.offset as usize);
        if ref_spec.deref.is_empty()
            && let Some(offset) = ref_spec.offset
        {
            primary_addr = (primary_addr as i64).wrapping_add(offset) as usize;
        }

        let mut companions: Vec<usize> = Vec::with_capacity(ref_spec.also_write.len());
        for name in &ref_spec.also_write {
            let r = ctx
                .resolve_property(class_addr, name)
                .map_err(RuntimeError::from)?
                .ok_or_else(|| RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: format!(
                        "reflection: also_write property `{name}` not found on class `{}` chain",
                        ref_spec.class_path
                    ),
                })?;
            companions.push((obj_addr as usize).saturating_add(r.offset as usize));
        }

        let derefs: Vec<DerefStep> = ref_spec
            .deref
            .iter()
            .map(|d| DerefStep {
                property_name: d.property_name.clone(),
                inner_offset: d.inner_offset,
            })
            .collect();

        tracing::info!(
            feature = self.id(),
            class = %ref_spec.class_path,
            obj = format!("0x{:X}", obj_addr),
            primary = format!("0x{:X}", primary_addr),
            companions = companions.len(),
            deref_hops = derefs.len(),
            "reflection resolved"
        );

        *self.reflection_cache.lock() = Some(ReflectionCache {
            obj_addr,
            class_addr,
            primary_offset: primary_resolved.offset,
            primary: primary_addr,
            companions,
            derefs,
        });
        Ok(primary_addr)
    }

    /// Validate the reflection cache and re-resolve from scratch if the
    /// cached UObject is no longer live. Returns the current primary
    /// address — same as the cache's `primary` if validation passed, or a
    /// new one if we had to re-walk after a world transition.
    ///
    /// Validation is a single 8-byte read at `obj_addr + 0x10`
    /// (UObject::ClassPrivate). If it still matches the cached `class_addr`,
    /// the object is alive at the same address and we trust the cache. Any
    /// mismatch — or read failure (unmapped page) — triggers a full
    /// `resolve_via_reflection` call, which updates the cache and returns
    /// the new primary.
    ///
    /// This is the heart of the self-heal: level transitions free the
    /// PlayerState (and its possessed Pawn / CharacterMovement), so the
    /// next freeze-tick or write would otherwise hit a freed object. With
    /// this guard, the trainer transparently re-finds the new instances.
    fn ensure_reflection_fresh(&self, ctx: &dyn Ctx) -> RuntimeResult<usize> {
        let ref_spec =
            self.spec
                .reflection
                .as_ref()
                .ok_or_else(|| RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: "ensure_reflection_fresh called on non-reflection feature".into(),
                })?;

        let cached = {
            let guard = self.reflection_cache.lock();
            guard
                .as_ref()
                .map(|c| (c.obj_addr, c.class_addr, c.primary))
        };

        if let Some((obj_addr, expected_class, primary)) = cached {
            // Read the class slot. If the read or comparison fails, fall
            // through to re-resolve — never propagate the error here.
            let class_slot = (obj_addr as usize).saturating_add(UOBJECT_CLASS_PRIVATE_OFFSET);
            if let Ok(live_class) = read_pointer(ctx, class_slot)
                && live_class as u64 == expected_class
            {
                return Ok(primary);
            }
            tracing::info!(
                feature = self.id(),
                obj = format!("0x{obj_addr:X}"),
                "reflection cache invalidated (object reallocated) — re-resolving"
            );
        }
        self.resolve_via_reflection(ctx, ref_spec)
    }

    /// Walk the deref chain from `root_ptr_addr` and return the address of
    /// the final value-type field. Called on every `read()`/`write()` for
    /// features with `[[reflection.deref]]` entries.
    ///
    /// The chain is re-walked on every call (no cached intermediate
    /// addresses) because every pointer in it can be reseated — pawn
    /// pointers shift on character switches, component pointers reseat
    /// when components are re-registered, etc. The dereffed objects' class
    /// lookups in `ctx.resolve_property` are cached per UClass inside
    /// `Ue5Session`, so the only real cost per call is N pointer reads +
    /// 1 class-property lookup per hop (HashMap hit after the first walk).
    ///
    /// `derefs` must be non-empty; callers should bypass this and use
    /// `primary` directly when the chain is empty.
    fn resolve_deref_addr(
        &self,
        ctx: &dyn Ctx,
        root_ptr_addr: usize,
        derefs: &[DerefStep],
    ) -> RuntimeResult<usize> {
        debug_assert!(
            !derefs.is_empty(),
            "resolve_deref_addr called with empty chain — caller should write at primary directly"
        );

        // Start by reading the live pointer at the root slot.
        let mut current_ptr_addr = root_ptr_addr;
        for (hop_idx, step) in derefs.iter().enumerate() {
            let target_obj = read_pointer(ctx, current_ptr_addr).map_err(RuntimeError::from)?;
            if target_obj == 0 {
                return Err(RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: format!(
                        "reflection.deref: null pointer at hop {hop_idx} \
                         (slot 0x{current_ptr_addr:X}) before resolving `{}` — \
                         intermediate sub-object isn't live (typically: no \
                         pawn possessed yet, or component not registered)",
                        step.property_name
                    ),
                });
            }
            // Resolve the named property against the dereffed object's
            // actual UClass (read from its UObject header). This handles
            // concrete subclasses without us hard-coding them.
            let class_addr =
                read_pointer(ctx, target_obj.saturating_add(UOBJECT_CLASS_PRIVATE_OFFSET))
                    .map_err(RuntimeError::from)? as u64;
            let resolved = ctx
                .resolve_property(class_addr, &step.property_name)
                .map_err(RuntimeError::from)?
                .ok_or_else(|| RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: format!(
                        "reflection.deref: property `{}` not found on dereferenced \
                         object's class at hop {hop_idx} (class_addr=0x{:X})",
                        step.property_name, class_addr
                    ),
                })?;
            let next_addr = target_obj.saturating_add(resolved.offset as usize);
            // Apply per-step `inner_offset` (defaults to 0). The realistic
            // use is on the FINAL hop, where the resolved property is a
            // StructProperty and the caller needs a primitive field inside
            // it (e.g. `Health.CurrentValue` at +12 inside
            // `FGameplayAttributeData`). On intermediate hops the slot is a
            // pointer about to be dereffed by the next iteration; shifting
            // it would point at unrelated memory. We don't forbid that
            // combination here — the cost of being wrong is "next read
            // returns an error," the same surface as a typo in a name.
            current_ptr_addr = (next_addr as i64).wrapping_add(step.inner_offset) as usize;
        }
        // After the final hop, `current_ptr_addr` points at the value-type
        // field (since the last step's resolved property is the value, not
        // another pointer to dereference). For
        // `WriteSpec::CallInstanceUFunction` the caller treats this address
        // as a pointer SLOT and dereferences it one more time to get the
        // call target.
        if let Some(offset) = self.spec.reflection.as_ref().and_then(|r| r.offset) {
            current_ptr_addr = (current_ptr_addr as i64).wrapping_add(offset) as usize;
        }
        Ok(current_ptr_addr)
    }

    // Layout/protocol constants for `set_progress_tags`. Properties of UE5
    // + this game's struct layouts, not user-configurable, so they live in
    // code rather than the TOML.
    const SPT_RULES_ARRAY_OFFSET: u64 = 0x30;
    const SPT_DEFS_ARRAY_OFFSET: u64 = 0x50;
    const SPT_RULE_STRIDE: usize = 56;
    const SPT_RULE_VALUES_OFFSET: usize = 0x18;
    const SPT_DEF_ENTRY_STRIDE: usize = 16;
    const SPT_DEF_ENTRY_DATA_PTR_OFFSET: usize = 8;
    const SPT_DEF_INSTANCE_TAG_OFFSET: usize = 0x4C;
    const SPT_FNAME_SIZE: usize = 8;
    const SPT_SET_PARMS_SIZE: usize = 0x31; // 49 bytes — SetGameProgressValue
    const SPT_GET_PARMS_SIZE: usize = 0x11; // 17 bytes — GetGameProgressValue
    const SPT_PARM_WORLDCTX: usize = 0x00;
    const SPT_PARM_TAG: usize = 0x08;
    const SPT_PARM_VALUE: usize = 0x10;

    /// Find the data-asset, read its outer TArray, extract per-entry
    /// `FGameplayTag` bytes. Shared by `write` (grant) and `read` (count).
    fn extract_progress_tags(
        &self,
        ctx: &dyn Ctx,
        source: &crate::signature::TagSourceSpec,
    ) -> RuntimeResult<Vec<[u8; Self::SPT_FNAME_SIZE]>> {
        use crate::signature::TagSourceMode;

        let (asset_addr, _) = ctx
            .find_uobject(&source.asset_class, &source.predicate.to_core())
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "set_progress_tags: no live {} matching {:?}",
                    source.asset_class, source.predicate
                ),
            })?;
        let asset_addr = asset_addr as usize;

        let (array_offset, entry_stride) = match source.mode {
            TagSourceMode::Definitions => (Self::SPT_DEFS_ARRAY_OFFSET, Self::SPT_DEF_ENTRY_STRIDE),
            TagSourceMode::RulesValues => (Self::SPT_RULES_ARRAY_OFFSET, Self::SPT_RULE_STRIDE),
        };

        let mut header = [0u8; 16];
        ctx.read_bytes(asset_addr + array_offset as usize, &mut header)
            .map_err(RuntimeError::from)?;
        let data_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap());
        let num = i32::from_le_bytes(header[8..12].try_into().unwrap());
        if data_ptr == 0 || num <= 0 {
            return Ok(Vec::new());
        }
        let num = num as usize;

        let mut body = vec![0u8; num * entry_stride];
        ctx.read_bytes(data_ptr as usize, &mut body)
            .map_err(RuntimeError::from)?;

        let mut tags: Vec<[u8; Self::SPT_FNAME_SIZE]> = Vec::with_capacity(num);
        match source.mode {
            TagSourceMode::Definitions => {
                for i in 0..num {
                    let off = i * Self::SPT_DEF_ENTRY_STRIDE + Self::SPT_DEF_ENTRY_DATA_PTR_OFFSET;
                    let data_ptr = u64::from_le_bytes(body[off..off + 8].try_into().unwrap());
                    if data_ptr == 0 {
                        tags.push([0u8; Self::SPT_FNAME_SIZE]);
                        continue;
                    }
                    let mut tag = [0u8; Self::SPT_FNAME_SIZE];
                    ctx.read_bytes(
                        data_ptr as usize + Self::SPT_DEF_INSTANCE_TAG_OFFSET,
                        &mut tag,
                    )
                    .map_err(RuntimeError::from)?;
                    tags.push(tag);
                }
            }
            TagSourceMode::RulesValues => {
                for i in 0..num {
                    let vh = i * Self::SPT_RULE_STRIDE + Self::SPT_RULE_VALUES_OFFSET;
                    let values_data_ptr = u64::from_le_bytes(body[vh..vh + 8].try_into().unwrap());
                    let values_num = i32::from_le_bytes(body[vh + 8..vh + 12].try_into().unwrap());
                    if values_data_ptr == 0 || values_num <= 0 {
                        tags.push([0u8; Self::SPT_FNAME_SIZE]);
                        continue;
                    }
                    let mut tag = [0u8; Self::SPT_FNAME_SIZE];
                    ctx.read_bytes(values_data_ptr as usize, &mut tag)
                        .map_err(RuntimeError::from)?;
                    tags.push(tag);
                }
            }
        }
        Ok(tags)
    }

    /// Count tags currently at the target value via per-tag
    /// `GetGameProgressValue` calls. Returns `(current_unlocked, total)`.
    /// Skips zero-FName placeholders (null Values entries) — those are
    /// neither counted nor totaled.
    fn count_progress_tags(
        &self,
        ctx: &dyn Ctx,
        source: &crate::signature::TagSourceSpec,
        call: &crate::signature::SetCallSpec,
        value: u8,
        _cached_asset_addr: usize,
    ) -> RuntimeResult<(u32, u32)> {
        let tags = self.extract_progress_tags(ctx, source)?;
        if tags.is_empty() {
            return Ok((0, 0));
        }

        let (world_ctx_addr, _) = ctx
            .find_uobject(
                &call.world_context_class,
                &call.world_context_predicate.to_core(),
            )
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "set_progress_tags: no live world-context {} matching {:?}",
                    call.world_context_class, call.world_context_predicate
                ),
            })?;
        let (statics_addr, statics_class) = ctx
            .find_uobject(&call.statics_class, &openforge_core::Predicate::Any)
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!("set_progress_tags: CDO of {} not found", call.statics_class),
            })?;

        let mut current = 0u32;
        let mut total = 0u32;
        for tag in &tags {
            if tag.iter().all(|b| *b == 0) {
                continue;
            }
            total += 1;
            let mut params = vec![0u8; Self::SPT_GET_PARMS_SIZE];
            params[Self::SPT_PARM_WORLDCTX..Self::SPT_PARM_WORLDCTX + 8]
                .copy_from_slice(&world_ctx_addr.to_le_bytes());
            params[Self::SPT_PARM_TAG..Self::SPT_PARM_TAG + Self::SPT_FNAME_SIZE]
                .copy_from_slice(tag);
            let ret = ctx
                .call_ufunction(statics_addr, statics_class, "GetGameProgressValue", params)
                .map_err(RuntimeError::from)?
                .ok_or_else(|| RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: "GetGameProgressValue not found on statics".into(),
                })?;
            if ret.len() >= Self::SPT_GET_PARMS_SIZE && ret[Self::SPT_PARM_VALUE] == value {
                current += 1;
            }
        }
        Ok((current, total))
    }

    /// Execute the SetProgressTags grant: walk the data-asset's TArray,
    /// extract registered `FGameplayTag`s, then call the configured
    /// static UFunction (`SetGameProgressValue`) once per tag with `value`.
    /// `cached_asset_addr` is unused — we re-find the asset to defend
    /// against game-restart address shift; `extract_progress_tags` does the
    /// re-find.
    fn write_set_progress_tags(
        &self,
        ctx: &dyn Ctx,
        source: &crate::signature::TagSourceSpec,
        call: &crate::signature::SetCallSpec,
        value: u8,
        _cached_asset_addr: usize,
    ) -> RuntimeResult<()> {
        let tags = self.extract_progress_tags(ctx, source)?;
        if tags.is_empty() {
            tracing::warn!(
                feature = self.id(),
                "set_progress_tags: empty TArray — nothing to grant"
            );
            return Ok(());
        }

        let (world_ctx_addr, _) = ctx
            .find_uobject(
                &call.world_context_class,
                &call.world_context_predicate.to_core(),
            )
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "set_progress_tags: no live world-context {} matching {:?}",
                    call.world_context_class, call.world_context_predicate
                ),
            })?;
        let (statics_addr, statics_class) = ctx
            .find_uobject(&call.statics_class, &openforge_core::Predicate::Any)
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!("set_progress_tags: CDO of {} not found", call.statics_class),
            })?;

        let mut granted = 0usize;
        let mut unchanged = 0usize;
        let mut skipped = 0usize;
        for tag in &tags {
            if tag.iter().all(|b| *b == 0) {
                skipped += 1;
                continue;
            }
            let mut params = vec![0u8; Self::SPT_SET_PARMS_SIZE];
            params[Self::SPT_PARM_WORLDCTX..Self::SPT_PARM_WORLDCTX + 8]
                .copy_from_slice(&world_ctx_addr.to_le_bytes());
            params[Self::SPT_PARM_TAG..Self::SPT_PARM_TAG + Self::SPT_FNAME_SIZE]
                .copy_from_slice(tag);
            params[Self::SPT_PARM_VALUE] = value;
            // bOnlyIfHigher (+0x11), ChangeSourceIdentifier (+0x18, FString=16),
            // Instigator (+0x28, UObject*=8), ReturnValue (+0x30) all stay
            // zero. Engine fills ReturnValue.
            let ret = ctx
                .call_ufunction(statics_addr, statics_class, &call.function, params)
                .map_err(RuntimeError::from)?
                .ok_or_else(|| RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: format!(
                        "set_progress_tags: UFunction `{}` not found on {}",
                        call.function, call.statics_class
                    ),
                })?;
            if ret.len() >= Self::SPT_SET_PARMS_SIZE && ret[0x30] != 0 {
                granted += 1;
            } else {
                unchanged += 1;
            }
        }
        tracing::info!(
            feature = self.id(),
            granted,
            unchanged,
            skipped,
            total = tags.len(),
            value,
            "set_progress_tags fired"
        );
        Ok(())
    }

    /// Execute a `CallInstanceUFunction` write: walk the reflection deref
    /// chain to the final pointer SLOT, dereference it to get the live
    /// target object, read its UClass from the UObject header, and invoke
    /// `function_name` with `params` via `call_ufunction`. Used by Fly
    /// Mode: chain ends at `CharacterMovement` (an ObjectProperty), the
    /// slot's pointer is the live `DinnerCharacterMovementComponent`,
    /// and the call is `SetMovementMode(NewMovementMode, NewCustomMode)`.
    fn write_call_instance_ufunction(
        &self,
        ctx: &dyn Ctx,
        function_name: &str,
        params: Vec<u8>,
    ) -> RuntimeResult<()> {
        // Validate that we have a reflection spec with a non-empty deref
        // chain. `validate()` already enforces this; the runtime panic-style
        // expect() here documents the invariant without adding a fallback.
        let _ref_spec = self
            .spec
            .reflection
            .as_ref()
            .expect("validate() guarantees call_instance_ufunction has a [reflection] block");

        // Ensure the primary cache entry is fresh (heals across world
        // transitions). Returns the PawnPrivate (or equivalent primary)
        // slot address.
        let primary_addr = self.ensure_reflection_fresh(ctx)?;

        let chain: Vec<DerefStep> = {
            let guard = self.reflection_cache.lock();
            guard
                .as_ref()
                .expect("ensure_reflection_fresh populates the cache")
                .derefs
                .clone()
        };
        debug_assert!(
            !chain.is_empty(),
            "validate() guarantees the chain is non-empty"
        );

        let target_slot_addr = self.resolve_deref_addr(ctx, primary_addr, &chain)?;
        // One extra deref: the chain stops at the slot holding the target
        // object's pointer (the FINAL hop's resolved property is an
        // ObjectProperty for CallInstanceUFunction, not a value-type field).
        // Read the slot to get the live call target.
        let target_obj = read_pointer(ctx, target_slot_addr).map_err(RuntimeError::from)?;
        if target_obj == 0 {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "call_instance_ufunction: target pointer slot at 0x{target_slot_addr:X} \
                     is null — the live component isn't attached yet (typical: player in a \
                     vehicle, mid level transition, or no pawn possessed)"
                ),
            });
        }
        let class_addr = read_pointer(ctx, target_obj.saturating_add(UOBJECT_CLASS_PRIVATE_OFFSET))
            .map_err(RuntimeError::from)? as u64;

        let params_len = params.len();
        let ret = ctx
            .call_ufunction(target_obj as u64, class_addr, function_name, params)
            .map_err(RuntimeError::from)?;
        if ret.is_none() {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "call_instance_ufunction: UFunction `{function_name}` not found on \
                     target object's class (target_obj=0x{target_obj:X}, \
                     class_addr=0x{class_addr:X})"
                ),
            });
        }
        tracing::info!(
            feature = self.id(),
            function = function_name,
            target = format!("0x{target_obj:X}"),
            params_len,
            "call_instance_ufunction fired"
        );
        Ok(())
    }

    /// Execute one freeze-for-matching tick: walk every live UObject of
    /// `class_path`, apply include/exclude FQN filters host-side, then write
    /// `value` at each `field_offset` on each surviving match.
    ///
    /// Writes are best-effort per-match — a single bad address (e.g. an
    /// object freed between the walk and the write) is logged at debug and
    /// the rest of the matches continue. The freeze loop will retry on the
    /// next tick anyway.
    #[allow(clippy::too_many_arguments)]
    fn write_freeze_for_matching(
        &self,
        ctx: &dyn Ctx,
        class_path: &str,
        field_offsets: &[i64],
        value: f32,
        max_results: u32,
        include_fqn_contains: &[String],
        exclude_fqn_contains: &[String],
    ) -> RuntimeResult<()> {
        let (matches, _truncated) = ctx
            .find_all_uobjects(class_path, &openforge_core::Predicate::Any, max_results)
            .map_err(RuntimeError::from)?;
        if matches.is_empty() {
            return Ok(());
        }
        let bytes = value.to_le_bytes();
        let mut hits = 0usize;
        let mut writes = 0usize;
        for m in &matches {
            // Lowercase once per match so substring tests are cheap.
            let fqn_lc = m.fqn.to_ascii_lowercase();
            if !include_fqn_contains.is_empty()
                && !include_fqn_contains
                    .iter()
                    .all(|n| fqn_lc.contains(&n.to_ascii_lowercase()))
            {
                continue;
            }
            if exclude_fqn_contains
                .iter()
                .any(|n| fqn_lc.contains(&n.to_ascii_lowercase()))
            {
                continue;
            }
            hits += 1;
            let base = m.obj_addr as usize;
            for off in field_offsets {
                let addr = (base as i64).wrapping_add(*off) as usize;
                if let Err(e) = ctx.write_bytes(addr, &bytes) {
                    tracing::debug!(
                        feature = self.id(),
                        addr = format!("0x{addr:X}"),
                        error = %e,
                        "freeze_for_matching: write failed (skipping)"
                    );
                } else {
                    writes += 1;
                }
            }
        }
        tracing::debug!(
            feature = self.id(),
            class = class_path,
            scanned = matches.len(),
            hits,
            writes,
            "freeze_for_matching tick"
        );
        Ok(())
    }

    fn write_teleport_to_waypoint(
        &self,
        ctx: &dyn Ctx,
        waypoint_spec: &ReflectionSpec,
    ) -> RuntimeResult<()> {
        // 1. Resolve waypoint coordinates
        let pred = waypoint_spec.predicate.to_core();
        let (waypoint_obj_addr, waypoint_class_addr) = ctx
            .find_uobject(&waypoint_spec.class_path, &pred)
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "teleport_to_waypoint: no live UObject of class `{}` (predicate {:?}). \
                     Make sure a waypoint pin is currently set on the map screen.",
                    waypoint_spec.class_path, waypoint_spec.predicate
                ),
            })?;

        let primary_resolved = ctx
            .resolve_property(waypoint_class_addr, &waypoint_spec.property_name)
            .map_err(RuntimeError::from)?
            .ok_or_else(|| RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "teleport_to_waypoint: property `{}` not found on waypoint class `{}` chain",
                    waypoint_spec.property_name, waypoint_spec.class_path
                ),
            })?;

        let mut waypoint_addr =
            (waypoint_obj_addr as usize).saturating_add(primary_resolved.offset as usize);

        let waypoint_derefs: Vec<DerefStep> = waypoint_spec
            .deref
            .iter()
            .map(|d| DerefStep {
                property_name: d.property_name.clone(),
                inner_offset: d.inner_offset,
            })
            .collect();

        if !waypoint_derefs.is_empty() {
            waypoint_addr = self.resolve_deref_addr(ctx, waypoint_addr, &waypoint_derefs)?;
        }

        let mut coords = [0u8; 24];
        ctx.read_bytes(waypoint_addr, &mut coords)
            .map_err(RuntimeError::from)?;

        // 2. Resolve player pawn actor
        let player_primary_addr = self.ensure_reflection_fresh(ctx)?;

        let player_chain_opt: Option<Vec<DerefStep>> = {
            let cache_guard = self.reflection_cache.lock();
            cache_guard
                .as_ref()
                .filter(|c| !c.derefs.is_empty())
                .map(|c| c.derefs.clone())
        };

        let player_actor = if let Some(chain) = player_chain_opt {
            let target_slot_addr = self.resolve_deref_addr(ctx, player_primary_addr, &chain)?;
            read_pointer(ctx, target_slot_addr).map_err(RuntimeError::from)?
        } else {
            read_pointer(ctx, player_primary_addr).map_err(RuntimeError::from)?
        };

        if player_actor == 0 {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: "teleport_to_waypoint: player pawn actor is null (no pawn possessed)"
                    .to_string(),
            });
        }

        let player_class_addr = read_pointer(
            ctx,
            player_actor.saturating_add(UOBJECT_CLASS_PRIVATE_OFFSET),
        )
        .map_err(RuntimeError::from)? as u64;

        // 3. Build parameter buffer (FVector DestLocation + FRotator DestRotation)
        let mut params = vec![0u8; 48];
        params[0..24].copy_from_slice(&coords);

        // 4. Call native UFunction K2_TeleportTo on player pawn
        let ret = ctx
            .call_ufunction(
                player_actor as u64,
                player_class_addr,
                "K2_TeleportTo",
                params,
            )
            .map_err(RuntimeError::from)?;

        if ret.is_none() {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "teleport_to_waypoint: UFunction `K2_TeleportTo` not found on player pawn actor (class_addr=0x{:X})",
                    player_class_addr
                ),
            });
        }

        tracing::info!(
            feature = self.id(),
            waypoint = format!("0x{:X}", waypoint_addr),
            player = format!("0x{:X}", player_actor),
            "teleport_to_waypoint fired successfully"
        );
        Ok(())
    }

    fn module_name(&self) -> &str {
        self.spec
            .locator
            .as_ref()
            .and_then(|l| l.module.as_deref())
            .unwrap_or("")
    }

    fn apply_chain(&self, ctx: &dyn Ctx, mut addr: usize) -> CoreResult<usize> {
        if let Some(chain) = &self.spec.pointer_chain {
            for hop in &chain.hops {
                if hop.deref {
                    addr = read_pointer(ctx, addr)?;
                }
                addr = (addr as i64).wrapping_add(hop.offset) as usize;
            }
        }
        Ok(addr)
    }

    fn resolve_via_heap_scan(&self, ctx: &dyn Ctx, hs: &HeapScanSpec) -> RuntimeResult<usize> {
        // Only u64/i64 needles are currently supported. Promotion of smaller
        // value_types could be added later, but the LEGO-style "max-cap as
        // structural fingerprint" pattern is always 64-bit-wide on Windows
        // x64 because the underlying class fields are.
        if !matches!(hs.value_type, ValueKind::I64 | ValueKind::U64) {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "heap_scan.value_type must be i64 or u64 (got {:?})",
                    hs.value_type
                ),
            });
        }
        let needle = hs.value as u64;
        let hits = ctx
            .scan_heap_for_u64_labeled(needle, hs.alignment.max(1), self.id())
            .map_err(RuntimeError::from)?;
        if hits.is_empty() {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "heap_scan found no matches for value 0x{needle:X} — game state may differ from the signature's assumptions"
                ),
            });
        }
        let validators = hs.all_validators();
        let mut survivors: Vec<usize> = Vec::new();
        for &m in &hits {
            let candidate = (m as i64).wrapping_add(hs.field_offset) as usize;
            let mut all_pass = true;
            for v in &validators {
                if !validator_passes(ctx, candidate, v)? {
                    all_pass = false;
                    break;
                }
            }
            if all_pass {
                survivors.push(candidate);
            }
        }
        match survivors.len() {
            0 => Err(RuntimeError::SignatureInvalid {
                feature: self.id().to_string(),
                reason: format!(
                    "heap_scan: {} raw matches but all failed validator; nothing to bind to",
                    hits.len()
                ),
            }),
            1 => {
                tracing::info!(
                    feature = self.id(),
                    addr = format!("0x{:X}", survivors[0]),
                    raw_matches = hits.len(),
                    "heap_scan bound"
                );
                Ok(survivors[0])
            }
            n => {
                let pretty: Vec<String> = survivors.iter().map(|a| format!("0x{a:X}")).collect();
                tracing::warn!(
                    feature = self.id(),
                    candidates = ?pretty,
                    "heap_scan ambiguous"
                );
                Err(RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: format!(
                        "heap_scan: {n} candidates pass validator at {} — tighten the validator",
                        pretty.join(", ")
                    ),
                })
            }
        }
    }
}

fn validator_passes(ctx: &dyn Ctx, candidate: usize, v: &HeapValidatorSpec) -> RuntimeResult<bool> {
    let probe_addr = (candidate as i64).wrapping_add(v.relative_offset) as usize;
    let size = v.field_type.size();
    let mut buf = vec![0u8; size];
    if ctx.read_bytes(probe_addr, &mut buf).is_err() {
        // Unreadable probe = treat candidate as invalid.
        return Ok(false);
    }
    let value = Value::from_le_bytes(v.field_type, &buf)?.as_f64();
    if let Some(min) = v.min
        && value < min
    {
        return Ok(false);
    }
    if let Some(max) = v.max
        && value > max
    {
        return Ok(false);
    }
    Ok(true)
}

impl Feature for DeclarativeFeature {
    fn id(&self) -> &str {
        &self.spec.meta.feature
    }

    fn display_name(&self) -> &str {
        &self.spec.meta.display_name
    }

    fn tagline(&self) -> Option<&str> {
        self.spec.meta.tagline.as_deref()
    }

    fn description(&self) -> Option<&str> {
        self.spec.meta.description.as_deref()
    }

    fn icon(&self) -> Option<&str> {
        self.spec.meta.icon.as_deref()
    }

    fn icon_color(&self) -> Option<&str> {
        self.spec.meta.icon_color.as_deref()
    }

    fn control(&self) -> Option<&ControlSpec> {
        self.spec.control.as_ref()
    }

    fn tier(&self) -> &str {
        &self.spec.meta.tier
    }

    fn category(&self) -> &str {
        &self.spec.meta.kind
    }

    fn kind(&self) -> ValueKind {
        self.spec.effective_kind()
    }

    fn range(&self) -> ValueRange {
        if let Some(v) = &self.spec.value {
            ValueRange {
                min: v.min,
                max: v.max,
            }
        } else {
            ValueRange::default()
        }
    }

    fn write_strategy(&self) -> WriteStrategyKind {
        match self.spec.write {
            WriteSpec::OneShot => WriteStrategyKind::OneShot,
            WriteSpec::Freeze { .. } => WriteStrategyKind::Freeze,
            WriteSpec::CodePatch { .. } => WriteStrategyKind::CodePatch,
            // Action button: kind=Bool, write(true) fires the grant once.
            WriteSpec::SetProgressTags { .. } => WriteStrategyKind::OneShot,
            // FE-wise this acts like a code_patch: a Bool switch whose
            // toggle calls `write(true|false)` once per click. Classifying
            // it as `CodePatch` makes the SwitchControl render the
            // checked-reflects-current-value path, which is what we want
            // (the local store mirrors the write after the IPC succeeds).
            WriteSpec::CallInstanceUFunction { .. } => WriteStrategyKind::CodePatch,
            WriteSpec::TeleportToWaypoint { .. } => WriteStrategyKind::OneShot,
            // FreezeForMatching is a freeze loop driven by the host (the
            // existing `spawn_freeze_task` machinery). The FE renders a
            // freeze switch — `frozen` toggle gates the loop start/stop.
            WriteSpec::FreezeForMatching { .. } => WriteStrategyKind::Freeze,
        }
    }

    fn supports_probe(&self) -> bool {
        // SetProgressTags reads are expensive (N × GetGameProgressValue
        // UFunction calls per invocation). Skip them in the probe loop;
        // their status_text refreshes on-demand after writes anyway.
        //
        // CallInstanceUFunction toggles fire a UFunction every probe tick
        // would re-call SetMovementMode (or equivalent) on cadence — not
        // safe; the read sweep would silently spam the engine. Skip the
        // probe; the SwitchControl tracks its own toggle state.
        //
        // FreezeForMatching reads are O(N) over GUObjectArray; the freeze
        // loop already pays that cost on cadence. Skipping the probe loop
        // avoids doubling the walk frequency.
        !matches!(
            self.spec.write,
            WriteSpec::SetProgressTags { .. }
                | WriteSpec::CallInstanceUFunction { .. }
                | WriteSpec::FreezeForMatching { .. }
        )
    }

    fn freeze_interval_ms(&self) -> u32 {
        match self.spec.write {
            WriteSpec::Freeze { interval_ms, .. } => interval_ms,
            WriteSpec::FreezeForMatching { interval_ms, .. } => interval_ms,
            _ => 250,
        }
    }

    fn freeze_value(&self) -> Option<f64> {
        match self.spec.write {
            WriteSpec::Freeze { freeze_value, .. } => freeze_value,
            // FreezeForMatching's effective_kind is Bool; the freeze loop
            // ticks the feature with whatever Value the host computed from
            // freeze_value(). Without an override here, the loop would
            // capture `read()` → `Bool(false)` → every write would no-op.
            // Returning 1.0 coerces to `Bool(true)` so each tick proceeds
            // into the bulk-write path. The 1.0 itself never reaches game
            // memory — the f32 constant written to each match is `value`
            // from the spec, not this coerced bool.
            WriteSpec::FreezeForMatching { .. } => Some(1.0),
            _ => None,
        }
    }

    fn snapshot(&self, ctx: &dyn Ctx, addr: usize) -> RuntimeResult<Vec<u8>> {
        let size = self.kind().size();
        let companions: Vec<usize> = self
            .reflection_cache
            .lock()
            .as_ref()
            .map(|c| c.companions.clone())
            .unwrap_or_default();
        let total = size * (1 + companions.len());
        let mut buf = vec![0u8; total];
        ctx.read_bytes(addr, &mut buf[..size])
            .map_err(RuntimeError::from)?;
        for (i, comp) in companions.iter().enumerate() {
            let off = size * (1 + i);
            ctx.read_bytes(*comp, &mut buf[off..off + size])
                .map_err(RuntimeError::from)?;
        }
        Ok(buf)
    }

    fn restore(&self, ctx: &dyn Ctx, addr: usize, snapshot: &[u8]) -> RuntimeResult<()> {
        let size = self.kind().size();
        let companions: Vec<usize> = self
            .reflection_cache
            .lock()
            .as_ref()
            .map(|c| c.companions.clone())
            .unwrap_or_default();
        let expected = size * (1 + companions.len());
        if snapshot.len() != expected {
            // Snapshot was captured before companions resolved (or after the
            // cache moved). Fall back to restoring just the primary — partial
            // restore is better than no restore.
            let n = snapshot.len().min(size);
            ctx.write_bytes(addr, &snapshot[..n])
                .map_err(RuntimeError::from)?;
            return Ok(());
        }
        ctx.write_bytes(addr, &snapshot[..size])
            .map_err(RuntimeError::from)?;
        for (i, comp) in companions.iter().enumerate() {
            let off = size * (1 + i);
            ctx.write_bytes(*comp, &snapshot[off..off + size])
                .map_err(RuntimeError::from)?;
        }
        Ok(())
    }

    fn resolve(&self, ctx: &dyn Ctx) -> CoreResult<usize> {
        // FreezeForMatching does discovery on every freeze tick (the live
        // enemy roster changes constantly — new spawns, deaths, level
        // streams). Resolve only needs to confirm "at least one match
        // exists right now" so the UI can move the feature to Ready. We do
        // a single FindAllUObjects walk with max_results=1; any hit is
        // sufficient. The returned address is the first match's obj_addr
        // for logging/cache; the freeze loop re-walks each tick.
        if let WriteSpec::FreezeForMatching {
            class_path,
            max_results,
            ..
        } = &self.spec.write
        {
            let _ = max_results;
            let (matches, _truncated) =
                ctx.find_all_uobjects(class_path, &openforge_core::Predicate::Any, 1)?;
            let first = matches.first().ok_or_else(|| {
                openforge_core::Error::Custom(format!(
                    "freeze_for_matching: no live `{class_path}` instances yet — \
                     enter a level / hub and retry"
                ))
            })?;
            return Ok(first.obj_addr as usize);
        }
        // SetProgressTags features carry their own asset discovery. The
        // resolved address is the data-asset's UObject pointer — useful as
        // a Pending→Ready signal for the UI (the asset isn't loaded until
        // the player enters a level). Returning anything non-zero satisfies
        // the runtime's "feature resolved" contract; the actual TArray
        // iteration runs lazily inside write().
        if let WriteSpec::SetProgressTags { source, .. } = &self.spec.write {
            let pred = source.predicate.to_core();
            let (asset_addr, _class_addr) = ctx
                .find_uobject(&source.asset_class, &pred)?
                .ok_or_else(|| {
                    openforge_core::Error::Custom(format!(
                        "set_progress_tags: no {} matching {:?} is loaded — \
                         is the player in a level?",
                        source.asset_class, source.predicate
                    ))
                })?;
            return Ok(asset_addr as usize);
        }

        let base = if let Some(ref_spec) = &self.spec.reflection {
            self.resolve_via_reflection(ctx, ref_spec).map_err(|e| {
                openforge_core::Error::Custom(format!("reflection resolve failed: {e}"))
            })?
        } else if let Some(hs) = &self.spec.heap_scan {
            self.resolve_via_heap_scan(ctx, hs).map_err(|e| {
                openforge_core::Error::Custom(format!("heap_scan resolve failed: {e}"))
            })?
        } else {
            let locator = self
                .spec
                .locator
                .as_ref()
                .expect("validate() guarantees one of locator/heap_scan/reflection");
            let pattern = self
                .pattern
                .as_ref()
                .expect("pattern is built whenever locator is present");
            let match_addr = ctx
                .scan_module(self.module_name(), pattern)?
                .ok_or_else(|| openforge_core::Error::PatternNotFound(self.id().to_string()))?;
            let resolve: openforge_core::Resolve = locator.resolve.into();
            resolve.apply(ctx, match_addr)?
        };
        self.apply_chain(ctx, base)
    }

    fn read(&self, ctx: &dyn Ctx, addr: usize) -> RuntimeResult<Value> {
        // SetProgressTags: query the runtime state by calling
        // `GetGameProgressValue` per tag. "Applied" = all tags at target
        // value. ~N round-trips per read; affordable on attach + on-demand
        // refresh, NOT in a tight poll loop. The UI switch reads this as
        // ON-when-fully-unlocked, OFF otherwise; once granted, the user
        // can't "unswitch" — write(false) is intentionally a no-op so
        // unlocks persist.
        if let WriteSpec::SetProgressTags {
            source,
            call,
            value,
            ..
        } = &self.spec.write
        {
            let (current, total) = self
                .count_progress_tags(ctx, source, call, *value, addr)
                .unwrap_or((0, 0));
            return Ok(Value::Bool(total > 0 && current == total));
        }
        // CallInstanceUFunction toggles are stateless from the trainer's POV
        // — there's no in-memory bool to read back. The MovementMode byte
        // at CMC+0x241 (for fly mode) does reflect engine state, but probing
        // it would require knowing the live CMC's class layout for every
        // such feature. Returning Bool(false) keeps the toggle OFF on
        // initial sweep; the SwitchControl mirrors the user's last click
        // locally via setCodePatch's post-write store update.
        if matches!(
            self.spec.write,
            WriteSpec::CallInstanceUFunction { .. } | WriteSpec::TeleportToWaypoint { .. }
        ) {
            return Ok(Value::Bool(false));
        }
        // FreezeForMatching has no single "current value" — it writes the
        // same constant to N matched UObjects every tick. The FE freeze
        // switch tracks its own state; returning Bool(false) keeps the
        // initial sweep silent and the toggle starts OFF.
        if matches!(self.spec.write, WriteSpec::FreezeForMatching { .. }) {
            return Ok(Value::Bool(false));
        }
        match &self.spec.write {
            WriteSpec::CodePatch {
                original_bytes,
                patched_bytes,
            } => {
                let patched = parse_hex(patched_bytes)?;
                let mut buf = vec![0u8; patched.len().max(parse_hex(original_bytes)?.len())];
                ctx.read_bytes(addr, &mut buf)?;
                // "Applied" = first byte of buffer matches the patched first byte.
                let applied = !patched.is_empty() && buf[0] == patched[0];
                Ok(Value::Bool(applied))
            }
            _ => {
                let kind = self.kind();
                let mut buf = vec![0u8; kind.size()];
                // For reflection-backed features, validate the cached
                // UObject is still live and re-resolve if not. `addr` is
                // ignored when reflection is configured — the fresh primary
                // from the cache (refreshed if needed) takes precedence,
                // because the attach-time `addr` becomes stale on world
                // transitions while the reflection lookup self-heals.
                let primary_addr = if self.spec.reflection.is_some() {
                    self.ensure_reflection_fresh(ctx)?
                } else {
                    addr
                };
                // Reflection-with-deref: `primary_addr` is the root pointer
                // slot, NOT a value slot. Chase the chain to the final value
                // address. Without deref entries, `primary_addr` IS the value slot.
                let read_addr = {
                    let cache_guard = self.reflection_cache.lock();
                    match cache_guard.as_ref() {
                        Some(c) if !c.derefs.is_empty() => {
                            // Clone the chain so we can drop the lock before
                            // the (potentially slow) IPC walk.
                            let chain = c.derefs.clone();
                            drop(cache_guard);
                            self.resolve_deref_addr(ctx, primary_addr, &chain)?
                        }
                        _ => primary_addr,
                    }
                };
                ctx.read_bytes(read_addr, &mut buf)?;
                Value::from_le_bytes(kind, &buf)
            }
        }
    }

    fn write(&self, ctx: &dyn Ctx, addr: usize, v: Value) -> RuntimeResult<()> {
        // FreezeForMatching: every freeze-task tick lands here with the
        // freeze_value (already coerced to Bool by the spec's effective_kind).
        // Walk the live UObject roster, apply the filters, and write the
        // constant at each match's field_offsets. write(false) is a no-op
        // (toggle OFF stops the freeze task altogether).
        if let WriteSpec::FreezeForMatching {
            class_path,
            field_offsets,
            value,
            max_results,
            include_fqn_contains,
            exclude_fqn_contains,
            ..
        } = &self.spec.write
        {
            let toggle_on = match v {
                Value::Bool(b) => b,
                other => other.as_f64() != 0.0,
            };
            if !toggle_on {
                return Ok(());
            }
            let _ = addr;
            return self.write_freeze_for_matching(
                ctx,
                class_path,
                field_offsets,
                *value,
                *max_results,
                include_fqn_contains,
                exclude_fqn_contains,
            );
        }
        // SetProgressTags: action-trigger. write(true) fires the bulk grant,
        // write(false) is intentionally a no-op (player shouldn't lose
        // unlocks). The cached `addr` is the data-asset's UObject pointer
        // from resolve(); the helper re-finds the asset by predicate anyway
        // so a stale `addr` across game restarts is fine.
        if let WriteSpec::SetProgressTags {
            source,
            call,
            value,
            ..
        } = &self.spec.write
        {
            let fire = match v {
                Value::Bool(b) => b,
                other => other.as_f64() != 0.0,
            };
            if !fire {
                return Ok(());
            }
            return self.write_set_progress_tags(ctx, source, call, *value, addr);
        }
        // CallInstanceUFunction: walk the reflection chain to the final
        // pointer slot, deref it to get the live call target (e.g. the
        // DinnerCharacterMovementComponent for fly mode), and invoke the
        // configured UFunction with the on/off payload. An empty payload
        // is a no-op — used to declare "this action can't be reversed".
        if let WriteSpec::CallInstanceUFunction {
            function,
            params_on,
            params_off,
        } = &self.spec.write
        {
            let toggle_on = match v {
                Value::Bool(b) => b,
                other => other.as_f64() != 0.0,
            };
            let payload_hex = if toggle_on { params_on } else { params_off };
            let payload = crate::signature::parse_hex_payload(payload_hex).map_err(|reason| {
                RuntimeError::SignatureInvalid {
                    feature: self.id().to_string(),
                    reason: format!("call_instance_ufunction: {reason}"),
                }
            })?;
            if payload.is_empty() {
                // Empty params_off (toggle-off has no inverse) — silent no-op.
                tracing::info!(
                    feature = self.id(),
                    toggle = toggle_on,
                    "call_instance_ufunction: empty payload, skipping call"
                );
                return Ok(());
            }
            return self.write_call_instance_ufunction(ctx, function, payload);
        }
        if let WriteSpec::TeleportToWaypoint { waypoint } = &self.spec.write {
            let fire = match v {
                Value::Bool(b) => b,
                other => other.as_f64() != 0.0,
            };
            if !fire {
                return Ok(());
            }
            return self.write_teleport_to_waypoint(ctx, waypoint);
        }
        match &self.spec.write {
            WriteSpec::CodePatch {
                original_bytes,
                patched_bytes,
            } => {
                let apply = match v {
                    Value::Bool(b) => b,
                    other => other.as_f64() != 0.0,
                };
                let original = parse_hex(original_bytes)?;
                let patched = parse_hex(patched_bytes)?;
                if apply {
                    // Route through `patch_code` so the IPC backend
                    // (`Ue5Session`) can register the patch with the DLL's
                    // auto-restore map. Native `Target` falls back to a
                    // verify-and-write via the trait's default impl.
                    ctx.patch_code(addr, &original, &patched)?;
                } else {
                    ctx.restore_code(addr, &original)?;
                }
                Ok(())
            }
            _ => {
                let coerced = v.coerce(self.kind())?;
                self.range().check(coerced.as_f64())?;
                let bytes = coerced.to_le_bytes();

                // For reflection-backed features, validate the cached
                // UObject is still live and re-resolve if not. `addr` is
                // ignored when reflection is configured — the fresh primary
                // from the cache (refreshed if needed) takes precedence,
                // because the attach-time `addr` becomes stale on world
                // transitions while the reflection lookup self-heals.
                let primary_addr = if self.spec.reflection.is_some() {
                    self.ensure_reflection_fresh(ctx)?
                } else {
                    addr
                };

                // Reflection cache drives the write dispatch:
                //
                // * No reflection or reflection-without-deref → write at
                //   `primary_addr` directly, then mirror to `also_write`
                //   companions on the same object.
                // * Reflection-with-deref → `primary_addr` is the root pointer
                //   slot (e.g. PlayerState.PawnPrivate). Writing a typed
                //   value there would corrupt the pointer. Skip the
                //   primary write; walk the chain and write to the
                //   real target's address.
                let chain_opt: Option<Vec<DerefStep>> = {
                    let cache_guard = self.reflection_cache.lock();
                    cache_guard
                        .as_ref()
                        .filter(|c| !c.derefs.is_empty())
                        .map(|c| c.derefs.clone())
                };

                if let Some(chain) = chain_opt {
                    let deref_addr = self.resolve_deref_addr(ctx, primary_addr, &chain)?;
                    ctx.write_bytes(deref_addr, &bytes)?;
                    // `also_write` + deref isn't expressible today: no
                    // current feature needs "deref then mirror to peer
                    // offsets on the dereffed class." Validate() catches
                    // the combination at parse time.
                } else {
                    ctx.write_bytes(primary_addr, &bytes)?;
                    let cache_guard = self.reflection_cache.lock();
                    if let Some(c) = cache_guard.as_ref() {
                        for &companion_addr in &c.companions {
                            ctx.write_bytes(companion_addr, &bytes)?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn quick_check(&self, ctx: &dyn Ctx, addr: usize) -> bool {
        // Reflection-backed features point at live UObjects. Those get
        // re-allocated every game launch, so an address cached from a
        // previous process is almost never valid in a new one — even when
        // the module base happens to match. Force a re-resolve.
        if self.spec.reflection.is_some() {
            return false;
        }
        // FreezeForMatching's cached `addr` is just "the first match seen
        // last time" — useful as a Ready signal, useless across sessions
        // (objects get freed/re-allocated). Always re-resolve.
        if matches!(self.spec.write, WriteSpec::FreezeForMatching { .. }) {
            return false;
        }
        // Heap-scan features: re-run validators against the cached address.
        // The cached `addr` is the post-`field_offset` candidate, so validators
        // (which use `relative_offset` from that point) apply directly.
        if let Some(hs) = &self.spec.heap_scan {
            for v in hs.all_validators() {
                match validator_passes(ctx, addr, v) {
                    Ok(true) => continue,
                    _ => return false,
                }
            }
            return true;
        }
        // AOB-pattern features: cached addresses point inside `.text`, which
        // is stable for the process lifetime. Trust the cache.
        true
    }

    fn status_text(&self, ctx: &dyn Ctx, addr: usize) -> RuntimeResult<Option<String>> {
        // SetProgressTags: compute "X / Y unlocked" + smart completion
        // message. Reuses count_progress_tags from the read() path.
        if let WriteSpec::SetProgressTags {
            source,
            call,
            value,
            display,
        } = &self.spec.write
        {
            let (current, total) = self.count_progress_tags(ctx, source, call, *value, addr)?;
            if total == 0 {
                return Ok(Some("No progress entries discovered.".to_string()));
            }
            // If the signature provides a player-facing display override
            // (PROG_FastTravelUnlock: 65 internal tags but 9 visible map
            // terminals), report against override.total + override.label
            // instead of the raw engine count. Cap `current` to override.total
            // so "65 unlocked" never leaks into the UI.
            if let Some(d) = display {
                let capped = current.min(d.total);
                let msg = if capped >= d.total {
                    format!("All {} {} unlocked.", d.total, d.label)
                } else if capped == 0 {
                    format!("0 / {} {} unlocked — click to grant all.", d.total, d.label)
                } else {
                    let remaining = d.total - capped;
                    format!(
                        "{capped} / {} {} unlocked — {remaining} remaining.",
                        d.total, d.label
                    )
                };
                return Ok(Some(msg));
            }
            let msg = if current >= total {
                // Tailored message for skill features (the only SetProgressTags
                // shipped today). Generic enough to fit future tag-grant
                // features too — "All N unlocked" is universally true.
                let id = self.id();
                if id == "unlock_all_skills" {
                    format!("All {total} skills unlocked. You don't need skill bricks anymore.")
                } else {
                    format!("All {total} unlocked.")
                }
            } else if current == 0 {
                format!("0 / {total} unlocked — click to grant all.")
            } else {
                let remaining = total - current;
                format!("{current} / {total} unlocked — {remaining} remaining.")
            };
            return Ok(Some(msg));
        }
        if let WriteSpec::TeleportToWaypoint { waypoint } = &self.spec.write {
            let pred = waypoint.predicate.to_core();
            if let Ok(Some((waypoint_obj_addr, waypoint_class_addr))) =
                ctx.find_uobject(&waypoint.class_path, &pred)
                && let Ok(Some(primary_resolved)) =
                    ctx.resolve_property(waypoint_class_addr, &waypoint.property_name)
            {
                let waypoint_addr =
                    (waypoint_obj_addr as usize).saturating_add(primary_resolved.offset as usize);
                let waypoint_derefs: Vec<DerefStep> = waypoint
                    .deref
                    .iter()
                    .map(|d| DerefStep {
                        property_name: d.property_name.clone(),
                        inner_offset: d.inner_offset,
                    })
                    .collect();
                let resolved = if !waypoint_derefs.is_empty() {
                    self.resolve_deref_addr(ctx, waypoint_addr, &waypoint_derefs)
                } else {
                    Ok(waypoint_addr)
                };
                if let Ok(addr) = resolved {
                    let mut coords = [0u8; 24];
                    if ctx.read_bytes(addr, &mut coords).is_ok() {
                        let x = f64::from_le_bytes(coords[0..8].try_into().unwrap());
                        let y = f64::from_le_bytes(coords[8..16].try_into().unwrap());
                        let z = f64::from_le_bytes(coords[16..24].try_into().unwrap());
                        return Ok(Some(format!(
                            "Active Waypoint: {:.0}, {:.0}, {:.0}",
                            x, y, z
                        )));
                    }
                }
            }
            return Ok(Some("No active waypoint set on map".to_string()));
        }
        Ok(None)
    }
}

fn parse_hex(s: &str) -> RuntimeResult<Vec<u8>> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        if tok.len() != 2 || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(RuntimeError::SignatureInvalid {
                feature: String::new(),
                reason: format!("bad hex byte {tok:?} in code_patch bytes"),
            });
        }
        out.push(u8::from_str_radix(tok, 16).unwrap());
    }
    Ok(out)
}
