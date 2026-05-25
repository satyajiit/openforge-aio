//! `LuaEngineHost` impl that bridges `openforge-ue5-lua` bindings into this
//! DLL's reflection engine.
//!
//! Lifetime: one `EngineHost` per pipe client (stored on `ConnState`),
//! holding an `Arc<UeEngine>` clone. Engine work routes through the existing
//! `ops::reflection` primitives plus the FName / FQN helpers on `UeEngine`
//! itself, so the Lua VM sees exactly the same view of the game world as a
//! protocol-driven `FindUObject` / `ReadProperty` call from the host.
//!
//! ## Why every method wraps `catch_unwind`
//!
//! User scripts are untrusted. A bad property write on a stale pointer can
//! tear the engine's invariants; a wild address fed to `obj:CallFunction`
//! can hit a guard page. Both classes of fault could panic somewhere deep
//! in our reflection stack, and a panic unwinding into mlua corrupts the VM
//! and abort the worker thread. We translate every panic into an `Err`
//! string instead, which mlua surfaces as a regular `error()` the script
//! can `pcall`.
//!
//! `catch_unwind` does **not** catch Windows structured exceptions (access
//! violations). FName decode goes through the existing SEH shim in
//! `crate::seh`; reads + writes go through `LocalReader`, which uses
//! `ReadProcessMemory` / `WriteProcessMemory` (documented to fault-isolate).
//! `CallUFunction` still does a raw FFI call into engine code — a wild
//! pointer there CAN take the game down. Future work: add an SEH guard for
//! the `ProcessEvent` call specifically.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use openforge_ue5_lua::{FoundObject, LuaEngineHost};
use openforge_ue5_protocol::{
    NamePredicate, PropKind, PropValue, ResolvedProperty,
};

use crate::engine::UeEngine;
use crate::ops::reflection;

/// Concrete host the per-game DLL passes to `LuaRuntime::new`.
///
/// Constructed once per pipe client connection. Cheap to clone (the inner
/// engine is reference-counted), so the runtime stores its own `Arc` clone.
pub struct EngineHost {
    engine: Arc<UeEngine>,
}

impl EngineHost {
    pub fn new(engine: Arc<UeEngine>) -> Self {
        Self { engine }
    }
}

/// Helper: run `f` under `catch_unwind`, translating any panic to a stable
/// error string. The closure captures `&self` by reference (Rust can't
/// prove the captured borrows are unwind-safe without help), so we wrap in
/// `AssertUnwindSafe`. This is sound because:
///   * The engine's internal caches are `Mutex`-protected — a panicked
///     thread won't leave them poisoned in `parking_lot`'s case (it has no
///     poisoning).
///   * The reflection ops are pure functions over `&UeEngine`; they have
///     no state to corrupt.
fn guarded<T>(label: &'static str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => Err(format!("`{label}` panicked (likely stale pointer)")),
    }
}

impl LuaEngineHost for EngineHost {
    fn find_uobject(
        &self,
        class_path: &str,
        predicate: &NamePredicate,
    ) -> Result<Option<(u64, u64)>, String> {
        guarded("find_uobject", || {
            Ok(reflection::find_uobject(&self.engine, class_path, predicate))
        })
    }

    fn find_all_uobjects(
        &self,
        class_path: &str,
        predicate: &NamePredicate,
        max_results: usize,
    ) -> Result<Vec<FoundObject>, String> {
        guarded("find_all_uobjects", || {
            let (matches, _truncated) =
                reflection::find_all_uobjects(&self.engine, class_path, predicate, max_results);
            Ok(matches
                .into_iter()
                .map(|m| FoundObject {
                    obj_addr: m.obj_addr,
                    class_addr: m.class_addr,
                    fqn: m.fqn,
                })
                .collect())
        })
    }

    fn resolve_property(
        &self,
        class_addr: u64,
        name: &str,
    ) -> Result<Option<ResolvedProperty>, String> {
        guarded("resolve_property", || {
            Ok(reflection::resolve_property(&self.engine, class_addr, name))
        })
    }

    fn read_property(&self, addr: u64, kind: PropKind) -> Result<PropValue, String> {
        guarded("read_property", || {
            let size = kind.size() as usize;
            if size == 0 {
                return Err("zero-sized read".into());
            }
            let mut buf = vec![0u8; size];
            self.engine
                .reader
                .read_bytes(addr as usize, &mut buf)
                .map_err(|_| format!("read failed at 0x{addr:X}"))?;
            Ok(decode_value(kind, &buf))
        })
    }

    fn write_property(&self, addr: u64, value: PropValue) -> Result<(), String> {
        guarded("write_property", || {
            let bytes = encode_value(value);
            if bytes.is_empty() {
                return Err("empty write".into());
            }
            self.engine
                .reader
                .write_bytes(addr as usize, &bytes)
                .map_err(|_| format!("write failed at 0x{addr:X}"))
        })
    }

    fn call_ufunction(
        &self,
        obj_addr: u64,
        class_addr: u64,
        fn_name: &str,
        params: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        guarded("call_ufunction", || {
            reflection::call_ufunction(&self.engine, obj_addr, class_addr, fn_name, &params)
        })
    }

    fn full_name_of(&self, obj_addr: u64) -> Result<String, String> {
        guarded("full_name_of", || {
            if obj_addr == 0 {
                return Err("null obj_addr".into());
            }
            Ok(self.engine.build_fqn(obj_addr as usize))
        })
    }

    fn class_name_of(&self, obj_addr: u64) -> Result<String, String> {
        guarded("class_name_of", || {
            if obj_addr == 0 {
                return Err("null obj_addr".into());
            }
            let class_ptr = self
                .engine
                .get_object_class(obj_addr as usize)
                .map_err(|_| format!("read class ptr failed at 0x{obj_addr:X}"))?;
            self.engine
                .get_object_name(class_ptr)
                .map_err(|e| format!("decode class name failed: {e:?}"))
        })
    }
}

fn decode_value(kind: PropKind, buf: &[u8]) -> PropValue {
    match kind {
        PropKind::I8 => PropValue::I8(i8::from_le_bytes(buf[..1].try_into().unwrap())),
        PropKind::U8 => PropValue::U8(buf[0]),
        PropKind::Bool => PropValue::Bool(buf[0] != 0),
        PropKind::I16 => PropValue::I16(i16::from_le_bytes(buf[..2].try_into().unwrap())),
        PropKind::U16 => PropValue::U16(u16::from_le_bytes(buf[..2].try_into().unwrap())),
        PropKind::I32 => PropValue::I32(i32::from_le_bytes(buf[..4].try_into().unwrap())),
        PropKind::U32 => PropValue::U32(u32::from_le_bytes(buf[..4].try_into().unwrap())),
        PropKind::F32 => PropValue::F32(f32::from_le_bytes(buf[..4].try_into().unwrap())),
        PropKind::I64 => PropValue::I64(i64::from_le_bytes(buf[..8].try_into().unwrap())),
        PropKind::U64 => PropValue::U64(u64::from_le_bytes(buf[..8].try_into().unwrap())),
        PropKind::F64 => PropValue::F64(f64::from_le_bytes(buf[..8].try_into().unwrap())),
        PropKind::Bytes(_) => PropValue::Bytes(buf.to_vec()),
    }
}

fn encode_value(value: PropValue) -> Vec<u8> {
    match value {
        PropValue::I8(v) => v.to_le_bytes().to_vec(),
        PropValue::U8(v) => v.to_le_bytes().to_vec(),
        PropValue::Bool(v) => vec![v as u8],
        PropValue::I16(v) => v.to_le_bytes().to_vec(),
        PropValue::U16(v) => v.to_le_bytes().to_vec(),
        PropValue::I32(v) => v.to_le_bytes().to_vec(),
        PropValue::U32(v) => v.to_le_bytes().to_vec(),
        PropValue::F32(v) => v.to_le_bytes().to_vec(),
        PropValue::I64(v) => v.to_le_bytes().to_vec(),
        PropValue::U64(v) => v.to_le_bytes().to_vec(),
        PropValue::F64(v) => v.to_le_bytes().to_vec(),
        PropValue::Bytes(b) => b,
    }
}
