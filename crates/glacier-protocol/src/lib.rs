//! Wire protocol shared between the per-game Glacier DLL (loaded into a
//! Glacier 2 game process — today `openforge-glacier-007-dll`) and
//! [`openforge-glacier-host`] (injector + client in the trainer process).
//!
//! This is the Glacier analogue of [`openforge-ue5-protocol`]. The framing,
//! handshake discipline, and stability rules are identical; only the domain
//! payloads differ — Glacier exposes a `ZTypeRegistry` reflection model
//! (`IType` / `STypeID` / `SNamedPropertyInfo` for static types and
//! `SPropertyData` for live entity instances) rather than UE5's
//! `UObject` / `FProperty` graph. The structural decode is implemented once in
//! `openforge-glacier-host` (`GlacierReflection`, over a `Ctx`), and the DLL
//! runs that same engine *in-process* against the game's own address space.
//!
//! ## Framing
//!
//! Every message is a `u32` little-endian length prefix followed by a
//! `postcard`-encoded payload. Server pipes are message-mode, but framing is
//! length-prefixed so a byte-mode fallback still works. Max payload is
//! [`MAX_FRAME_BYTES`].
//!
//! ## Connection model
//!
//! - Pipe name: `\\.\pipe\openforge-glacier-<pid>` (see [`pipe_name_for_pid`]).
//! - Bidirectional, one client at a time. The DLL re-opens after each client
//!   disconnect so the trainer can reattach without restarting the game.
//! - First exchange MUST be [`Request::Hello`] / [`Response::Welcome`]. The
//!   server rejects any other message before the handshake; a version mismatch
//!   makes the server close the pipe with [`Response::Error`].
//!
//! ## Stability
//!
//! Bump [`PROTOCOL_VERSION`] for any wire-incompatible change. Adding a
//! `Request`/`Response` variant *is* wire-breaking — `postcard` keys variants
//! by ordinal, so older peers reject the new ordinals with a decode error.
//! Only ever append variants, and bump the version when you do.

#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Wire-protocol version.
///
/// History:
/// * `1` — initial Glacier surface: handshake; single-channel memory ops
///   (`ReadBytes` / `WriteBytes` / `EnumModules` / `ScanModule[All]` /
///   `ScanHeapForU64`); reflection reads (`ResolveType`,
///   `EnumerateTypeProperties`, `InstanceProperties`,
///   `ResolveInstanceProperty`); and a guarded raw property write
///   (`SetProperty`).
/// * `2` — in-process live entity discovery: `FindEntitiesWithProperty`
///   (`Response::Entities`). Heap-scans for `ZEntityImpl`-shaped objects whose
///   per-instance `SPropertyData` array carries a named property — the anchor
///   features need, since live instances don't back-reference the reflection
///   `IType`.
/// * `3` — engine-call actuation: `FireNode` (configure a logic node's inputs,
///   then signal an input pin / `Activate`), `SetPropertyEngine` (the engine's
///   own setter for `E_HAS_GETTER_SETTER` properties a raw write can't touch),
///   `RegisterFreeze` / `UnregisterFreeze` (per-frame value re-stamp),
///   `FindInstancesOfType` (type-keyed instance discovery), and a low-level
///   `GameThreadCall` escape hatch. These route through an AOB-resolved engine
///   function called from a safe execution context (worker thread, else a
///   writable-pointer frame anchor — never a `.text` detour, which Denuvo's
///   anti-tamper would flag). Pin ids are IOI's proprietary signed-i32 hash and
///   travel the wire as a raw [`NodeFire::SignalInputPin`] `u32` — the DLL never
///   hashes a pin name.
/// * `4` — dynamic guarded freeze: `StartFreeze` / `StopFreeze` /
///   `QueryFreezeStats` (`Response::FreezeStarted` / `FreezeStopped` /
///   `FreezeStats`). Unlike v3's `RegisterFreeze` (re-stamps a constant via the
///   pipe drain), `StartFreeze` runs a DLL-side per-frame thread, independent of
///   the pipe, that either copies a sibling field (`source_offset`, e.g.
///   current := max for difficulty-agnostic god mode) or stamps a constant —
///   each write gated by a read-before-write plausibility check that SKIPS a
///   freed/reused box instead of corrupting it. `RegisterFreeze` /
///   `UnregisterFreeze` are left as-is (stubs), not repurposed.
/// * `5` — in-process writer discovery: `FindWriter` (`Response::WriterFound`).
///   Sets an x86-64 hardware write-watchpoint (DR0/DR7) on a target address
///   across the game's own threads from INSIDE the process and catches the hit
///   with a vectored exception handler — no `DebugActiveProcess`, so Denuvo's
///   anti-debug never sees a debugger attach. Returns the writing instruction's
///   RIP, a byte window around it, and the integer register file at the trap
///   (so the host can recover the master pointer the write came from). The
///   Denuvo-safe analogue of the discover pipeline's external `watch-write`;
///   it's how we locate the ammo-decrement / recoil-write to code-patch.
/// * `6` — code patching: `CodePatch` / `RestorePatch` (`Response::WriteOk`).
///   The DLL does the `.text`-overwrite dance in-process (VirtualProtect→RWX,
///   verify-original, write, restore-protect, FlushInstructionCache) and tracks
///   applied patches per connection, auto-restoring every one when the client
///   disconnects — so detach reverts code patches like the god-mode freeze.
///   `GlacierSession` overrides `Ctx::patch_code`/`restore_code` to route here
///   (the default verify-and-write can't touch RX `.text`). Lands the
///   FindWriter-discovered NOPs (No Reload / Infinite Ammo / No Recoil).
pub const PROTOCOL_VERSION: u32 = 6;

/// Maximum size of a single framed message, in bytes. Guards both peers from a
/// malformed length prefix that would otherwise allocate gigabytes. A full
/// `EnumModules` or a wide `ScanHeapForU64` result stays well under this; cap
/// at 256 MiB to leave headroom for large `ReadBytes` dumps.
pub const MAX_FRAME_BYTES: u32 = 256 * 1024 * 1024;

/// Build the named-pipe path the DLL listens on for a given game pid.
///
/// Format: `\\.\pipe\openforge-glacier-<pid>`. The `glacier` infix keeps it
/// disjoint from the UE5 stack's `openforge-ue5-<pid>` namespace, so both
/// backends can run side by side without colliding.
pub fn pipe_name_for_pid(pid: u32) -> String {
    alloc::format!(r"\\.\pipe\openforge-glacier-{pid}")
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// One loaded module in the game process, as reported by
/// [`Request::EnumModules`]. Mirrors the UE5 stack's `ModuleEntry`: lets the
/// host resolve module bases / `.text` spans without its own toolhelp32 walk —
/// every module fact comes from inside the game's address space via the DLL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEntry {
    /// File-name only (e.g. `"007FirstLight.exe"`). Matched case-insensitively
    /// on the host side.
    pub name: String,
    /// Module load base in the game's address space.
    pub base: u64,
    /// `MODULEENTRY32W.modBaseSize` (the loader's reported image size).
    pub size: u64,
    /// RVA of the `.text` section (`0` if the PE has no parseable `.text`).
    pub text_offset: u64,
    /// Size of the `.text` section in bytes.
    pub text_size: u64,
}

/// Wire-friendly AOB pattern. `bytes[i]` is a literal iff `mask[i] == 0xFF`;
/// `mask[i] == 0x00` marks `bytes[i]` as a wildcard. The two slices MUST be
/// equal length and contain at least one literal byte.
///
/// The host builds this from an `openforge_core::Pattern`; the DLL's scan op
/// reconstructs a matcher from `(bytes, mask)` directly without re-parsing the
/// human-readable string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternWire {
    pub bytes: Vec<u8>,
    pub mask: Vec<u8>,
}

/// Logging level requested by the host for the DLL's internal trace ring.
/// The DLL ships its log lines back in [`Response::LogLines`].
///
/// Variant order is the postcard discriminant; only ever append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// A resolved Glacier reflection type — wire mirror of
/// `openforge_glacier_host::TypeInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlacierType {
    pub name: String,
    /// VA of the (static) `IType` record.
    pub itype_va: u64,
    /// VA of the `STypeID` handle.
    pub stypeid_va: u64,
    /// VA of the type-name string the resolver matched.
    pub name_string_va: u64,
    pub flags: u16,
    pub size: u16,
    pub align: u8,
    /// `true` if resolved via the static `IType.pszTypeName` slot (module
    /// data); `false` via a registry hash-map node (heap).
    pub via_static: bool,
}

/// One property on a static (class) type — wire mirror of
/// `openforge_glacier_host::PropertyInfo`, decoded from the `IClassType`
/// descriptor array. `crc32` (the CRC32 of `name`) is the stable identity used
/// to match the per-instance [`GlacierField`] at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlacierTypeProp {
    pub index: u32,
    pub name: String,
    pub crc32: u32,
    pub flags: u32,
    pub type_name: Option<String>,
}

/// A property resolved against one live entity's `SPropertyData` array — wire
/// mirror of `openforge_glacier_host::ResolvedGlacierField`.
///
/// `offset` is the per-instance byte offset (`m_nPropertyOffset`); the live
/// value lives at `obj_base + offset` where `obj_base == entity_va + 8` (see
/// [`Response::InstanceProps::obj_base`]). `has_getter_setter` gates raw
/// writes: when set, the engine's setter is canonical and a raw write is
/// refused by [`Request::SetProperty`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlacierField {
    /// Property name from the descriptor (`SNamedPropertyInfo`). `None` if the
    /// descriptor pointer was unreadable.
    pub name: Option<String>,
    /// CRC32 of the property name (the lookup key).
    pub crc32: u32,
    /// `SPropertyData.m_nPropertyOffset` — per-instance byte offset, added to
    /// the entity's `obj_base`, not to `entity_va`.
    pub offset: i64,
    /// `SPropertyData.m_nPropertyFlags` (per-instance flags).
    pub sprop_flags: u32,
    /// `SPropertyInfo.m_Flags` from the descriptor. `None` if unreadable.
    pub descriptor_flags: Option<u32>,
    /// `true` iff `descriptor_flags & E_HAS_GETTER_SETTER`.
    pub has_getter_setter: bool,
    /// Best-effort property type name (decoded from the descriptor's STypeID).
    pub type_name: Option<String>,
}

/// Width specifier for a typed value crossing the wire. Mirrors UE5's
/// `PropKind`. Variant order is the postcard discriminant; only ever append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueKind {
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
    /// `n` raw bytes — for widths the typed variants don't cover.
    Bytes(u32),
}

impl ValueKind {
    /// Bytes on the wire / in memory.
    pub fn size(self) -> u32 {
        match self {
            ValueKind::I8 | ValueKind::U8 | ValueKind::Bool => 1,
            ValueKind::I16 | ValueKind::U16 => 2,
            ValueKind::I32 | ValueKind::U32 | ValueKind::F32 => 4,
            ValueKind::I64 | ValueKind::U64 | ValueKind::F64 => 8,
            ValueKind::Bytes(n) => n,
        }
    }
}

/// Typed value carried by [`Request::SetProperty`]. Variant order is the
/// postcard discriminant; only ever append.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GlacierValue {
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Bytes(Vec<u8>),
}

impl GlacierValue {
    pub fn kind(&self) -> ValueKind {
        match self {
            GlacierValue::I8(_) => ValueKind::I8,
            GlacierValue::I16(_) => ValueKind::I16,
            GlacierValue::I32(_) => ValueKind::I32,
            GlacierValue::I64(_) => ValueKind::I64,
            GlacierValue::U8(_) => ValueKind::U8,
            GlacierValue::U16(_) => ValueKind::U16,
            GlacierValue::U32(_) => ValueKind::U32,
            GlacierValue::U64(_) => ValueKind::U64,
            GlacierValue::F32(_) => ValueKind::F32,
            GlacierValue::F64(_) => ValueKind::F64,
            GlacierValue::Bool(_) => ValueKind::Bool,
            GlacierValue::Bytes(v) => ValueKind::Bytes(v.len() as u32),
        }
    }

    /// Little-endian memory image of the value — exactly the bytes a raw write
    /// stamps at the resolved property address.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        match self {
            GlacierValue::I8(v) => v.to_le_bytes().to_vec(),
            GlacierValue::I16(v) => v.to_le_bytes().to_vec(),
            GlacierValue::I32(v) => v.to_le_bytes().to_vec(),
            GlacierValue::I64(v) => v.to_le_bytes().to_vec(),
            GlacierValue::U8(v) => v.to_le_bytes().to_vec(),
            GlacierValue::U16(v) => v.to_le_bytes().to_vec(),
            GlacierValue::U32(v) => v.to_le_bytes().to_vec(),
            GlacierValue::U64(v) => v.to_le_bytes().to_vec(),
            GlacierValue::F32(v) => v.to_le_bytes().to_vec(),
            GlacierValue::F64(v) => v.to_le_bytes().to_vec(),
            GlacierValue::Bool(v) => alloc::vec![u8::from(*v)],
            GlacierValue::Bytes(v) => v.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Actuation types (protocol v3)
// ---------------------------------------------------------------------------

/// One input to configure on a logic node before firing it. `property` is the
/// node's input property name (CRC32-matched against its `SPropertyData`, same
/// as every other reflection lookup); `value` is stamped via the engine setter
/// when the property is getter/setter, else raw-written, before the pin fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInput {
    pub property: String,
    pub value: GlacierValue,
}

/// How to actuate a configured logic node.
///
/// `SignalInputPin` carries the **raw IOI pin id** — Glacier 2 pin ids are a
/// proprietary signed-i32 hash, *not* the CRC32 used for property names, so the
/// host supplies the literal id (e.g. `Activate == 0x4F1066FB`) and the DLL
/// passes it straight to the engine. `Activate` is the common-case alias for
/// that literal so callers needn't repeat it.
///
/// Variant order is the postcard discriminant; only ever append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeFire {
    /// Signal a specific input pin by its raw engine pin id.
    SignalInputPin(u32),
    /// Signal the node's `Activate` pin (the `0x4F1066FB` literal).
    Activate,
}

/// Opaque handle for a registered freeze, returned by
/// [`Request::RegisterFreeze`] and passed back to [`Request::UnregisterFreeze`].
pub type FreezeHandle = u32;

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Every message the host can send.
///
/// **Adding a variant is a wire-breaking change** — `postcard` keys variants
/// by ordinal. Bump [`PROTOCOL_VERSION`] and only append.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// First message; the server replies with [`Response::Welcome`] and only
    /// then accepts other requests.
    Hello { client_version: u32 },
    /// Cheap liveness ping. Replies [`Response::Pong`].
    Ping,
    /// Ask the server to close this connection cleanly. The pipe-server thread
    /// re-opens for the next client; the DLL itself stays loaded.
    Disconnect,
    /// Set the DLL's runtime log level (affects [`Request::DrainLog`]).
    SetLogLevel(LogLevel),
    /// Drain the DLL's in-memory log ring (up to `max_lines`; `0` = all).
    DrainLog { max_lines: u32 },

    // -- Single-channel memory ops -----------------------------------------
    //
    // The host never touches game memory directly; every read / write / scan
    // crosses the pipe and the DLL executes it in-process via a fault-isolated
    // `ReadProcessMemory(GetCurrentProcess())` reader.
    /// Read `len` bytes at `addr`. Unreadable pages yield [`Response::Error`].
    ReadBytes { addr: u64, len: u32 },
    /// Write `bytes` at `addr`. Non-writable pages yield [`Response::Error`].
    WriteBytes { addr: u64, bytes: Vec<u8> },
    /// Enumerate every loaded module in the game process.
    EnumModules,
    /// Scan a module's `.text` section for the first match of `pattern`.
    /// `module_name` is case-insensitive; the empty string means the main
    /// module.
    ScanModule {
        module_name: String,
        pattern: PatternWire,
    },
    /// Like [`Request::ScanModule`] but returns every match (uniqueness
    /// probes).
    ScanModuleAll {
        module_name: String,
        pattern: PatternWire,
    },
    /// Walk every committed RW region and return aligned addresses where a
    /// `u64` equals `needle`. `label` is echoed into the DLL log for
    /// attribution (typically the feature id).
    ScanHeapForU64 {
        needle: u64,
        alignment: u32,
        label: String,
    },

    // -- Reflection reads ---------------------------------------------------
    //
    // Backed by `openforge_glacier_host::GlacierReflection` running against the
    // DLL's own address space.
    /// Resolve a reflection type by name (string → pointers → validated
    /// `IType`). Returns [`Response::ResolvedType`] or [`Response::NotFound`].
    ResolveType { name: String },
    /// Enumerate a class type's static properties (the `IClassType` descriptor
    /// array). Returns [`Response::TypeProperties`].
    EnumerateTypeProperties { name: String },
    /// Enumerate every per-instance property on a live entity (decoded from its
    /// `ZEntityType`'s `SPropertyData` array). Returns
    /// [`Response::InstanceProps`].
    InstanceProperties { entity_va: u64 },
    /// Resolve a single named property on a live entity to its per-instance
    /// offset (CRC32 match). Returns [`Response::ResolvedField`] or
    /// [`Response::NotFound`].
    ResolveInstanceProperty { entity_va: u64, name: String },

    // -- Guarded property write --------------------------------------------
    /// Resolve `property` on the live entity at `entity_va`, then stamp
    /// `value`'s little-endian image at `obj_base + offset`.
    ///
    /// Refuses (with [`Response::Error`]) when the property carries
    /// `E_HAS_GETTER_SETTER` — a raw write is not honored by the engine for
    /// those; they need the engine's own setter (a later, game-thread-routed
    /// op). Returns [`Response::NotFound`] when the property isn't present on
    /// the instance, or [`Response::WriteOk`] on success.
    SetProperty {
        entity_va: u64,
        property: String,
        value: GlacierValue,
    },

    // -- live entity discovery (protocol v2) -------------------------------
    /// Heap-scan the game's address space for live `ZEntityImpl`-shaped objects
    /// whose per-instance `SPropertyData` array carries the property named
    /// `property` (matched by CRC32). Returns up to `max_results` entity VAs in
    /// [`Response::Entities`].
    ///
    /// This is the anchor primitive for features: live instances do not
    /// back-reference the reflection `IType`, so a type lookup can't find them.
    /// The DLL filters candidates by a vtable-into-main-module check, then
    /// validates the entity chain — all in-process and fault-isolated.
    FindEntitiesWithProperty { property: String, max_results: u32 },

    // -- Engine-call actuation (protocol v3) -------------------------------
    //
    // These run an engine function against live state. They execute in a safe
    // context (the worker thread, or — if a node faults off-thread — a
    // writable-pointer frame anchor); never via a `.text` detour. Every call
    // is SEH-guarded in the DLL so a bad pointer yields `Error`, not a crash.
    /// Low-level escape hatch: call the engine function at `fn_va` with up to
    /// four integer/pointer arguments (MS x64 ABI: RCX, RDX, R8, R9). Returns
    /// [`Response::GameThreadCallResult`] with the raw RAX, or [`Response::Error`]
    /// on a structured exception. Used to reach engine entry points the typed
    /// ops don't model yet.
    GameThreadCall { fn_va: u64, args: Vec<u64> },
    /// Heap-scan for live instances of a reflection type by name (resolves the
    /// type's `STypeID`, then matches candidate entities' `m_pEntityType`).
    /// Returns up to `max_results` entity VAs in [`Response::Entities`]. The
    /// type-keyed analogue of [`Request::FindEntitiesWithProperty`], for nodes
    /// whose input property names are ambiguous across node types.
    FindInstancesOfType { type_name: String, max_results: u32 },
    /// Configure `inputs` on the logic node at `node_va`, then actuate it via
    /// `fire`. The DLL resolves each input property on the instance, writes it
    /// (engine setter for getter/setter props, raw stamp otherwise), then calls
    /// the AOB-resolved `SignalInputPin` with the raw pin id. Returns
    /// [`Response::WriteOk`] (the pin call returned), [`Response::NotFound`] (an
    /// input property isn't present on the instance), or [`Response::Error`]
    /// (resolve failure / SEH fault / unresolved engine fn).
    FireNode {
        node_va: u64,
        inputs: Vec<NodeInput>,
        fire: NodeFire,
    },
    /// Register a per-frame freeze: re-resolve `property` on the entity at
    /// `entity_va` every drain and stamp `value`. Refused (with
    /// [`Response::Error`]) for getter/setter properties. Returns
    /// [`Response::RegisteredFreeze`] with a handle for later removal.
    RegisterFreeze {
        entity_va: u64,
        property: String,
        value: GlacierValue,
    },
    /// Remove a previously registered freeze. Idempotent — an unknown handle
    /// still returns [`Response::WriteOk`].
    UnregisterFreeze { handle: FreezeHandle },
    /// Resolve `property` on the entity at `entity_va`, then set `value` via the
    /// engine's own `SetPropertyValue` (with change handlers invoked). This is
    /// the canonical path for `E_HAS_GETTER_SETTER` properties that
    /// [`Request::SetProperty`] refuses to raw-write. Returns
    /// [`Response::WriteOk`] / [`Response::NotFound`] / [`Response::Error`].
    SetPropertyEngine {
        entity_va: u64,
        property: String,
        value: GlacierValue,
    },

    // -- Dynamic guarded freeze (protocol v4) ------------------------------
    //
    // A DLL-side per-frame freeze thread, INDEPENDENT of the pipe (a client
    // disconnect must not stop an in-game freeze). Each tick it reads the live
    // f32 at `box_va + write_offset`; unless that value is finite and within
    // `[guard_min, guard_max]` it SKIPS the write, so a freed/reused box (the
    // resolved address went stale after a checkpoint reload) is left alone
    // rather than corrupted. On a passing guard it writes either the sibling
    // field at `box_va + source_offset` (difficulty-agnostic copy, e.g.
    // current := max) or, when `source_offset` is `None`, the constant `value`.
    /// Start a guarded per-frame freeze. Returns [`Response::FreezeStarted`].
    StartFreeze {
        /// Base VA the offsets are relative to (typically the resolved box).
        box_va: u64,
        /// Offset (from `box_va`) of the field to hold — the write target.
        write_offset: i64,
        /// When `Some`, copy `value_kind`-wide bytes from `box_va + this` into
        /// the write target each tick (e.g. current := max). When `None`, stamp
        /// `value` instead.
        source_offset: Option<i64>,
        /// Constant stamped when `source_offset` is `None` (ignored otherwise).
        value: GlacierValue,
        /// Width of the value copied/stamped.
        value_kind: ValueKind,
        /// Read-before-write guard: skip the tick unless the current f32 at the
        /// write target is finite and within `[guard_min, guard_max]`.
        guard_min: f32,
        guard_max: f32,
    },
    /// Stop a freeze started by [`Request::StartFreeze`]. Idempotent — an
    /// unknown/already-stopped handle still returns [`Response::FreezeStopped`].
    StopFreeze { handle: FreezeHandle },
    /// Query a running freeze's counters. Returns [`Response::FreezeStats`], or
    /// [`Response::Error`] for an unknown handle.
    QueryFreezeStats { handle: FreezeHandle },

    // -- in-process writer discovery (protocol v5) -------------------------
    //
    // Locate the instruction that writes a target address WITHOUT attaching a
    // debugger (Denuvo would flag `DebugActiveProcess`). The DLL arms an
    // x86-64 hardware write-watchpoint (DR0 + DR7) on the address across every
    // game thread via `SetThreadContext`, installs a vectored exception
    // handler, and waits for the single-step trap. On a hit it records the
    // faulting RIP + integer registers, then disarms DR0/DR7 on every thread.
    /// Watch `addr` for a write of `width` bytes (1/2/4/8) for up to
    /// `timeout_ms`. Returns [`Response::WriterFound`] on the first observed
    /// write, or [`Response::Error`] on timeout / arming failure.
    FindWriter {
        addr: u64,
        width: u8,
        timeout_ms: u32,
    },

    // -- code patching (protocol v6) ---------------------------------------
    //
    // In-process `.text` overwrite with VirtualProtect + FlushInstructionCache.
    // The DLL verifies the live bytes equal `original` before patching (a
    // mismatch means the game updated → refuse, don't corrupt), and registers
    // the patch so a client disconnect auto-restores it.
    /// Overwrite the bytes at `addr` with `patched` (must equal `original`
    /// first). Returns [`Response::WriteOk`] or [`Response::Error`].
    CodePatch {
        addr: u64,
        original: Vec<u8>,
        patched: Vec<u8>,
    },
    /// Restore `original` at `addr` and drop it from the auto-restore set.
    RestorePatch { addr: u64, original: Vec<u8> },
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// Every message the server can send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    /// Reply to [`Request::Hello`].
    Welcome {
        server_version: u32,
        pid: u32,
        /// Main module load base in the game's address space.
        module_base: u64,
        /// Main module image size.
        module_size: u64,
    },
    Pong,
    DisconnectAck,
    LogLines(Vec<String>),

    // -- Memory-op responses -----------------------------------------------
    /// Bytes read by [`Request::ReadBytes`].
    Bytes(Vec<u8>),
    /// [`Request::WriteBytes`] / [`Request::SetProperty`] succeeded.
    WriteOk,
    Modules(Vec<ModuleEntry>),
    /// Match addresses in the game's address space. Empty = no match.
    Matches(Vec<u64>),

    // -- Reflection responses ----------------------------------------------
    /// [`Request::ResolveType`] succeeded.
    ResolvedType(GlacierType),
    /// [`Request::EnumerateTypeProperties`] result (possibly empty).
    TypeProperties(Vec<GlacierTypeProp>),
    /// [`Request::InstanceProperties`] result. `obj_base` is the entity's
    /// `ZEntityRef` value base (`== entity_va + 8`); every field's live value
    /// lives at `obj_base + field.offset`.
    InstanceProps {
        entity_va: u64,
        obj_base: u64,
        fields: Vec<GlacierField>,
    },
    /// [`Request::ResolveInstanceProperty`] succeeded.
    ResolvedField(GlacierField),
    /// Lookup miss: type/property/instance not found. **Not** an error — the
    /// caller asked for something that isn't present (e.g. the entity doesn't
    /// carry that property, or the type name isn't in the registry).
    NotFound,

    /// Server-side error. The connection stays open unless the error is fatal
    /// (handshake failure, internal panic), in which case the DLL closes after
    /// sending this.
    Error(String),

    // -- live entity discovery (protocol v2) -------------------------------
    /// [`Request::FindEntitiesWithProperty`] result: live entity VAs (possibly
    /// empty). Each is a `ZEntityImpl` base whose `SPropertyData` carries the
    /// requested property. Reused by [`Request::FindInstancesOfType`].
    Entities(Vec<u64>),

    // -- Engine-call actuation (protocol v3) -------------------------------
    /// [`Request::GameThreadCall`] result: the raw RAX the engine function
    /// returned.
    GameThreadCallResult {
        ret: u64,
    },
    /// [`Request::RegisterFreeze`] succeeded; `handle` removes it later.
    RegisteredFreeze {
        handle: FreezeHandle,
    },

    // -- Dynamic guarded freeze (protocol v4) ------------------------------
    /// [`Request::StartFreeze`] succeeded; `handle` stops it later.
    FreezeStarted {
        handle: FreezeHandle,
    },
    /// [`Request::StopFreeze`] acknowledged (idempotent).
    FreezeStopped,
    /// [`Request::QueryFreezeStats`] counters: successful `writes`, guard-
    /// `skipped` ticks, and total `ticks` since the freeze started.
    FreezeStats {
        writes: u64,
        skipped: u64,
        ticks: u64,
    },

    // -- in-process writer discovery (protocol v5) -------------------------
    /// [`Request::FindWriter`] hit: the instruction that wrote the watched
    /// address. `rip` is the faulting instruction pointer (HW data breakpoints
    /// trap AFTER the write, so `rip` is the address of the *next* instruction;
    /// the host walks `bytes` to recover the writer's own start). `bytes` is a
    /// raw window starting at `bytes_start`. `regs` is the integer register
    /// file at the trap, in the fixed order
    /// `[rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp, r8..r15]` — one of these holds
    /// the base pointer the write addressed off of.
    WriterFound {
        rip: u64,
        bytes_start: u64,
        bytes: Vec<u8>,
        thread_id: u32,
        regs: [u64; 16],
    },
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Frame errors (decode side).
#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame too large: {got} bytes (max {max})")]
    Oversize { got: u32, max: u32 },
    #[error("frame too small: {got} bytes (min 1)")]
    Empty { got: u32 },
    #[error("postcard decode: {0}")]
    Decode(#[from] postcard::Error),
}

/// Encode `msg` into the postcard wire format (no length prefix — see
/// [`encode_framed`]).
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

/// Encode `msg` plus a `u32`-LE length prefix, ready to write in one shot.
pub fn encode_framed<T: Serialize>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    let body = postcard::to_allocvec(msg)?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a postcard payload (no length prefix).
pub fn decode<'a, T: Deserialize<'a>>(buf: &'a [u8]) -> Result<T, FrameError> {
    if buf.is_empty() {
        return Err(FrameError::Empty { got: 0 });
    }
    Ok(postcard::from_bytes(buf)?)
}

/// Parse a `u32`-LE length prefix. Returns the payload length or a
/// [`FrameError`] if it's empty or oversize.
pub fn parse_len_prefix(prefix: [u8; 4]) -> Result<u32, FrameError> {
    let len = u32::from_le_bytes(prefix);
    if len == 0 {
        return Err(FrameError::Empty { got: 0 });
    }
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::Oversize {
            got: len,
            max: MAX_FRAME_BYTES,
        });
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_hello() {
        let req = Request::Hello {
            client_version: PROTOCOL_VERSION,
        };
        let bytes = encode(&req).unwrap();
        let back: Request = decode(&bytes).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn roundtrip_welcome_framed() {
        let rsp = Response::Welcome {
            server_version: PROTOCOL_VERSION,
            pid: 26156,
            module_base: 0x1_4000_0000,
            module_size: 0x1700_0000,
        };
        let framed = encode_framed(&rsp).unwrap();
        let len = parse_len_prefix([framed[0], framed[1], framed[2], framed[3]]).unwrap();
        assert_eq!(len as usize, framed.len() - 4);
        let back: Response = decode(&framed[4..]).unwrap();
        assert_eq!(back, rsp);
    }

    #[test]
    fn roundtrip_set_property() {
        let req = Request::SetProperty {
            entity_va: 0x1_8F27_2A90,
            property: "m_infiniteAmmo".into(),
            value: GlacierValue::Bool(true),
        };
        let back: Request = decode(&encode(&req).unwrap()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn roundtrip_fire_node() {
        let req = Request::FireNode {
            node_va: 0x2FC1_FB80,
            inputs: alloc::vec![
                NodeInput {
                    property: "m_humanoid".into(),
                    value: GlacierValue::U64(0x2FE4_7BC8),
                },
                NodeInput {
                    property: "m_invulnerable".into(),
                    value: GlacierValue::Bool(true),
                },
            ],
            fire: NodeFire::SignalInputPin(0x4F10_66FB),
        };
        let back: Request = decode(&encode(&req).unwrap()).unwrap();
        assert_eq!(back, req);

        // The `Activate` alias and a registered-freeze handle round-trip too.
        let rsp = Response::RegisteredFreeze { handle: 7 };
        assert_eq!(decode::<Response>(&encode(&rsp).unwrap()).unwrap(), rsp);
        assert_eq!(NodeFire::Activate, NodeFire::Activate);
    }

    #[test]
    fn roundtrip_start_freeze() {
        // The difficulty-agnostic god-mode shape: hold current (+0x00) by
        // copying max (+0x04) each tick, guarded.
        let req = Request::StartFreeze {
            box_va: 0x1_96D8_DB10,
            write_offset: 0x00,
            source_offset: Some(0x04),
            value: GlacierValue::F32(0.0),
            value_kind: ValueKind::F32,
            guard_min: 1.0,
            guard_max: 10000.0,
        };
        let back: Request = decode(&encode(&req).unwrap()).unwrap();
        assert_eq!(back, req);

        for rsp in [
            Response::FreezeStarted { handle: 3 },
            Response::FreezeStopped,
            Response::FreezeStats {
                writes: 100,
                skipped: 5,
                ticks: 105,
            },
        ] {
            assert_eq!(decode::<Response>(&encode(&rsp).unwrap()).unwrap(), rsp);
        }
    }

    #[test]
    fn frame_size_enforced() {
        let prefix = (MAX_FRAME_BYTES + 1).to_le_bytes();
        assert!(matches!(
            parse_len_prefix(prefix),
            Err(FrameError::Oversize { .. })
        ));
        assert!(matches!(
            parse_len_prefix(0u32.to_le_bytes()),
            Err(FrameError::Empty { .. })
        ));
    }

    #[test]
    fn pipe_name_format() {
        assert_eq!(
            pipe_name_for_pid(26156),
            r"\\.\pipe\openforge-glacier-26156"
        );
    }

    #[test]
    fn value_kind_size_and_le() {
        assert_eq!(GlacierValue::I32(0).kind().size(), 4);
        assert_eq!(GlacierValue::F64(0.0).kind().size(), 8);
        assert_eq!(GlacierValue::Bool(true).to_le_bytes(), alloc::vec![1u8]);
        assert_eq!(GlacierValue::U16(0x0201).to_le_bytes(), alloc::vec![1u8, 2]);
        assert_eq!(GlacierValue::Bytes(alloc::vec![9u8; 3]).kind().size(), 3);
    }
}
