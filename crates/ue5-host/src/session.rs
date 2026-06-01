//! High-level `Send + Sync` facade with caches mirroring `UeEngine` in the
//! discover crate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openforge_core::{
    Ctx, Error as CoreError, FieldKind, Module, Pattern, Predicate, ResolvedField,
    Result as CoreResult, Target,
};
use openforge_ue5_protocol::{
    LuaOutputLine, LuaScriptStatus, NamePredicate, PropInfo, PropKind, PropValue, ResolvedProperty,
    UFunctionInfo, UeObjectRef,
};
use parking_lot::Mutex;
use tracing::{debug, info};

use crate::Welcome;
use crate::client::Ue5Client;
use crate::error::{HostError, Result};
use crate::injector::Injector;

/// Default wall-clock budget for opening the pipe after injection.
/// 5 s comfortably covers DLL DllMain + locator + pipe-server thread start.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared client state: pipe + the lazy caches.
///
/// `client` is a `parking_lot::Mutex<Ue5Client>` because the request/response
/// protocol is synchronous and stateful — concurrent senders would interleave
/// each other's responses. The mutex serialises *whole* request-response
/// pairs.
struct Inner {
    client: Mutex<Ue5Client>,
    objects: Mutex<Option<Arc<Vec<UeObjectRef>>>>,
    props: Mutex<HashMap<u64, Arc<Vec<PropInfo>>>>,
    funcs: Mutex<HashMap<u64, Arc<Vec<UFunctionInfo>>>>,
    /// Resolved-property cache: `(class_addr, property_name_lowercased)` →
    /// metadata. Safe to cache for the life of the session because UClass
    /// FProperty layouts are immutable for the life of the process (the
    /// game doesn't load classes that mutate offsets at runtime). Cleared
    /// only on session drop.
    resolved_props: Mutex<HashMap<(u64, String), ResolvedProperty>>,
    welcome: Welcome,
    /// Cached module list fetched once at attach via `Request::EnumModules`.
    /// Stored as `Vec<Module>` so the `Ctx` trait's `&Module`-returning
    /// methods can hand out borrowed references tied to `&self.inner`.
    /// Toolhelp32 reports the EXE first, so `modules[0]` is the main module.
    modules: Vec<Module>,
}

/// High-level facade. Cheap to clone via `Arc<Inner>` if a caller needs to
/// hold two references; we don't `#[derive(Clone)]` so misuse is explicit.
pub struct Ue5Session {
    inner: Arc<Inner>,
}

impl Ue5Session {
    /// Find a candidate process, inject `dll_path` (if not already loaded),
    /// and connect the pipe.
    ///
    /// `process_candidates` is the same shape `Target::attach_by_candidates`
    /// takes (case-insensitive exe-name match). `dll_path` points at the
    /// per-game DLL the caller wants injected — typically resolved via
    /// [`crate::resolve_dll_path`] using the game crate's `DLL_FILE_NAME`.
    pub fn attach(process_candidates: &[&str], dll_path: &Path) -> Result<Self> {
        let target = Target::attach_by_candidates(process_candidates).map_err(|e| match e {
            openforge_core::Error::ProcessNotFound(s) => HostError::ProcessNotFound(s),
            other => HostError::InjectionFailed(other.to_string()),
        })?;
        Self::attach_pid(target.pid, dll_path)
    }

    /// Inject + connect for an already-resolved pid (e.g. discover's CLI flow
    /// builds a `Target` up-front for its scanning APIs).
    pub fn attach_pid(pid: u32, dll_path: &Path) -> Result<Self> {
        info!(pid, dll = %dll_path.display(), "injecting DLL");
        Injector::inject(pid, dll_path)?;
        let mut client = Ue5Client::connect(pid, DEFAULT_CONNECT_TIMEOUT)?;
        let welcome = client.welcome().clone();
        // Note: `layout_validated == false` is NOT a session-level error.
        // FProperty-offset validation only matters for signature emission
        // (which gates on `welcome().layout_validated` at the use site —
        // see `crates/discover/src/ue5/emit.rs`). Object walks, name decoding,
        // and class dumping all work fine without it. Callers that need
        // emission-grade validation should check `welcome().layout_validated`
        // and map it to `HostError::LayoutUnresolved` themselves.
        if !welcome.layout_validated {
            tracing::warn!(
                pid,
                "DLL reports layout_validated=false; emission paths will refuse, walks proceed"
            );
        }
        // One up-front module enumeration so the `Ctx` impl can hand out
        // `&Module` references without locking the client per call. Failure
        // here is fatal — without the module list we can't service any
        // feature's `scan_module` call, so there's no recovery path.
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
                objects: Mutex::new(None),
                props: Mutex::new(HashMap::new()),
                funcs: Mutex::new(HashMap::new()),
                resolved_props: Mutex::new(HashMap::new()),
                welcome,
                modules,
            }),
        })
    }

    /// Cached resolved layout from the DLL.
    pub fn welcome(&self) -> &Welcome {
        &self.inner.welcome
    }

    /// Lazy, cached `walk_objects()`.
    pub fn walk_objects(&self) -> Result<Arc<Vec<UeObjectRef>>> {
        if let Some(cached) = self.inner.objects.lock().clone() {
            return Ok(cached);
        }
        let v = self.inner.client.lock().walk_objects(None)?;
        let arc = Arc::new(v);
        *self.inner.objects.lock() = Some(Arc::clone(&arc));
        debug!(len = arc.len(), "cached walk_objects");
        Ok(arc)
    }

    /// Lazy, cached `walk_properties(class_ptr)`.
    pub fn walk_properties_cached(&self, class_ptr: u64) -> Result<Arc<Vec<PropInfo>>> {
        if let Some(cached) = self.inner.props.lock().get(&class_ptr).cloned() {
            return Ok(cached);
        }
        let v = self.inner.client.lock().walk_properties(class_ptr)?;
        let arc = Arc::new(v);
        self.inner.props.lock().insert(class_ptr, Arc::clone(&arc));
        Ok(arc)
    }

    /// Lazy, cached `walk_functions(class_ptr)`.
    pub fn walk_functions_cached(&self, class_ptr: u64) -> Result<Arc<Vec<UFunctionInfo>>> {
        if let Some(cached) = self.inner.funcs.lock().get(&class_ptr).cloned() {
            return Ok(cached);
        }
        let v = self.inner.client.lock().walk_functions(class_ptr)?;
        let arc = Arc::new(v);
        self.inner.funcs.lock().insert(class_ptr, Arc::clone(&arc));
        Ok(arc)
    }

    /// Read-through to the client. Bypasses caches because values change.
    pub fn read_property(&self, addr: u64, kind: PropKind) -> Result<PropValue> {
        self.inner.client.lock().read_property(addr, kind)
    }

    /// Write-through to the client.
    pub fn write_property(&self, addr: u64, value: PropValue) -> Result<()> {
        self.inner.client.lock().write_property(addr, value)
    }

    /// Pid the session is talking to.
    pub fn pid(&self) -> u32 {
        self.inner.welcome.pid
    }

    /// Main module load base (cached from `EnumModules` at attach). Useful
    /// for the trainer's per-attach address-cache invalidation key: a base
    /// shift across runs means the game restarted (ASLR or otherwise) and
    /// the cached addresses are stale.
    pub fn main_module_base(&self) -> u64 {
        self.inner.modules[0].base as u64
    }

    /// Cheap liveness ping.
    pub fn ping(&self) -> Result<()> {
        self.inner.client.lock().ping()
    }

    /// Drain the DLL's in-memory log ring.
    pub fn drain_log(&self, max_lines: u32) -> Result<Vec<String>> {
        self.inner.client.lock().drain_log(max_lines)
    }

    /// Update the DLL's log level.
    pub fn set_log_level(&self, level: openforge_ue5_protocol::LogLevel) -> Result<()> {
        self.inner.client.lock().set_log_level(level)
    }

    // -- Reflection-promoted feature helpers ---------------------------------

    /// Find a live UObject by class name. **Never cached** — the result is
    /// inherently transient (the object exists once the player is in a
    /// world; not before). Callers use this in a poll loop and transition
    /// the feature to `Ready` on `Ok(Some(_))`.
    pub fn find_uobject(
        &self,
        class_path: &str,
        predicate: NamePredicate,
    ) -> Result<Option<(u64, u64)>> {
        self.inner.client.lock().find_uobject(class_path, predicate)
    }

    /// Multi-result UObject lookup. `max_results` caps the DLL's walk so the
    /// response frame stays under the 256 KB framer limit; for current
    /// "iterate-and-write" features (one-hit-kill), a few hundred is
    /// generous (the largest LotDK level dump showed ~50 enemies + ~50
    /// vehicles with HealthAttributeSet at peak). Result lifetime: same as
    /// individual `FindUObject` returns — valid until the listed UObjects
    /// are destroyed.
    pub fn find_all_uobjects(
        &self,
        class_path: &str,
        predicate: NamePredicate,
        max_results: u32,
    ) -> Result<(Vec<openforge_ue5_protocol::FoundObjectEntry>, bool)> {
        self.inner
            .client
            .lock()
            .find_all_uobjects(class_path, predicate, max_results)
    }

    /// Resolve a single FProperty by name on `class_addr` (or any super in
    /// its chain). Result is cached for the life of the session — UClass
    /// layouts don't change at runtime, so the second call to the same
    /// `(class_addr, property_name)` is a HashMap lookup. `Ok(None)` means
    /// the property name doesn't exist on the chain (treat as TOML config
    /// error; not a transient miss).
    pub fn resolve_property(
        &self,
        class_addr: u64,
        property_name: &str,
    ) -> Result<Option<ResolvedProperty>> {
        let key = (class_addr, property_name.to_ascii_lowercase());
        if let Some(cached) = self.inner.resolved_props.lock().get(&key).cloned() {
            return Ok(Some(cached));
        }
        let resolved = self
            .inner
            .client
            .lock()
            .resolve_property(class_addr, property_name)?;
        if let Some(ref r) = resolved {
            self.inner.resolved_props.lock().insert(key, r.clone());
        }
        Ok(resolved)
    }

    /// Call a UFunction on a live UObject by name, via the engine's
    /// `ProcessEvent` dispatcher. `params` is the caller's serialized
    /// argument blob (laid out in the UFunction's expected parameter
    /// order). Returns the post-call buffer (out-parameters + return slot
    /// updated by the engine). `Ok(None)` means the named function doesn't
    /// exist on the class chain.
    pub fn call_ufunction(
        &self,
        obj_addr: u64,
        class_addr: u64,
        function_name: &str,
        params: Vec<u8>,
    ) -> Result<Option<Vec<u8>>> {
        self.inner
            .client
            .lock()
            .call_ufunction(obj_addr, class_addr, function_name, params)
    }

    // -- Lua runtime (protocol v5) ----------------------------------------

    /// Dispatch a Lua script onto the DLL's in-process mlua VM. Returns
    /// once the chunk's main body has been received and the worker thread
    /// has begun executing it (which for keybind-only scripts means
    /// registrations are in place and callbacks armed). See
    /// [`crate::Ue5Client::run_lua`] for error semantics.
    pub fn run_lua(&self, script: String, name: String) -> Result<()> {
        self.inner.client.lock().run_lua(script, name)
    }

    /// Cancel the in-flight Lua script (if any). Idempotent.
    pub fn stop_lua(&self) -> Result<()> {
        self.inner.client.lock().stop_lua()
    }

    /// Snapshot the Lua VM lifecycle. Cheap; the polling task in the Tauri
    /// backend calls this every 250 ms while a script is running.
    pub fn lua_status(&self) -> Result<LuaScriptStatus> {
        self.inner.client.lock().lua_status()
    }

    /// Drain up to `max_lines` of captured `print()` output (oldest first).
    /// `0` drains everything currently buffered.
    pub fn drain_lua_output(&self, max_lines: u32) -> Result<Vec<LuaOutputLine>> {
        self.inner.client.lock().drain_lua_output(max_lines)
    }
}

// SAFETY: `Inner` contains only `Mutex<T: Send>` fields and plain data;
// `parking_lot::Mutex` is `Send + Sync` when `T: Send`, which holds for all
// our fields. The pipe HANDLE inside `Ue5Client` is treated as `Send` (see
// `pipe::PipeHandle`), and we never expose it across threads without the
// `client` mutex.
unsafe impl Send for Ue5Session {}
unsafe impl Sync for Ue5Session {}

/// `Ctx` impl that proxies every read / write / scan / patch through the
/// pipe to the in-game DLL. The host never touches the game's memory
/// directly — this is the single-channel migration's whole point.
///
/// All methods take a `parking_lot::Mutex` lock on the underlying client
/// for the duration of one request-response pair. The protocol is
/// synchronous and stateful (frames must not interleave), so single-writer
/// serialisation is by design.
impl Ctx for Ue5Session {
    fn pid(&self) -> u32 {
        self.inner.welcome.pid
    }

    fn main_module(&self) -> &Module {
        // Toolhelp32 + the DLL's `enumerate_modules` reports the EXE first;
        // see `crates/batman-lod-dll/src/pe.rs::enumerate_modules`.
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
                "ipc read_bytes: short response ({}/{})",
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

    fn uobject_class_offset(&self) -> usize {
        // The injected DLL measures `UObjectBase::ClassPrivate` in the live
        // process and reports it in the Welcome handshake. Prefer that over
        // the trait's `0x10` default; fall back only if the DLL left it unset.
        let v = self.inner.welcome.offsets.uobject_class_private as usize;
        if v == 0 { 0x10 } else { v }
    }

    fn scan_module(&self, name: &str, pattern: &Pattern) -> CoreResult<Option<usize>> {
        let (bytes, mask) = pattern.wire();
        let wire = openforge_ue5_protocol::PatternWire { bytes, mask };
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
        let wire = openforge_ue5_protocol::PatternWire { bytes, mask };
        let v = self
            .inner
            .client
            .lock()
            .scan_module_all(name, wire)
            .map_err(host_err)?;
        Ok(v.into_iter().map(|a| a as usize).collect())
    }

    fn scan_heap_for_u64(&self, needle: u64, alignment: usize) -> CoreResult<Vec<usize>> {
        self.scan_heap_for_u64_labeled(needle, alignment, "")
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

    fn patch_code(&self, addr: usize, original: &[u8], replacement: &[u8]) -> CoreResult<Vec<u8>> {
        self.inner
            .client
            .lock()
            .code_patch(addr as u64, original.to_vec(), replacement.to_vec())
            .map_err(host_err)
    }

    fn restore_code(&self, addr: usize, original: &[u8]) -> CoreResult<()> {
        self.inner
            .client
            .lock()
            .restore_patch(addr as u64, original.to_vec())
            .map_err(host_err)
    }

    fn find_uobject(
        &self,
        class_path: &str,
        predicate: &Predicate,
    ) -> CoreResult<Option<(u64, u64)>> {
        let wire_pred = predicate_to_wire(predicate);
        self.inner
            .client
            .lock()
            .find_uobject(class_path, wire_pred)
            .map_err(host_err)
    }

    fn find_all_uobjects(
        &self,
        class_path: &str,
        predicate: &Predicate,
        max_results: u32,
    ) -> CoreResult<(Vec<openforge_core::FoundObject>, bool)> {
        let wire_pred = predicate_to_wire(predicate);
        let (matches, truncated) = self
            .inner
            .client
            .lock()
            .find_all_uobjects(class_path, wire_pred, max_results)
            .map_err(host_err)?;
        let mapped = matches
            .into_iter()
            .map(|m| openforge_core::FoundObject {
                obj_addr: m.obj_addr,
                class_addr: m.class_addr,
                fqn: m.fqn,
            })
            .collect();
        Ok((mapped, truncated))
    }

    fn find_by_class_substring(
        &self,
        class_substrings: &[String],
        max_results: u32,
    ) -> CoreResult<(Vec<openforge_core::FoundObject>, bool)> {
        // Backed by the cached walk_objects() snapshot. First call per
        // session pays a single IPC round-trip; subsequent calls are
        // in-memory filtering against an Arc<Vec>.
        let objs = self.walk_objects().map_err(host_err)?;
        let needles: Vec<String> = class_substrings
            .iter()
            .map(|s| s.to_ascii_uppercase())
            .collect();
        let mut out: Vec<openforge_core::FoundObject> = Vec::new();
        let mut truncated = false;
        for obj in objs.iter() {
            if obj.class_ptr == 0 {
                continue;
            }
            let cn_upper = obj.class_name.to_ascii_uppercase();
            if !needles.iter().any(|n| cn_upper.contains(n)) {
                continue;
            }
            if out.len() >= max_results as usize {
                truncated = true;
                break;
            }
            out.push(openforge_core::FoundObject {
                obj_addr: obj.addr,
                class_addr: obj.class_ptr,
                fqn: obj.fqn.clone(),
            });
        }
        Ok((out, truncated))
    }

    fn call_ufunction(
        &self,
        obj_addr: u64,
        class_addr: u64,
        function_name: &str,
        params: Vec<u8>,
    ) -> CoreResult<Option<Vec<u8>>> {
        self.inner
            .client
            .lock()
            .call_ufunction(obj_addr, class_addr, function_name, params)
            .map_err(host_err)
    }

    fn resolve_property(
        &self,
        class_addr: u64,
        property_name: &str,
    ) -> CoreResult<Option<ResolvedField>> {
        // Session-level cache: re-derive the key the same way Ue5Session's
        // public `resolve_property` helper does so both entry points share
        // one cache entry per (class, name).
        let key = (class_addr, property_name.to_ascii_lowercase());
        if let Some(cached) = self.inner.resolved_props.lock().get(&key).cloned() {
            return Ok(Some(wire_to_core(cached)));
        }
        let resolved = self
            .inner
            .client
            .lock()
            .resolve_property(class_addr, property_name)
            .map_err(host_err)?;
        if let Some(ref r) = resolved {
            self.inner.resolved_props.lock().insert(key, r.clone());
        }
        Ok(resolved.map(wire_to_core))
    }
}

fn predicate_to_wire(p: &Predicate) -> openforge_ue5_protocol::NamePredicate {
    use openforge_ue5_protocol::NamePredicate as W;
    match p {
        Predicate::Any => W::Any,
        Predicate::Exact(s) => W::Exact(s.clone()),
        Predicate::Contains(s) => W::Contains(s.clone()),
        Predicate::FqnPrefix(s) => W::FqnPrefix(s.clone()),
    }
}

fn wire_to_core(r: openforge_ue5_protocol::ResolvedProperty) -> ResolvedField {
    ResolvedField {
        offset: r.offset,
        size: r.size,
        kind: wire_kind_to_core(r.kind),
        defined_in_class: r.defined_in_class,
    }
}

fn wire_kind_to_core(k: openforge_ue5_protocol::PropKind) -> FieldKind {
    use openforge_ue5_protocol::PropKind as P;
    match k {
        P::I8 => FieldKind::I8,
        P::I16 => FieldKind::I16,
        P::I32 => FieldKind::I32,
        P::I64 => FieldKind::I64,
        P::U8 => FieldKind::U8,
        P::U16 => FieldKind::U16,
        P::U32 => FieldKind::U32,
        P::U64 => FieldKind::U64,
        P::F32 => FieldKind::F32,
        P::F64 => FieldKind::F64,
        P::Bool => FieldKind::Bool,
        P::Bytes(n) => FieldKind::Bytes(n),
    }
}

/// Bridge `HostError` (this crate) into `openforge_core::Error::Custom`. The
/// `Ctx` trait signature requires `openforge_core::Error`; keeping the
/// translation in one place makes the conversion semantics explicit.
fn host_err(e: HostError) -> CoreError {
    CoreError::Custom(format!("ue5-host ipc: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that `Ue5Session: Send + Sync` (and won't regress
    /// silently from a future field addition).
    #[test]
    fn session_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Ue5Session>();
    }
}
