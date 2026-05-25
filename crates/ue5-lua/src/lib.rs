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
//! UE4SS generates per-class metatables that turn `pc:K2_GetPawn(...)`
//! into a direct UFunction dispatch. v1 of this crate intentionally does
//! NOT do that — generating metatables from the engine's UFunction chain
//! is a substantial follow-up. v1 ships a single generic escape hatch:
//!
//! ```lua
//! local raw_return_bytes = obj:CallFunction("K2_GetPawn", "")
//! ```
//!
//! The caller is responsible for encoding the parameter blob and decoding
//! the return blob according to the UFunction's parameter layout. A v2
//! generated-binding layer can be added on top of this without changing
//! [`LuaEngineHost`].
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

pub use host::{FoundObject, LuaEngineHost};
pub use openforge_ue5_protocol::{LuaLogLevel, LuaOutputLine, LuaScriptStatus};
pub use runtime::LuaRuntime;
