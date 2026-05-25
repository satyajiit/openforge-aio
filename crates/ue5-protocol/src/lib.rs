//! Wire protocol shared between every per-game OpenForge DLL (loaded into
//! the game process — today `openforge-batman-lod-dll`) and
//! [`openforge-ue5-host`] (injector + client in the trainer
//! process).
//!
//! ## Framing
//!
//! Every message is `u32` little-endian length, followed by `postcard`-encoded
//! payload. Pipes are message-mode on the server side, but framing is
//! length-prefixed so an accidental switch to byte-mode (or a forwarded
//! socket variant) still works. Max payload is [`MAX_FRAME_BYTES`].
//!
//! ## Connection model
//!
//! - Pipe name: `\\.\pipe\openforge-ue5-<pid>` (see [`pipe_name_for_pid`]).
//! - Pipe is bidirectional, one client at a time. The DLL re-opens after each
//!   client disconnect so the trainer can reattach without restarting the
//!   game.
//! - First exchange MUST be [`Request::Hello`] / [`Response::Welcome`]. The
//!   server rejects any other message before the handshake. Version mismatch
//!   = the server closes the pipe with an [`Response::Error`].
//!
//! ## Stability
//!
//! Bump [`PROTOCOL_VERSION`] for any wire-incompatible change. Adding a new
//! `Request`/`Response` variant is wire-compatible *only* if the receiver
//! treats unknown variants as a hard error (which `postcard` does
//! automatically — adding variants breaks the discriminant). So treat every
//! variant addition as a protocol bump.

#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Wire-protocol version.
///
/// History:
/// * `1` — initial: handshake + walk-object/property/function + read/write.
/// * `2` — single-channel memory-op family (`EnumModules`,
///   `ScanModule`, `ScanModuleAll`, `ScanHeapForU64`, `CodePatch`,
///   `RestorePatch`) so the host stops touching game memory directly.
/// * `3` — reflection lookups: `FindUObject`, `ResolveProperty`.
/// * `4` — UFunction invocation: `CallUFunction` (the universal
///   UE5 setter/method caller via `ProcessEvent`). Required for features
///   that grant in-game state changes through the engine's own logic —
///   "Unlock All Skill Bricks" being the first.
///
/// Adding a `Request`/`Response` variant is wire-breaking because
/// `postcard` keys variants by ordinal — pre-version clients/servers
/// reject the new variants outright with a decode error.
pub const PROTOCOL_VERSION: u32 = 4;

/// Maximum size of a single framed message, in bytes. Protects both sides
/// against a malformed length prefix that would otherwise allocate gigabytes.
/// A full `walk_objects()` on a large UE5 scene is typically ~30 MB serialized
/// (~500k UObjects × ~60 bytes each); cap at 256 MB to leave headroom.
pub const MAX_FRAME_BYTES: u32 = 256 * 1024 * 1024;

/// Build the named-pipe path the DLL listens on for a given game pid.
///
/// Format: `\\.\pipe\openforge-ue5-<pid>`. Pids are stable for the lifetime
/// of the process; reusing one across runs is fine because the pipe is
/// torn down on `DLL_PROCESS_DETACH` / pipe-server thread exit.
pub fn pipe_name_for_pid(pid: u32) -> String {
    alloc::format!(r"\\.\pipe\openforge-ue5-{pid}")
}

// ---------------------------------------------------------------------------
// Domain types (the algorithmic "what")
// ---------------------------------------------------------------------------

/// UObject / UStruct / FField / FProperty / UField / UFunction layout offsets.
///
/// These are runtime-probed by the DLL (the same algorithm as the legacy
/// external `UeEngine::probe_layout`) and shipped back in [`Response::Welcome`]
/// so the host can interpret raw `ReadProperty` payloads without re-probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UeOffsets {
    pub uobject_class_private: u32,
    pub uobject_name_private: u32,
    pub uobject_outer_private: u32,
    pub ustruct_super_struct: u32,
    pub ustruct_child_properties: u32,
    pub ustruct_children: u32,
    pub ffield_class_private: u32,
    pub ffield_next: u32,
    pub ffield_name_private: u32,
    pub fproperty_offset_internal: u32,
    pub fproperty_element_size: u32,
    pub ufield_next: u32,
    pub ufunction_flags: u32,
    pub ufunction_func: u32,
}

/// Property metadata returned by [`Request::WalkProperties`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropInfo {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    /// FFieldClass name, e.g. `IntProperty`, `FloatProperty`, `BoolProperty`.
    pub kind: String,
    pub defined_in_class: String,
}

/// UFunction metadata returned by [`Request::WalkFunctions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UFunctionInfo {
    pub name: String,
    pub addr: u64,
    pub native_func_ptr: u64,
    pub is_native: bool,
    pub defined_in_class: String,
}

/// Single UObject record returned by [`Request::WalkObjects`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UeObjectRef {
    pub addr: u64,
    pub class_ptr: u64,
    pub class_name: String,
    pub fqn: String,
}

/// Single match returned by [`Request::FindAllUObjects`] (in
/// [`Response::FoundObjects`]). Includes the FQN so the host can apply
/// richer filters than the wire `NamePredicate` supports — e.g. "include
/// any FQN containing `_Goon_` but exclude those also containing
/// `Default__`".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundObjectEntry {
    pub obj_addr: u64,
    pub class_addr: u64,
    pub fqn: String,
}

/// Width specifier used for [`Request::ReadProperty`] / [`Request::WriteProperty`].
///
/// Mirrors `cli::ScanType` on the host side. Variant order is the postcard
/// discriminant; only ever append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropKind {
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
    /// Read/write `n` raw bytes. Used for byte arrays and unknown widths.
    Bytes(u32),
}

impl PropKind {
    /// Bytes on the wire / in memory.
    pub fn size(self) -> u32 {
        match self {
            PropKind::I8 | PropKind::U8 | PropKind::Bool => 1,
            PropKind::I16 | PropKind::U16 => 2,
            PropKind::I32 | PropKind::U32 | PropKind::F32 => 4,
            PropKind::I64 | PropKind::U64 | PropKind::F64 => 8,
            PropKind::Bytes(n) => n,
        }
    }
}

/// Typed property value. Variant order is the postcard discriminant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropValue {
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

impl PropValue {
    pub fn kind(&self) -> PropKind {
        match self {
            PropValue::I8(_) => PropKind::I8,
            PropValue::I16(_) => PropKind::I16,
            PropValue::I32(_) => PropKind::I32,
            PropValue::I64(_) => PropKind::I64,
            PropValue::U8(_) => PropKind::U8,
            PropValue::U16(_) => PropKind::U16,
            PropValue::U32(_) => PropKind::U32,
            PropValue::U64(_) => PropKind::U64,
            PropValue::F32(_) => PropKind::F32,
            PropValue::F64(_) => PropKind::F64,
            PropValue::Bool(_) => PropKind::Bool,
            PropValue::Bytes(v) => PropKind::Bytes(v.len() as u32),
        }
    }
}

/// Logging level requested by the host for the DLL's internal trace ring.
/// The DLL ships its log lines back in [`Response::LogLines`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// One loaded module in the game process, as reported by
/// [`Request::EnumModules`]. Replaces the host's own toolhelp32 walk so the
/// trainer never needs `OpenProcess`/`Module32*W` — every module fact comes
/// from inside the game's address space via the DLL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEntry {
    /// File-name only (e.g. `"LEGOBatmanLotDK-Win64-Shipping.exe"`).
    /// Matched case-insensitively on the host side.
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

/// Wire-friendly AOB pattern. `bytes[i]` is the literal byte iff
/// `mask[i] == 0xFF`; `mask[i] == 0x00` marks `bytes[i]` as a wildcard
/// (its value is ignored). The two slices MUST have equal length and MUST
/// contain at least one literal byte (`mask[i] == 0xFF`).
///
/// The host crate's `openforge_core::Pattern` exposes a `wire()` helper
/// that builds this from a parsed pattern; the DLL's `aob_scan` op
/// reconstructs a scanner from `(bytes, mask)` directly without re-parsing
/// the human-readable string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternWire {
    pub bytes: Vec<u8>,
    pub mask: Vec<u8>,
}

/// How [`Request::FindUObject`] disambiguates when multiple instances of a
/// class exist in the GUObjectArray (typical for `*PlayerState`-style
/// classes: one inactive template plus one or more live instances).
///
/// Variant order is the postcard discriminant; only ever append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamePredicate {
    /// Any live instance. The DLL returns the FIRST match in
    /// GUObjectArray iteration order; callers that need stricter selection
    /// must use one of the variants below.
    Any,
    /// Object name exactly equals (case-insensitive).
    Exact(String),
    /// Object name (or any parent in the FQN chain) contains the substring
    /// (case-insensitive). Useful for "the live one is suffixed with
    /// `_C_2`" patterns.
    Contains(String),
    /// Object's FQN starts with the given prefix (case-insensitive). Lets
    /// callers scope by package — e.g. `"/Game/LegoBatman/..."` — to skip
    /// editor / template duplicates.
    FqnPrefix(String),
}

/// Result of [`Request::ResolveProperty`]: the offset + width metadata the
/// host needs to issue [`Request::ReadProperty`] / [`Request::WriteProperty`]
/// against an instance of the class.
///
/// `offset` is from the start of the UObject; `kind` is the [`PropKind`]
/// that matches the FProperty's wire-decoded type. `defined_in_class` is
/// the class FQN that owns the property (may be a parent of the lookup
/// class — UE5 inherits properties through the super chain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProperty {
    pub offset: u32,
    pub size: u32,
    pub kind: PropKind,
    pub defined_in_class: String,
}

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
    /// Walk the entire GUObjectArray and return every live UObject. `limit`
    /// caps the response (0 / `None` = unlimited).
    WalkObjects { limit: Option<u32> },
    /// Walk FProperty chain of `class_ptr` (UClass) and its supers.
    WalkProperties { class_ptr: u64 },
    /// Walk UFunction chain of `class_ptr` and its supers.
    WalkFunctions { class_ptr: u64 },
    /// Read raw memory at `addr` interpreted as `kind`.
    ReadProperty { addr: u64, kind: PropKind },
    /// Write `value` (must match `kind`) to `addr`.
    WriteProperty {
        addr: u64,
        kind: PropKind,
        value: PropValue,
    },
    /// Set the DLL's runtime log level. Affects what shows up in
    /// [`Request::DrainLog`].
    SetLogLevel(LogLevel),
    /// Drain the DLL's in-memory log ring (up to `max_lines`).
    DrainLog { max_lines: u32 },
    /// Ask the server to close this connection cleanly. The pipe-server
    /// thread then re-opens for the next client. The DLL itself stays loaded.
    Disconnect,

    // -- Single-channel memory ops -----------------------------------------
    //
    // These let the host stop touching game memory directly. Every read /
    // write / scan / patch crosses the pipe; the DLL executes them
    // in-process. See `openforge_ue5_host::Ue5DllCtx` for the host-side
    // wrapper that exposes them as a `Ctx`.
    /// Enumerate every loaded module in the game process. Replaces the
    /// host's toolhelp32 walk. The DLL caches its own list across calls.
    EnumModules,
    /// Scan a module's `.text` section for the first match of `pattern`.
    /// `module_name` is matched case-insensitively; the empty string means
    /// the main module.
    ScanModule {
        module_name: String,
        pattern: PatternWire,
    },
    /// Same as [`Request::ScanModule`] but returns every match. Used by
    /// signature-uniqueness probes (`extract-aob` / `verify`).
    ScanModuleAll {
        module_name: String,
        pattern: PatternWire,
    },
    /// Walk every committed, non-executable, non-guard RW region in the
    /// game's address space and return the addresses where an aligned `u64`
    /// equals `needle`. Used by the `heap_value_scan` locator. `label` is
    /// echoed into the DLL log for attribution (typically the feature id).
    ScanHeapForU64 {
        needle: u64,
        alignment: u32,
        label: String,
    },
    /// Apply a code patch in `.text`. Verifies `original` matches the bytes
    /// currently at `addr` before overwriting with `replacement`; refuses
    /// on mismatch (the game probably shipped a patch). The DLL tracks the
    /// applied patch and auto-restores it on client disconnect, so a stale
    /// trainer crash can't leave Lock Health "on" forever.
    CodePatch {
        addr: u64,
        original: Vec<u8>,
        replacement: Vec<u8>,
    },
    /// Restore previously-patched bytes. Idempotent: writing `original`
    /// over bytes that already equal `original` is a no-op. Also removes
    /// the patch from the DLL's auto-restore map (the caller now owns the
    /// lifecycle).
    RestorePatch { addr: u64, original: Vec<u8> },

    // -- Reflection-promoted feature ops ------------------------------------
    //
    // Higher-level lookups on top of the existing `WalkObjects` /
    // `WalkProperties` primitives. Each op runs in-DLL and is cached, so a
    // host-side feature read becomes a single round-trip:
    // FindUObject + ResolveProperty (both cached) + ReadProperty.
    /// Find a live UObject by class name. `class_path` is matched against
    /// each object's `class_name` (the last segment of its class FQN, e.g.
    /// `"ULegoPlayerState"`) case-insensitively. `predicate` picks among
    /// surviving candidates when more than one instance is loaded.
    ///
    /// Returns [`Response::FoundObject`] on success, [`Response::NotFound`]
    /// if no instance currently exists (typical when the player is in the
    /// main menu and gameplay objects haven't been spawned).
    FindUObject {
        class_path: String,
        predicate: NamePredicate,
    },

    /// Like [`Request::FindUObject`] but returns **every** match instead of
    /// stopping at the first. Used by "iterate-and-write" features (e.g.
    /// one-hit-kill freezes Health.CurrentValue on every live enemy's
    /// HealthAttributeSet). The DLL still walks GUObjectArray once but
    /// collects all class-matching objects, applies the predicate, and
    /// returns the full list. `max_results` caps the response size to keep
    /// frames under the 256 KB framer limit — set generously (e.g. 4096);
    /// the DLL stops adding once the cap is hit and reports the truncated
    /// count.
    ///
    /// Returns [`Response::FoundObjects`] with the matches (possibly empty
    /// when no live instance exists yet).
    FindAllUObjects {
        class_path: String,
        predicate: NamePredicate,
        max_results: u32,
    },

    /// Look up a single FProperty by name on a UClass and its supers.
    /// `class_addr` is the `class_ptr` returned in [`Response::FoundObject`]
    /// (or the `class_ptr` field of any [`UeObjectRef`]). The DLL caches
    /// `(class_addr, property_name)` results so repeated calls are O(1)
    /// after the first walk.
    ///
    /// Returns [`Response::Resolved`] on success, [`Response::NotFound`]
    /// if the property doesn't exist on the class chain.
    ResolveProperty {
        class_addr: u64,
        property_name: String,
    },

    // -- UFunction invocation ----------------------------------------------
    /// Call a UFunction on a live UObject by name, via the engine's
    /// `ProcessEvent` dispatcher. The DLL:
    ///   1. Walks `class_addr`'s function chain (cached) to find the
    ///      UFunction by `function_name` (case-insensitive). Miss returns
    ///      [`Response::NotFound`].
    ///   2. Copies `params` to a stack-allocated buffer sized to the
    ///      UFunction's `ParmsSize` (zero-pads short payloads; truncates
    ///      oversized ones is a hard error to avoid silent truncation of
    ///      stack-passed pointers).
    ///   3. Calls `ProcessEvent(obj, ufunction, parms_buf)`.
    ///   4. Reads back the post-call buffer (parameters can be `out`/`ref`)
    ///      and returns it in [`Response::CallOk`].
    ///
    /// **Parameter encoding**: caller is responsible for laying out the
    /// `params` blob in the exact byte order the UFunction expects. For
    /// "simple" trainer use cases (single int / bool argument) that's just
    /// the LE-encoded value. For struct args (e.g. FGameplayTag), the
    /// caller serializes the struct's expected memory image.
    CallUFunction {
        obj_addr: u64,
        class_addr: u64,
        function_name: String,
        params: Vec<u8>,
    },
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
        /// `GUObjectArray` base address (in the game's address space).
        guobject_array: u64,
        /// `FNamePool` base address.
        fname_pool: u64,
        /// Offset from `fname_pool` to the chunks-array pointer (typically
        /// `0x08`, `0x10`, `0x18`, or `0x20`).
        fname_pool_chunks_offset: u32,
        /// Resolved UE5 reflection layout offsets.
        offsets: UeOffsets,
        /// `FUObjectItem` stride (typically 24 in UE5).
        fuobject_item_stride: u32,
        /// True iff `offsets` were validated against a known UStruct.
        layout_validated: bool,
    },
    Pong,
    Objects(Vec<UeObjectRef>),
    Properties(Vec<PropInfo>),
    Functions(Vec<UFunctionInfo>),
    PropertyValue(PropValue),
    WriteOk,
    LogLines(Vec<String>),
    DisconnectAck,
    /// Server-side error. The connection stays open unless the error is
    /// fatal (handshake failure, internal panic), in which case the DLL
    /// closes after sending this.
    Error(String),

    // -- Single-channel memory-op responses ---------------------------------
    Modules(Vec<ModuleEntry>),
    /// Match addresses in the game's address space. Empty = no match.
    /// Single-element = first/unique match. Multi-element = `*_all` scan or
    /// heap-scan with multiple survivors.
    Matches(Vec<u64>),
    /// `CodePatch` ok; `previous` is the byte block that was overwritten
    /// (always equal to the request's `original` on success, surfaced so
    /// callers can independently verify the round-trip).
    PatchOk {
        previous: Vec<u8>,
    },
    /// `RestorePatch` ok.
    RestoreOk,

    // -- Reflection-promoted feature responses ------------------------------
    /// [`Request::FindUObject`] succeeded. `obj_addr` is the live UObject
    /// pointer in the game's address space (valid until the object is
    /// destroyed — typically a world transition); `class_addr` is the
    /// `UClass*` to feed into [`Request::ResolveProperty`].
    FoundObject {
        obj_addr: u64,
        class_addr: u64,
    },

    /// [`Request::FindAllUObjects`] result. Each match carries the live
    /// UObject pointer, its UClass pointer, and its built FQN (so the host
    /// can apply richer filters than the wire-level `NamePredicate` supports
    /// — typical: "exclude any FQN containing `_Playable_` or `_VEH_`").
    /// Empty `matches` is a legitimate "no live instances yet" reply —
    /// distinct from [`Response::NotFound`]. `truncated` is `true` iff the
    /// walk hit the request's `max_results` cap.
    ///
    /// FQNs balloon the frame; the DLL stops adding once collected size
    /// approaches the framer's 256 KB cap regardless of `max_results` (in
    /// practice ~2000 matches × ~100-byte FQN ≈ 200 KB).
    FoundObjects {
        matches: Vec<FoundObjectEntry>,
        truncated: bool,
    },
    /// [`Request::ResolveProperty`] succeeded.
    Resolved(ResolvedProperty),
    /// Lookup miss for [`Request::FindUObject`], [`Request::ResolveProperty`],
    /// or [`Request::CallUFunction`] (when the named UFunction doesn't exist
    /// on the class chain). **Not** an error — the object simply isn't
    /// loaded yet, or the named member doesn't exist on the requested class
    /// chain. Hosts use this to drive the "Pending → Ready" feature
    /// lifecycle (poll until FoundObject, then resolve once, then cache).
    NotFound,
    /// [`Request::CallUFunction`] succeeded. `return_value` is the post-call
    /// parameter buffer — out/ref parameters and the return slot have been
    /// updated in place by the engine. Functions with no out parameters
    /// return the input `params` blob unchanged (callers can ignore).
    CallOk {
        return_value: Vec<u8>,
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

/// Encode `msg` into the postcard wire format. Length-prefix not included —
/// see [`encode_framed`] for the full frame.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

/// Encode `msg` plus a `u32`-LE length prefix. Suitable for writing in one
/// shot to a pipe / socket.
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
    fn roundtrip_welcome() {
        let rsp = Response::Welcome {
            server_version: PROTOCOL_VERSION,
            pid: 1234,
            guobject_array: 0x14B65C4A0,
            fname_pool: 0x1B60AE9608,
            fname_pool_chunks_offset: 0x10,
            offsets: UeOffsets {
                uobject_class_private: 0x10,
                uobject_name_private: 0x18,
                uobject_outer_private: 0x20,
                ustruct_super_struct: 0x40,
                ustruct_child_properties: 0x50,
                ustruct_children: 0x48,
                ffield_class_private: 0x08,
                ffield_next: 0x20,
                ffield_name_private: 0x28,
                fproperty_offset_internal: 0x4C,
                fproperty_element_size: 0x38,
                ufield_next: 0x28,
                ufunction_flags: 0xB0,
                ufunction_func: 0xC8,
            },
            fuobject_item_stride: 24,
            layout_validated: true,
        };
        let framed = encode_framed(&rsp).unwrap();
        let len = parse_len_prefix([framed[0], framed[1], framed[2], framed[3]]).unwrap();
        assert_eq!(len as usize, framed.len() - 4);
        let back: Response = decode(&framed[4..]).unwrap();
        assert_eq!(back, rsp);
    }

    #[test]
    fn frame_size_enforced() {
        let prefix = (MAX_FRAME_BYTES + 1).to_le_bytes();
        assert!(matches!(
            parse_len_prefix(prefix),
            Err(FrameError::Oversize { .. })
        ));
        let prefix = 0u32.to_le_bytes();
        assert!(matches!(
            parse_len_prefix(prefix),
            Err(FrameError::Empty { .. })
        ));
    }

    #[test]
    fn pipe_name_format() {
        assert_eq!(pipe_name_for_pid(42), r"\\.\pipe\openforge-ue5-42");
    }

    #[test]
    fn prop_kind_size_matches_value() {
        assert_eq!(PropValue::I32(0).kind().size(), 4);
        assert_eq!(PropValue::F64(0.0).kind().size(), 8);
        assert_eq!(PropValue::Bool(true).kind().size(), 1);
        assert_eq!(PropValue::Bytes(vec![0u8; 7]).kind().size(), 7);
    }
}
