//! The bridge between the in-DLL Lua VM and the per-game reflection engine.
//!
//! Each per-game DLL (`openforge-batman-lod-dll`, future siblings) owns a
//! reflection engine that knows how to walk `GUObjectArray`, resolve
//! `FProperty` offsets, and dispatch `ProcessEvent`. The Lua runtime in this
//! crate has no business knowing how any of that is implemented — it only
//! needs a *handle* it can call from the worker thread.
//!
//! [`LuaEngineHost`] is that handle. The DLL constructs an `Arc<dyn
//! LuaEngineHost>` once and hands it to [`crate::LuaRuntime::new`]. From
//! that point on the Lua bindings call through the trait for every UE5
//! interaction, never touching engine internals directly.
//!
//! ## Threading
//!
//! The Lua worker, the delayed-callback timer, and the keybind poll thread
//! all live inside this crate. Any of them may invoke a host method, so
//! implementations MUST be `Send + Sync` and safe to call from arbitrary
//! threads. In practice the per-game DLL serializes engine access on its
//! own reflection mutex; the lua runtime adds no additional locking.
//!
//! ## Error model
//!
//! Every fallible method returns `Result<_, String>`. Strings are surfaced
//! to user scripts as `mlua::Error::external` so they can be caught with
//! `pcall`. Implementations should wrap their engine calls in
//! `std::panic::catch_unwind` / SEH; a panic that escapes into mlua will
//! poison the VM and abort the worker thread.

use openforge_ue5_protocol::{NamePredicate, PropKind, PropValue, ResolvedProperty};

/// Game-thread bridge implemented by each per-game DLL.
///
/// See the module docstring for threading + error-model requirements.
pub trait LuaEngineHost: Send + Sync + 'static {
    /// Locate a single live UObject by class name + predicate.
    ///
    /// Returns `Ok(Some((obj_addr, class_addr)))` on a hit, `Ok(None)` when
    /// no live instance matches (typical at main-menu time when gameplay
    /// objects have not been spawned). Errors are reserved for engine-level
    /// failures — a missing object is not an error.
    fn find_uobject(
        &self,
        class_path: &str,
        predicate: &NamePredicate,
    ) -> Result<Option<(u64, u64)>, String>;

    /// Locate every live UObject matching `class_path` + `predicate`, up to
    /// `max_results`. Used by `FindAllOf(class)` to power iterate-and-act
    /// scripts. An empty `Vec` is a legitimate "nothing loaded yet"
    /// response, not an error.
    fn find_all_uobjects(
        &self,
        class_path: &str,
        predicate: &NamePredicate,
        max_results: usize,
    ) -> Result<Vec<FoundObject>, String>;

    /// Resolve an FProperty by name on the given UClass (walking supers).
    /// Returns `Ok(None)` if the property does not exist on the class
    /// chain — Lua scripts typically translate that into a polite warning
    /// rather than a hard error.
    fn resolve_property(
        &self,
        class_addr: u64,
        name: &str,
    ) -> Result<Option<ResolvedProperty>, String>;

    /// Read raw memory at `addr` interpreted as `kind`.
    fn read_property(&self, addr: u64, kind: PropKind) -> Result<PropValue, String>;

    /// Write `value` to `addr`. The caller is responsible for ensuring
    /// `value.kind()` matches the FProperty's actual width.
    fn write_property(&self, addr: u64, value: PropValue) -> Result<(), String>;

    /// Invoke a UFunction on `obj_addr` by name. `class_addr` is the
    /// object's `UClass*` (used to locate the UFunction via the cached
    /// function chain). `params` is the raw byte image of the function's
    /// parameter struct — see [`openforge_ue5_protocol::Request::CallUFunction`]
    /// for the encoding contract.
    fn call_ufunction(
        &self,
        obj_addr: u64,
        class_addr: u64,
        fn_name: &str,
        params: Vec<u8>,
    ) -> Result<Vec<u8>, String>;

    /// Build the fully-qualified name of an object (walks the outer chain).
    /// Backs `obj:GetFullName()` in scripts.
    fn full_name_of(&self, obj_addr: u64) -> Result<String, String>;

    /// Return just the class-name segment (the last component of the
    /// object's class FQN, e.g. `"ULegoPlayerState"`). Backs
    /// `obj:type()` and `obj:GetClass():GetFName():ToString()`.
    fn class_name_of(&self, obj_addr: u64) -> Result<String, String>;

    /// List every UFunction defined on `class_addr` (or any super in the
    /// chain). Used by the metatable generator to enumerate callable
    /// methods when a Lua script first touches a UObjectHandle of this
    /// class. Cached per-class by the generator — implementations may
    /// also cache internally if the walk is expensive.
    fn list_class_functions(&self, class_addr: u64) -> Result<Vec<UFunctionSig>, String>;

    /// Resolve the parameter layout of a single UFunction.
    /// `ufunction_addr` comes from [`UFunctionSig::addr`] (returned by
    /// [`Self::list_class_functions`]). Used at call-time to marshal Lua
    /// args into the param buffer + unmarshal OUT/RETURN slots back to
    /// Lua values.
    fn ufunction_params(&self, ufunction_addr: u64) -> Result<Vec<UFunctionParam>, String>;
}

/// Wire-thin UFunction listing returned by
/// [`LuaEngineHost::list_class_functions`].
#[derive(Debug, Clone)]
pub struct UFunctionSig {
    pub name: String,
    /// UFunction* in game memory — pass to
    /// [`LuaEngineHost::ufunction_params`] and
    /// [`LuaEngineHost::call_ufunction`] (as part of the class lookup).
    pub addr: u64,
}

/// One FProperty on a UFunction's parameter chain.
#[derive(Debug, Clone)]
pub struct UFunctionParam {
    pub name: String,
    /// Byte offset within the param buffer.
    pub offset: u32,
    /// Width in bytes (used to size the buffer + the `Bytes(n)` case).
    pub size: u32,
    /// Wire-level type. Re-uses [`PropKind`] from the protocol crate.
    pub kind: PropKind,
    /// Reflection-layer flags: is this an OUT parameter (caller expects
    /// to read it back post-call), a RETURN parameter (the trailing slot
    /// UE5 reserves for the return value), or a plain IN parameter?
    /// Drives how the metatable generator surfaces results to Lua.
    pub flags: UFunctionParamFlags,
}

/// Subset of UE5's `EPropertyFlags` the metatable generator cares about.
/// All other flag bits (Edit, Net, Replicated, etc.) are intentionally
/// dropped — they have no bearing on argument marshalling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UFunctionParamFlags {
    /// `CPF_OutParm` (0x100): the function writes back to this slot.
    /// Surface to Lua as an extra return value.
    pub out: bool,
    /// `CPF_ReturnParm` (0x400): the dedicated return slot. Always the
    /// last entry on a UFunction's param chain when present.
    pub returns: bool,
    /// `CPF_ReferenceParm` (0x80000000): pass-by-reference. Rare for v2
    /// since we don't yet marshal struct args; surfaced for completeness.
    pub by_ref: bool,
}

/// One match returned by [`LuaEngineHost::find_all_uobjects`]. Mirrors the
/// wire [`openforge_ue5_protocol::FoundObjectEntry`] but lives in the
/// crate's own namespace so the trait is decoupled from the wire types' Serde
/// derivations (this crate is `no-serde`).
#[derive(Debug, Clone)]
pub struct FoundObject {
    /// Live UObject pointer in the game's address space.
    pub obj_addr: u64,
    /// The object's `UClass*` — required for subsequent
    /// [`LuaEngineHost::resolve_property`] / [`LuaEngineHost::call_ufunction`]
    /// calls.
    pub class_addr: u64,
    /// Fully-qualified name, precomputed by the engine. Scripts use this
    /// for filtering before deciding whether to act on the object.
    pub fqn: String,
}
