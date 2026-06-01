//! Engine-backend dispatch layer.
//!
//! This crate holds **only** the dispatch traits + an `inventory`-driven
//! registry keyed by [`EngineKind`]. It depends on `openforge-core` (for
//! [`Ctx`]) and `openforge-runtime` (for [`EngineKind`] / [`EngineDecl`]) and
//! on nothing else — no Win32, no concrete engine crate. The host crates
//! (`openforge-ue5-host`, `openforge-glacier-host`) implement [`EngineBackend`]
//! plus [`EngineSession`] for their session types and self-register via
//! [`register_engine!`], so the app selects a backend purely from the game's
//! manifest-declared [`EngineKind`], never from a DLL-filename string.
//!
//! The dependency arrows point inward: the app depends on this crate and on the
//! host crates; this crate depends on neither host crate. Linkage of a host
//! crate's `inventory::submit!` items is the only thing that makes its backend
//! discoverable at runtime (see `backend_for`), so the app must keep each host
//! crate linked (it already does, by depending on them).

use std::path::Path;
use std::sync::Arc;

use openforge_core::Ctx;
use openforge_runtime::manifest::{EngineDecl, EngineKind};

/// Failure while injecting + connecting an engine backend's session.
///
/// Deliberately a plain `String` newtype so this crate stays dependency-light
/// and each host crate can map its own `HostError` into it at the boundary
/// (`|e| EngineAttachError(e.to_string())`), preserving today's error text.
#[derive(Debug)]
pub struct EngineAttachError(pub String);

impl std::fmt::Display for EngineAttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineAttachError {}

/// Failure raised by an [`EngineSession`] capability method (e.g. the DLL-side
/// freeze ops). Distinct from [`EngineAttachError`] (which is attach-time only):
/// this is the post-attach operation error surface.
///
/// `Unsupported` carries the capability name so the app can produce a clear
/// message when a feature requests something the session's engine can't do
/// (the trait defaults return this). `Backend` wraps a host crate's own error
/// text at the boundary, keeping this crate dependency-light.
#[derive(Debug)]
pub enum EngineError {
    /// The session's engine does not implement the named capability.
    Unsupported(&'static str),
    /// A backend (host crate / DLL) operation failed; carries its error text.
    Backend(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Unsupported(cap) => write!(f, "unsupported engine capability: {cap}"),
            EngineError::Backend(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for EngineError {}

/// How a DLL-side freeze sources the value it stamps each tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeMode {
    /// Stamp a fixed value every tick.
    Constant,
    /// Copy a sibling field at `addr + source_offset` into the target each tick
    /// (e.g. current := max for difficulty-agnostic god mode).
    CopySibling { source_offset: i64 },
}

/// Plausibility band: the DLL skips a tick if the live target value is outside
/// [min, max], so a freed/reused allocation is never corrupted.
#[derive(Debug, Clone, Copy)]
pub struct FreezeGuard {
    pub min: f32,
    pub max: f32,
}

/// Opaque handle to a running DLL-side freeze slot. Engine-neutral newtype so
/// the app does not depend on any engine's protocol handle type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FreezeHandle(pub u64);

/// One attached engine session. The [`Ctx`] supertrait gives the shared
/// memory + reflection op surface every declarative feature already uses; the
/// inherent methods here expose the engine identity + a downcast escape hatch
/// for the handful of engine-specific call sites (Lua, reflection explorer,
/// Glacier copy-freeze).
pub trait EngineSession: Ctx + Send + Sync {
    /// Which engine this session backs.
    fn engine_kind(&self) -> EngineKind;
    /// Main-module load base — the resolved-address cache key. `None` if the
    /// session could not determine a base (should not happen post-handshake).
    fn main_module_base(&self) -> Option<u64>;
    /// Upcast to `&dyn Ctx`. An explicit method (impl is just `self`) so call
    /// sites never need trait-upcasting syntax.
    fn as_ctx(&self) -> &dyn Ctx;
    /// Downcast hook for the engine-specific call sites (Lua / reflection
    /// explorer downcast to `Ue5Session`; copy-freeze to `GlacierSession`).
    fn as_any(&self) -> &dyn std::any::Any;
    /// Owned-`Arc` downcast hook. Returns `self` as an `Arc<dyn Any + Send +
    /// Sync>` so a call site holding an `Arc<dyn EngineSession>` can recover the
    /// concrete `Arc<Ue5Session>` it needs to move into a long-lived background
    /// task (the read-probe + Lua-polling tasks are typed on the concrete Arc).
    /// Each impl is just `self`.
    fn into_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync>;

    /// Whether this engine performs freezing in-DLL (a per-frame thread). When
    /// false (the default), the app drives a host-side re-write loop instead.
    fn supports_dll_freeze(&self) -> bool {
        false
    }

    /// Start a DLL-side freeze. `value` is the constant to stamp for
    /// [`FreezeMode::Constant`] (ignored by the DLL for
    /// [`FreezeMode::CopySibling`]). Returns a handle the app stores and later
    /// passes to [`EngineSession::dll_unfreeze`].
    ///
    /// The default errors with [`EngineError::Unsupported`]; only engines that
    /// override [`EngineSession::supports_dll_freeze`] to `true` implement it.
    fn dll_freeze(
        &self,
        addr: u64,
        value: openforge_runtime::Value,
        mode: FreezeMode,
        guard: FreezeGuard,
    ) -> Result<FreezeHandle, EngineError> {
        let _ = (addr, value, mode, guard);
        Err(EngineError::Unsupported("dll_freeze"))
    }

    /// Stop a previously started DLL-side freeze. Idempotent at the backend.
    ///
    /// The default errors with [`EngineError::Unsupported`]; only engines that
    /// override [`EngineSession::supports_dll_freeze`] to `true` implement it.
    fn dll_unfreeze(&self, handle: FreezeHandle) -> Result<(), EngineError> {
        let _ = handle;
        Err(EngineError::Unsupported("dll_unfreeze"))
    }
}

/// A registered engine backend: the factory that injects the per-game DLL,
/// handshakes, and produces a boxed [`EngineSession`].
pub trait EngineBackend: Send + Sync + 'static {
    /// Which [`EngineKind`] this backend serves.
    fn kind(&self) -> EngineKind;
    /// Default injected-DLL filename when the manifest declares no explicit
    /// `[engine].dll` (and the legacy `dll_file_name` is empty).
    fn default_dll_name(&self) -> &'static str;
    /// Inject `dll_path` into `pid`, handshake, and build a session. `decl` is
    /// the game's `[engine]` declaration (engine-namespaced data constants are
    /// threaded through here in a later phase; backends currently ignore it).
    fn attach(
        &self,
        pid: u32,
        dll_path: &Path,
        decl: &EngineDecl,
    ) -> Result<Arc<dyn EngineSession>, EngineAttachError>;
}

/// Helper trait carrying the backend's `EngineKind` as an associated const so
/// [`register_engine!`] can submit it without a runtime call.
pub trait EngineBackendKind {
    const KIND: EngineKind;
}

/// One inventory entry per registered backend.
pub struct EngineRegistration {
    pub kind: EngineKind,
    pub make: fn() -> Box<dyn EngineBackend>,
}

inventory::collect!(EngineRegistration);

/// Register an [`EngineBackend`] type with the global registry. The type must
/// be `Default` (the factory is `|| Box::new(<Ty>::default())`) and implement
/// [`EngineBackendKind`]. Invoked once per backend in its host crate.
#[macro_export]
macro_rules! register_engine {
    ($ty:ty) => {
        $crate::inventory::submit! {
            $crate::EngineRegistration {
                kind: <$ty as $crate::EngineBackendKind>::KIND,
                make: || ::std::boxed::Box::new(<$ty>::default()),
            }
        }
    };
}

// Re-export `inventory` so `register_engine!` works in crates that don't
// depend on `inventory` directly.
pub use inventory;

/// Look up the registered backend for `kind`, if any. Returns `None` when no
/// host crate registered a backend for that engine (e.g. the crate wasn't
/// linked).
pub fn backend_for(kind: EngineKind) -> Option<Box<dyn EngineBackend>> {
    crate::inventory::iter::<EngineRegistration>()
        .find(|r| r.kind == kind)
        .map(|r| (r.make)())
}

/// Every engine kind that has a registered backend. Used by the app's startup
/// linkage assertion (R-4 mitigation).
pub fn registered_kinds() -> Vec<EngineKind> {
    crate::inventory::iter::<EngineRegistration>()
        .map(|r| r.kind)
        .collect()
}
