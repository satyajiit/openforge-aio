//! `Send + Sync` facade over the Glacier DLL pipe client.
//!
//! Implements [`openforge_core::Ctx`] (so the same memory primitives features
//! already use work over the injected backend) and exposes Glacier reflection
//! as inherent methods (`resolve_type`, `instance_properties`,
//! `resolve_instance_property`, `set_property`). The UE5-specific `Ctx`
//! reflection methods (`find_uobject`, `resolve_property`, …) keep their
//! erroring defaults — Glacier features go through the inherent methods.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openforge_core::{Ctx, Error as CoreError, Module, Pattern, Result as CoreResult, Target};
use openforge_glacier_protocol::{
    FreezeHandle, GlacierField, GlacierType, GlacierTypeProp, GlacierValue, LogLevel, NodeFire,
    NodeInput, PatternWire, ValueKind,
};
use parking_lot::Mutex;
use tracing::info;

use openforge_host_common::Injector;

use crate::Welcome;
use crate::client::GlacierClient;
use crate::error::{HostError, Result};

/// Default wall-clock budget for opening the pipe after injection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

struct Inner {
    client: Mutex<GlacierClient>,
    welcome: Welcome,
    /// Module list fetched once at attach via `EnumModules`, stored as
    /// `openforge_core::Module` so the `Ctx` `&Module`-returning methods can
    /// hand out borrows. The DLL reports the EXE first, so `modules[0]` is the
    /// main module.
    modules: Vec<Module>,
}

/// High-level facade. Holds the pipe behind a mutex; the request/response
/// protocol is synchronous, so the mutex serialises whole exchanges.
pub struct GlacierSession {
    inner: Arc<Inner>,
}

impl GlacierSession {
    /// Resolve a candidate process, inject `dll_path` (if not already loaded),
    /// and connect the pipe.
    pub fn attach(process_candidates: &[&str], dll_path: &Path) -> Result<Self> {
        let target = Target::attach_by_candidates(process_candidates).map_err(|e| match e {
            openforge_core::Error::ProcessNotFound(s) => HostError::ProcessNotFound(s),
            other => HostError::InjectionFailed(other.to_string()),
        })?;
        Self::attach_pid(target.pid, dll_path)
    }

    /// Inject + connect for an already-resolved pid.
    pub fn attach_pid(pid: u32, dll_path: &Path) -> Result<Self> {
        info!(pid, dll = %dll_path.display(), "injecting glacier DLL");
        Injector::inject(pid, dll_path)?;
        let mut client = GlacierClient::connect(pid, DEFAULT_CONNECT_TIMEOUT)?;
        let welcome = client.welcome().clone();

        // One up-front module enumeration so the `Ctx` impl can hand out
        // `&Module` borrows without locking the client per call.
        let module_entries = client.enum_modules()?;
        let modules: Vec<Module> = module_entries
            .into_iter()
            .map(|m| Module {
                name: m.name,
                base: m.base as usize,
                size: m.size as usize,
                text_offset: m.text_offset as usize,
                text_size: m.text_size as usize,
            })
            .collect();
        if modules.is_empty() {
            return Err(HostError::InjectionFailed(
                "DLL reported zero modules — Toolhelp32 likely failed".into(),
            ));
        }

        Ok(Self {
            inner: Arc::new(Inner {
                client: Mutex::new(client),
                welcome,
                modules,
            }),
        })
    }

    pub fn welcome(&self) -> &Welcome {
        &self.inner.welcome
    }

    pub fn pid(&self) -> u32 {
        self.inner.welcome.pid
    }

    /// Main module load base, cached at attach. A base shift across re-attach
    /// means the game restarted (ASLR) and any cached addresses are stale.
    pub fn main_module_base(&self) -> u64 {
        self.inner.modules[0].base as u64
    }

    pub fn ping(&self) -> Result<()> {
        self.inner.client.lock().ping()
    }

    pub fn drain_log(&self, max_lines: u32) -> Result<Vec<String>> {
        self.inner.client.lock().drain_log(max_lines)
    }

    pub fn set_log_level(&self, level: LogLevel) -> Result<()> {
        self.inner.client.lock().set_log_level(level)
    }

    // -- Glacier reflection (inherent) ------------------------------------

    pub fn resolve_type(&self, name: &str) -> Result<Option<GlacierType>> {
        self.inner.client.lock().resolve_type(name)
    }

    pub fn enumerate_type_properties(&self, name: &str) -> Result<Option<Vec<GlacierTypeProp>>> {
        self.inner.client.lock().enumerate_type_properties(name)
    }

    pub fn instance_properties(&self, entity_va: u64) -> Result<(u64, Vec<GlacierField>)> {
        self.inner.client.lock().instance_properties(entity_va)
    }

    pub fn resolve_instance_property(
        &self,
        entity_va: u64,
        name: &str,
    ) -> Result<Option<GlacierField>> {
        self.inner
            .client
            .lock()
            .resolve_instance_property(entity_va, name)
    }

    /// `Ok(true)` written; `Ok(false)` property not present; `Err` refused/fault.
    pub fn set_property(
        &self,
        entity_va: u64,
        property: &str,
        value: GlacierValue,
    ) -> Result<bool> {
        self.inner
            .client
            .lock()
            .set_property(entity_va, property, value)
    }

    /// Heap-scan for live entities carrying `property` (CRC32 match). Returns
    /// their `ZEntityImpl` VAs — the anchor for targeting a specific entity.
    pub fn find_entities_with_property(
        &self,
        property: &str,
        max_results: u32,
    ) -> Result<Vec<u64>> {
        self.inner
            .client
            .lock()
            .find_entities_with_property(property, max_results)
    }

    /// Fire a logic node's pin (engine `SignalInputPin`) on the game process.
    /// `Ok(true)` = the engine call returned; `Err` = SEH fault / unresolved
    /// engine fn. See [`GlacierClient::fire_node`].
    pub fn fire_node(&self, node_va: u64, inputs: Vec<NodeInput>, fire: NodeFire) -> Result<bool> {
        self.inner.client.lock().fire_node(node_va, inputs, fire)
    }

    /// Start a DLL-side guarded per-frame freeze (protocol v4). See
    /// [`GlacierClient::start_freeze`]. Used by god mode to hold current health
    /// by copying max (`source_offset`) each tick, difficulty-agnostically.
    #[allow(clippy::too_many_arguments)]
    pub fn start_freeze(
        &self,
        box_va: u64,
        write_offset: i64,
        source_offset: Option<i64>,
        value: GlacierValue,
        value_kind: ValueKind,
        guard_min: f32,
        guard_max: f32,
    ) -> Result<FreezeHandle> {
        self.inner.client.lock().start_freeze(
            box_va,
            write_offset,
            source_offset,
            value,
            value_kind,
            guard_min,
            guard_max,
        )
    }

    /// Stop a freeze started by [`GlacierSession::start_freeze`]. Idempotent.
    pub fn stop_freeze(&self, handle: FreezeHandle) -> Result<()> {
        self.inner.client.lock().stop_freeze(handle)
    }

    /// Query a freeze's `(writes, skipped, ticks)` counters.
    pub fn query_freeze_stats(&self, handle: FreezeHandle) -> Result<(u64, u64, u64)> {
        self.inner.client.lock().query_freeze_stats(handle)
    }
}

// SAFETY: `Inner` holds only `Mutex<T: Send>` + plain data. The pipe HANDLE in
// `GlacierClient` is treated as `Send` (see `pipe::PipeHandle`) and is never
// exposed across threads without the `client` mutex.
unsafe impl Send for GlacierSession {}
unsafe impl Sync for GlacierSession {}

/// `Ctx` impl proxying every read / write / scan through the pipe to the DLL.
impl Ctx for GlacierSession {
    fn pid(&self) -> u32 {
        self.inner.welcome.pid
    }

    fn main_module(&self) -> &Module {
        &self.inner.modules[0]
    }

    fn module(&self, name: &str) -> Option<&Module> {
        if name.is_empty() {
            return Some(self.main_module());
        }
        self.inner
            .modules
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> CoreResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let bytes = self
            .inner
            .client
            .lock()
            .read_bytes(addr as u64, buf.len() as u32)
            .map_err(host_err)?;
        if bytes.len() != buf.len() {
            return Err(CoreError::Custom(format!(
                "glacier ipc read_bytes: short response ({}/{})",
                bytes.len(),
                buf.len()
            )));
        }
        buf.copy_from_slice(&bytes);
        Ok(())
    }

    fn write_bytes(&self, addr: usize, buf: &[u8]) -> CoreResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.inner
            .client
            .lock()
            .write_bytes(addr as u64, buf.to_vec())
            .map_err(host_err)
    }

    fn scan_module(&self, name: &str, pattern: &Pattern) -> CoreResult<Option<usize>> {
        let (bytes, mask) = pattern.wire();
        let wire = PatternWire { bytes, mask };
        let result = self
            .inner
            .client
            .lock()
            .scan_module(name, wire)
            .map_err(host_err)?;
        Ok(result.map(|a| a as usize))
    }

    fn scan_module_all(&self, name: &str, pattern: &Pattern) -> CoreResult<Vec<usize>> {
        let (bytes, mask) = pattern.wire();
        let wire = PatternWire { bytes, mask };
        let v = self
            .inner
            .client
            .lock()
            .scan_module_all(name, wire)
            .map_err(host_err)?;
        Ok(v.into_iter().map(|a| a as usize).collect())
    }

    fn scan_heap_for_u64_labeled(
        &self,
        needle: u64,
        alignment: usize,
        label: &str,
    ) -> CoreResult<Vec<usize>> {
        let v = self
            .inner
            .client
            .lock()
            .scan_heap_for_u64(needle, alignment as u32, label)
            .map_err(host_err)?;
        Ok(v.into_iter().map(|a| a as usize).collect())
    }
}

fn host_err(e: HostError) -> CoreError {
    CoreError::Custom(format!("glacier-host ipc: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GlacierSession>();
    }
}
