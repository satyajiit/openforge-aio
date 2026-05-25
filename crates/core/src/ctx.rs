use crate::error::{Error, Result};
use crate::module::Module;
use crate::pattern::Pattern;

/// Discriminator for [`Ctx::find_uobject`] when multiple instances of a
/// class exist in the live UE5 object array (typical for `*PlayerState`,
/// `*Currency_*`, etc. — one class-default object plus N live instances).
///
/// Mirrors `openforge_ue5_protocol::NamePredicate` at the Ctx-trait level
/// so `openforge_core` stays free of any wire-protocol dependency. The
/// IPC-backed `Ue5Session` converts at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    /// Any live instance. Returns the first match in GUObjectArray
    /// iteration order — includes the class-default object (`Default__X`),
    /// which is almost never what a feature wants.
    Any,
    /// Object name exactly equals (case-insensitive).
    Exact(String),
    /// Object name or FQN contains the substring (case-insensitive).
    Contains(String),
    /// FQN starts with the prefix (case-insensitive). The canonical
    /// "skip the CDO, find the live transient instance" form is
    /// `FqnPrefix("/Engine/Transient/")`.
    FqnPrefix(String),
}

/// In-trait mirror of `openforge_ue5_protocol::PropKind`. Same rationale
/// as [`Predicate`] — keeps `openforge_core` wire-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
    /// Raw bytes for FProperty types the trait doesn't decode (ObjectProperty,
    /// StructProperty, NameProperty, …). `n` is the FProperty's `ElementSize`.
    Bytes(u32),
}

impl FieldKind {
    pub fn size(self) -> usize {
        match self {
            FieldKind::I8 | FieldKind::U8 | FieldKind::Bool => 1,
            FieldKind::I16 | FieldKind::U16 => 2,
            FieldKind::I32 | FieldKind::U32 | FieldKind::F32 => 4,
            FieldKind::I64 | FieldKind::U64 | FieldKind::F64 => 8,
            FieldKind::Bytes(n) => n as usize,
        }
    }
}

/// Result of [`Ctx::resolve_property`]: where on a UObject the named
/// FProperty lives and how to interpret its bytes. `defined_in_class` is
/// the class FQN that owns the property (may be a parent in the super
/// chain — UE5 inherits properties through `UStruct.SuperStruct`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedField {
    pub offset: u32,
    pub size: u32,
    pub kind: FieldKind,
    pub defined_in_class: String,
}

/// Single match returned by [`Ctx::find_all_uobjects`]. The FQN is carried
/// alongside the addresses so callers can apply host-side filters richer
/// than the wire predicate (e.g. "include any FQN containing `_Goon_` but
/// exclude `Default__`"). FQN format matches `UeObjectRef::fqn` on the
/// protocol side (slash-separated, leading `/Game/...` or `/Script/...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundObject {
    pub obj_addr: u64,
    pub class_addr: u64,
    pub fqn: String,
}

/// Abstraction over the live target process.
///
/// Designed so the same `Feature` and `Resolve` code paths work over either a
/// native Win32-backed `Target` (today) or a future host-shim that proxies a
/// sandboxed plugin (e.g. WebAssembly) through host functions. All memory
/// access goes through this trait — there are no raw Win32 calls in feature
/// implementations.
pub trait Ctx {
    fn pid(&self) -> u32;
    fn main_module(&self) -> &Module;
    fn module(&self, name: &str) -> Option<&Module>;

    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()>;
    fn write_bytes(&self, addr: usize, buf: &[u8]) -> Result<()>;

    /// Scan a module's `.text` section. `name` is matched case-insensitively;
    /// `""` means the main module.
    fn scan_module(&self, name: &str, pattern: &Pattern) -> Result<Option<usize>>;

    fn scan_module_all(&self, name: &str, pattern: &Pattern) -> Result<Vec<usize>>;

    /// Scan all committed RW regions in the target's address space for u64
    /// values exactly equal to `needle`, returning the matching source
    /// addresses. The needle is interpreted little-endian.
    ///
    /// Used by the `heap_value_scan` locator kind for re-discovering
    /// heap-allocated objects whose addresses shift across game sessions.
    /// Default implementation returns empty so non-Windows builds still
    /// compile; `Target` overrides it with the real VirtualQueryEx walk.
    ///
    /// Default impl forwards to the labeled variant with an empty label, so
    /// implementors that only need to override one method can override either.
    fn scan_heap_for_u64(&self, needle: u64, alignment: usize) -> Result<Vec<usize>> {
        self.scan_heap_for_u64_labeled(needle, alignment, "")
    }

    /// Same as [`Ctx::scan_heap_for_u64`], but tags the operation with a
    /// human-readable label (typically the feature id). Implementations that
    /// emit progress events can use this to attribute each event to the
    /// feature that triggered the scan.
    ///
    /// Default impl ignores the label and returns empty — implementors should
    /// override **this** method (the unlabeled one will delegate here).
    fn scan_heap_for_u64_labeled(
        &self,
        _needle: u64,
        _alignment: usize,
        _label: &str,
    ) -> Result<Vec<usize>> {
        Ok(Vec::new())
    }

    /// Apply a code patch in `.text`. Verifies the bytes currently at
    /// `addr` equal `original`; refuses (returns `Err`) on mismatch. On
    /// success returns the previous bytes (always equal to `original`,
    /// surfaced for caller round-trip verification).
    ///
    /// Default impl does the read-verify-write dance via [`Ctx::read_bytes`]
    /// and [`Ctx::write_bytes`]. The IPC-backed Ctx (`Ue5Session`) overrides
    /// to route through the DLL's `CodePatch` op so the DLL can auto-restore
    /// the patch when the pipe disconnects.
    fn patch_code(&self, addr: usize, original: &[u8], replacement: &[u8]) -> Result<Vec<u8>> {
        if replacement.is_empty() {
            return Err(Error::Custom("code_patch: replacement is empty".into()));
        }
        if original.len() != replacement.len() {
            return Err(Error::Custom(format!(
                "code_patch: length mismatch (original={}, replacement={})",
                original.len(),
                replacement.len()
            )));
        }
        let mut current = vec![0u8; replacement.len()];
        self.read_bytes(addr, &mut current)?;
        if current != original {
            return Err(Error::Custom(format!(
                "code_patch: bytes at 0x{addr:X} don't match `original`; \
                 expected={original:02X?} actual={current:02X?}"
            )));
        }
        self.write_bytes(addr, replacement)?;
        Ok(current)
    }

    /// Restore previously-patched bytes. Idempotent: writing `original`
    /// over bytes that already equal `original` is a no-op success.
    ///
    /// Default impl: a raw [`Ctx::write_bytes`]. The IPC-backed Ctx routes
    /// through the DLL's `RestorePatch` op so the DLL can drop the
    /// associated entry from its auto-restore map.
    fn restore_code(&self, addr: usize, original: &[u8]) -> Result<()> {
        self.write_bytes(addr, original)
    }

    /// Find a live UE5 UObject by class name + disambiguating predicate.
    /// Returns `Ok(None)` when the object isn't loaded yet (typical when
    /// the game is in the main menu or between levels). Returns `Ok(Some(
    /// (obj_addr, class_addr)))` on success — the second component is the
    /// `UClass*` to feed into [`Ctx::resolve_property`].
    ///
    /// **Reflection support is per-impl.** Only an IPC-backed `Ctx` (the
    /// `Ue5Session` produced by the trainer's DLL injection) implements
    /// this. Native `Target` (used by the discover CLI for raw-memory
    /// exploration) returns an error — there's no UE5 reflection machinery
    /// to walk without a DLL injected.
    fn find_uobject(
        &self,
        _class_path: &str,
        _predicate: &Predicate,
    ) -> Result<Option<(u64, u64)>> {
        Err(Error::Custom(
            "reflection: this Ctx does not support find_uobject \
             (only the IPC-backed Ue5Session does)"
                .into(),
        ))
    }

    /// Multi-result variant of [`Ctx::find_uobject`]. Returns **every** live
    /// UObject whose UClass name matches `class_path` (case-insensitive) and
    /// whose FQN passes `predicate`, capped at `max_results`. The bool is
    /// `true` iff the walk hit the cap before completing.
    ///
    /// Used by "iterate-and-write" features (e.g. one-hit-kill freezing
    /// Health.CurrentValue on every live enemy's HealthAttributeSet). Same
    /// per-impl support story as [`Ctx::find_uobject`].
    fn find_all_uobjects(
        &self,
        _class_path: &str,
        _predicate: &Predicate,
        _max_results: u32,
    ) -> Result<(Vec<FoundObject>, bool)> {
        Err(Error::Custom(
            "reflection: this Ctx does not support find_all_uobjects \
             (only the IPC-backed Ue5Session does)"
                .into(),
        ))
    }

    /// Class-name substring variant of [`Ctx::find_all_uobjects`]. Walks
    /// every live UObject and returns those whose UClass name (case-
    /// insensitive) contains **any** of `class_substrings`. Use this when
    /// the target population spans many BP subclasses with a shared naming
    /// convention (`BP_JokerGang_*_Goon_C`, `BP_GCPD_SWAT_*_C`, …) that
    /// can't be expressed via the exact-match `find_all_uobjects`.
    ///
    /// Backed by the host-side cached `walk_objects()` snapshot — first
    /// call per session pays a single IPC round-trip; subsequent calls are
    /// in-memory filtering.
    fn find_by_class_substring(
        &self,
        _class_substrings: &[String],
        _max_results: u32,
    ) -> Result<(Vec<FoundObject>, bool)> {
        Err(Error::Custom(
            "reflection: this Ctx does not support find_by_class_substring \
             (only the IPC-backed Ue5Session does)"
                .into(),
        ))
    }

    /// Look up a single FProperty by name on `class_addr` (or any super in
    /// its chain). Returns `Ok(None)` when the property doesn't exist on
    /// the class chain — treat this as a hard configuration error (the
    /// feature TOML names a property the game doesn't have), not a
    /// transient miss.
    ///
    /// Same per-impl support story as [`Ctx::find_uobject`].
    fn resolve_property(
        &self,
        _class_addr: u64,
        _property_name: &str,
    ) -> Result<Option<ResolvedField>> {
        Err(Error::Custom(
            "reflection: this Ctx does not support resolve_property \
             (only the IPC-backed Ue5Session does)"
                .into(),
        ))
    }

    /// Call a UE5 UFunction by name on a live UObject via the DLL's
    /// `ProcessEvent` dispatcher. `params` is the caller's pre-laid-out
    /// parameter blob (matching the UFunction's child-FProperty layout);
    /// the DLL zero-pads or rejects oversized blobs and returns the
    /// post-call buffer (out-parameters and return slot updated by the
    /// engine). `Ok(None)` means the named function doesn't exist on the
    /// class chain — treat as a hard configuration error.
    ///
    /// For static UFunctions (BlueprintFunctionLibrary methods like
    /// `TtGameProgressStatics::SetGameProgressValue`), pass the class CDO
    /// as `obj_addr`. The CDO is what `find_uobject(class_name, Any)`
    /// returns first.
    ///
    /// Same per-impl support story as [`Ctx::find_uobject`] — only the
    /// IPC-backed `Ue5Session` implements this.
    fn call_ufunction(
        &self,
        _obj_addr: u64,
        _class_addr: u64,
        _function_name: &str,
        _params: Vec<u8>,
    ) -> Result<Option<Vec<u8>>> {
        Err(Error::Custom(
            "reflection: this Ctx does not support call_ufunction \
             (only the IPC-backed Ue5Session does)"
                .into(),
        ))
    }
}

/// Read a 64-bit little-endian pointer at `addr`.
pub fn read_pointer(ctx: &dyn Ctx, addr: usize) -> Result<usize> {
    let mut buf = [0u8; 8];
    ctx.read_bytes(addr, &mut buf)?;
    Ok(u64::from_le_bytes(buf) as usize)
}
