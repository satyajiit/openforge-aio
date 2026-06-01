//! Glacier 2 engine backend: registers `GlacierSession` with the
//! `openforge-engine` dispatch registry under [`EngineKind::Glacier2`].
//!
//! `attach` replicates the exact inject + connect sequence the app's
//! `commands.rs` activate() block performed before Phase 4a
//! (`GlacierSession::attach_pid(pid, dll_path)`), mapping the host `HostError`
//! to the engine-neutral [`EngineAttachError`] at the boundary so attach
//! behavior is byte-identical.

use std::path::Path;
use std::sync::Arc;

use openforge_engine::{
    EngineAttachError, EngineBackend, EngineBackendKind, EngineError, EngineSession, FreezeGuard,
    FreezeHandle, FreezeMode, register_engine,
};
use openforge_glacier_protocol::{GlacierValue, ValueKind};
use openforge_runtime::Value;
use openforge_runtime::manifest::{EngineDecl, EngineKind};

use crate::session::GlacierSession;

/// Map an engine-neutral [`Value`] to the Glacier wire `(GlacierValue,
/// ValueKind)` pair the DLL freeze op expects.
///
/// Total over every [`Value`] variant — no panics, no stubs. The protocol's
/// `ValueKind`/`GlacierValue` enums cover the same numeric widths as the
/// runtime's, so each arm maps one-to-one. `bool` rides the protocol's native
/// `Bool` path (the DLL writes one byte for it). For `CopySibling` god mode the
/// DLL ignores the value entirely; this mapping matters only for `Constant`
/// freezes, where the width must match the field.
fn map_value(value: Value) -> (GlacierValue, ValueKind) {
    match value {
        Value::Bool(b) => (GlacierValue::Bool(b), ValueKind::Bool),
        Value::U8(v) => (GlacierValue::U8(v), ValueKind::U8),
        Value::U16(v) => (GlacierValue::U16(v), ValueKind::U16),
        Value::U32(v) => (GlacierValue::U32(v), ValueKind::U32),
        Value::U64(v) => (GlacierValue::U64(v), ValueKind::U64),
        Value::I8(v) => (GlacierValue::I8(v), ValueKind::I8),
        Value::I16(v) => (GlacierValue::I16(v), ValueKind::I16),
        Value::I32(v) => (GlacierValue::I32(v), ValueKind::I32),
        Value::I64(v) => (GlacierValue::I64(v), ValueKind::I64),
        Value::F32(v) => (GlacierValue::F32(v), ValueKind::F32),
        Value::F64(v) => (GlacierValue::F64(v), ValueKind::F64),
    }
}

/// The Glacier 2 backend factory.
#[derive(Default)]
pub struct Glacier2Backend;

impl EngineBackendKind for Glacier2Backend {
    const KIND: EngineKind = EngineKind::Glacier2;
}

impl EngineBackend for Glacier2Backend {
    fn kind(&self) -> EngineKind {
        EngineKind::Glacier2
    }

    fn default_dll_name(&self) -> &'static str {
        "glacier_007_dll.dll"
    }

    fn attach(
        &self,
        pid: u32,
        dll_path: &Path,
        _decl: &EngineDecl,
    ) -> Result<Arc<dyn EngineSession>, EngineAttachError> {
        // TODO(phase 5): thread real EngineDecl for [engine.glacier2] data constants.
        let session = GlacierSession::attach_pid(pid, dll_path)
            .map_err(|e| EngineAttachError(e.to_string()))?;
        Ok(Arc::new(session))
    }
}

impl EngineSession for GlacierSession {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Glacier2
    }

    fn main_module_base(&self) -> Option<u64> {
        Some(GlacierSession::main_module_base(self))
    }

    fn as_ctx(&self) -> &dyn openforge_core::Ctx {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self
    }

    fn supports_dll_freeze(&self) -> bool {
        true
    }

    fn dll_freeze(
        &self,
        addr: u64,
        value: Value,
        mode: FreezeMode,
        guard: FreezeGuard,
    ) -> Result<FreezeHandle, EngineError> {
        // `Constant` writes a fixed value (no copy); `CopySibling` copies the
        // field at `addr + source_offset` into the target each tick (god mode's
        // current := max). The target write_offset is always 0 — `addr` already
        // points at the field being held.
        let source_offset = match mode {
            FreezeMode::Constant => None,
            FreezeMode::CopySibling { source_offset } => Some(source_offset),
        };
        let (gv, gkind) = map_value(value);
        let handle = self
            .start_freeze(addr, 0, source_offset, gv, gkind, guard.min, guard.max)
            .map_err(|e| EngineError::Backend(e.to_string()))?;
        // glacier-protocol `FreezeHandle` is a `u32`; widen into the neutral
        // `u64` newtype so the app never names the protocol type.
        Ok(FreezeHandle(handle as u64))
    }

    fn dll_unfreeze(&self, handle: FreezeHandle) -> Result<(), EngineError> {
        // Narrow the neutral handle back to the protocol's `u32`. We minted it
        // from a `u32` in `dll_freeze`, so the cast is lossless.
        self.stop_freeze(handle.0 as u32)
            .map_err(|e| EngineError::Backend(e.to_string()))
    }
}

register_engine!(Glacier2Backend);

/// Force the linker to keep this crate's [`register_engine!`] inventory item in
/// the final binary. A `register_engine!` submit only reaches the registry if
/// the crate is linked, and the app stopped referencing any `glacier-host`
/// symbol directly once Phase 4b removed the `GlacierSession` downcast — so the
/// linker would dead-strip the whole crate and `backend_for(Glacier2)` would
/// return `None` at attach. The app calls this once at startup (mirrors
/// `openforge_bundle::ensure_linked`); the `#[used]` static referencing the
/// backend's `KIND` is the belt-and-suspenders against aggressive DCE.
pub fn ensure_linked() {
    #[used]
    static FORCE_LINK: openforge_runtime::manifest::EngineKind =
        <Glacier2Backend as EngineBackendKind>::KIND;
    let _ = FORCE_LINK;
}
