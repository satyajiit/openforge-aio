//! UE5 engine backend: registers `Ue5Session` with the `openforge-engine`
//! dispatch registry under [`EngineKind::Ue5`].
//!
//! `attach` replicates the exact inject + connect sequence the app's
//! `commands.rs` activate() block performed before Phase 4a
//! (`Ue5Session::attach_pid(pid, dll_path)`), mapping the host `HostError` to
//! the engine-neutral [`EngineAttachError`] at the boundary so attach behavior
//! is byte-identical.

use std::path::Path;
use std::sync::Arc;

use openforge_engine::{
    EngineAttachError, EngineBackend, EngineBackendKind, EngineSession, register_engine,
};
use openforge_runtime::manifest::{EngineDecl, EngineKind};

use crate::session::Ue5Session;

/// The UE5 backend factory.
#[derive(Default)]
pub struct Ue5Backend;

impl EngineBackendKind for Ue5Backend {
    const KIND: EngineKind = EngineKind::Ue5;
}

impl EngineBackend for Ue5Backend {
    fn kind(&self) -> EngineKind {
        EngineKind::Ue5
    }

    fn default_dll_name(&self) -> &'static str {
        "batman_lod_dll.dll"
    }

    fn attach(
        &self,
        pid: u32,
        dll_path: &Path,
        _decl: &EngineDecl,
    ) -> Result<Arc<dyn EngineSession>, EngineAttachError> {
        // TODO(phase 5): thread real EngineDecl for [engine.ue5] data constants.
        let session =
            Ue5Session::attach_pid(pid, dll_path).map_err(|e| EngineAttachError(e.to_string()))?;
        Ok(Arc::new(session))
    }
}

impl EngineSession for Ue5Session {
    fn engine_kind(&self) -> EngineKind {
        EngineKind::Ue5
    }

    fn main_module_base(&self) -> Option<u64> {
        Some(Ue5Session::main_module_base(self))
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
}

register_engine!(Ue5Backend);
