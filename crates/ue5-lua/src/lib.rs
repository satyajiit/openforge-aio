//! Embeddable Lua 5.4 runtime + UE4SS-compatible bindings, loaded into a
//! per-game DLL (today: `openforge-batman-lod-dll`).
//!
//! ## What this crate ships
//!
//! 1. [`LuaRuntime`] — a lifecycle owner for a Lua VM that lives on its
//!    own worker thread. Built with [`LuaRuntime::new`], driven by
//!    [`LuaRuntime::run`] / [`LuaRuntime::stop`], polled with
//!    [`LuaRuntime::status`] / [`LuaRuntime::drain_output`].
//! 2. [`LuaEngineHost`] — a trait the per-game DLL implements to bridge
//!    Lua bindings into its reflection engine. Methods are blocking and
//!    expected to run on whatever thread the runtime calls them from; the
//!    DLL serializes engine access on its own mutex.
//! 3. A UE4SS-compatible Lua API surface: `StaticFindObject`, `FindAllOf`,
//!    `obj:GetFullName()`, `obj:GetClass():GetFName():ToString()`,
//!    `obj:type()`, `obj:IsValid()`, `obj:GetIndex()`,
//!    `ExecuteInGameThread`, `ExecuteWithDelay`, `LoopAsync`,
//!    `RegisterKeyBind`, `Key.<NAME>`.
//!
//! ## UE4SS compatibility caveats
//!
//! v2 generates per-class metatables that turn `pc:K2_GetPawn()` into a
//! direct UFunction dispatch. The metatable is built lazily on first
//! attribute access against a class and cached for the life of the VM:
//!
//! ```lua
//! local pc  = StaticFindObject('/Game/.../BP_PlayerController')
//! local pawn = pc:K2_GetPawn()       -- generated; marshals automatically
//! ```
//!
//! Argument marshalling covers primitive UE5 types: every signed/unsigned
//! integer width (Int8/16/32/64, Byte/UInt16/32/64), Float / Double,
//! Bool, and raw `Bytes(n)` slots (Lua string of exactly `n` bytes).
//! OUT and RETURN parameters are decoded back from the post-call buffer
//! and surfaced as multiple Lua return values in declaration order.
//!
//! Struct arguments (`StructProperty`, `FName`, `FString`, soft object
//! refs, delegate slots) still go through the v1 escape hatch — the
//! caller serializes the parameter blob and decodes the result:
//!
//! ```lua
//! local raw_return_bytes = obj:CallFunction("ApplyGameplayTag", tag_bytes)
//! ```
//!
//! Object returns (`ObjectProperty`, surfaced as a `Bytes(8)` slot) are
//! wrapped in a fresh `UObjectHandle` whose class pointer is unknown
//! until the next reflection hop — chaining `obj:GetPawn():K2_GetPawn()`
//! triggers a `class_name_of` round-trip in between.
//!
//! ## Threading model
//!
//! The Lua VM is owned by *one* worker thread, full stop. Two ancillary
//! background threads exist:
//!   * A timer thread that fires queued `ExecuteWithDelay` callbacks back
//!     to the worker via the worker's mpsc inbox.
//!   * A keybind poll thread that polls `GetAsyncKeyState` at ~33 Hz and
//!     edge-detects press transitions, also dispatching via the worker
//!     inbox.
//!
//! User-script code therefore runs on the worker thread *only*. Host
//! implementations of [`LuaEngineHost`] can be called from any of the
//! three (a host call from a binding closure runs on the worker; a host
//! call from a custom future doesn't exist in v1). They must be `Send +
//! Sync` and reentrant.

mod bindings;
mod host;
mod keys;
mod output;
mod runtime;
mod workers;

pub use host::{FoundObject, LuaEngineHost, UFunctionParam, UFunctionParamFlags, UFunctionSig};
pub use openforge_ue5_protocol::{LuaLogLevel, LuaOutputLine, LuaScriptStatus};
pub use runtime::LuaRuntime;
