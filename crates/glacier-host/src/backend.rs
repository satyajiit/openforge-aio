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
    EngineAttachError, EngineBackend, EngineBackendKind, EngineSession, register_engine,
};
use openforge_runtime::manifest::{EngineDecl, EngineKind};

use crate::session::GlacierSession;

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
}

register_engine!(Glacier2Backend);
