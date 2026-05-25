//! Dispatch glue for the protocol v5 Lua ops.
//!
//! The actual VM lives in `openforge-ue5-lua::LuaRuntime`. This module owns
//! one runtime instance per pipe client (lazily created on the first
//! `RunLua` request) and routes the four new request variants to it.
//!
//! Lifetime: the runtime is held by `ConnState`, so dropping the connection
//! drops the runtime, which joins its worker + timer + key-poll threads.
//! That's the cleanup guarantee the trainer relies on: a crashed host or a
//! `Disconnect` request leaves no Lua threads running inside the game.
//!
//! ## ACTIVE_LUA — bridge to the ProcessEvent hook
//!
//! The ProcessEvent detour ([`crate::game_thread_hook`]) is a global
//! DLL-lifetime construct, but the Lua runtime is per-connection. To let
//! the hook drain the *current* connection's game-thread queue without
//! plumbing the runtime through engine state, every connection's runtime
//! registers itself in [`ACTIVE_LUA`] on first `ensure` and clears the slot
//! on `Drop` (only if the slot still points to ITS runtime — last-writer
//! wins on overlapping connections, which can't happen given the named
//! pipe is single-instance, but the `Arc::ptr_eq` check defends against
//! a stale clear if that constraint ever loosens).
//!
//! The hook's tick callback locks [`ACTIVE_LUA`], clones the Arc (cheap),
//! drops the lock, then calls `drain_game_thread()` on the runtime — which
//! `try_lock`s the VM mutex and skips the tick if held. Net effect:
//! `ExecuteInGameThread` closures fire on the engine's own thread within
//! milliseconds of being queued.

use std::sync::Arc;
use std::sync::OnceLock;

use openforge_ue5_lua::LuaRuntime;
use openforge_ue5_protocol::{LuaScriptStatus, Response};
use parking_lot::Mutex;

use crate::engine::UeEngine;
use crate::lua_host::EngineHost;

/// Slot holding the currently-active per-connection runtime, exposed to
/// the ProcessEvent hook so its tick callback can find a runtime to drain
/// without per-connection plumbing.
///
/// `OnceLock` initializes the `Mutex<Option<...>>` lazily on first
/// `ensure`; the inner `Option` is populated then cleared by
/// `LuaState::ensure` / `LuaState::Drop`.
pub static ACTIVE_LUA: OnceLock<Mutex<Option<Arc<LuaRuntime>>>> = OnceLock::new();

/// Public helper for the hook installer: a cheap, panic-free tick callback
/// that snapshots [`ACTIVE_LUA`], drops the lock, and drains the
/// game-thread queue if a runtime is registered.
pub fn drain_active_lua() {
    let slot = match ACTIVE_LUA.get() {
        Some(s) => s,
        None => return,
    };
    // Clone the Arc out so we don't hold the slot mutex while calling
    // into the runtime (which itself takes the VM mutex).
    let rt = slot.lock().clone();
    if let Some(rt) = rt {
        rt.drain_game_thread();
    }
}

/// Per-connection Lua state. Lazily initialized on first `RunLua` to keep
/// the cost zero for connections that never use scripting (the mlua VM is
/// ~150 KiB resident even at idle).
pub struct LuaState {
    runtime: Option<Arc<LuaRuntime>>,
}

impl LuaState {
    pub fn new() -> Self {
        Self { runtime: None }
    }

    /// Get-or-create the runtime, given the engine. Returns `None` only
    /// when `engine` is `None` (reflection not resolved) — every other path
    /// is infallible.
    ///
    /// Side effects:
    ///   1. Registers the new runtime in [`ACTIVE_LUA`] so the game-thread
    ///      hook can find it. Cleared in [`LuaState::drop`].
    ///   2. Installs the ProcessEvent detour ([`crate::game_thread_hook`])
    ///      if not already installed — DLL-lifetime, idempotent. We defer
    ///      the install to here (rather than at engine attach) so trainer
    ///      attaches that never run Lua scripts pay zero detour overhead
    ///      and run against the un-modified engine. This shrinks the blast
    ///      radius of any hook bug to "users who actually run Lua scripts".
    fn ensure(&mut self, engine: Option<Arc<UeEngine>>) -> Option<&Arc<LuaRuntime>> {
        if self.runtime.is_none() {
            let engine = engine?;
            // Capture process_event before moving engine into the host —
            // we use it for the hook install below.
            let process_event_addr = engine.process_event;
            let host = Arc::new(EngineHost::new(engine));
            let rt = Arc::new(LuaRuntime::new(host));
            // Publish to the hook BEFORE storing locally so a racing
            // ProcessEvent (extremely unlikely — this runs on the pipe
            // worker thread) sees a valid runtime.
            let slot = ACTIVE_LUA.get_or_init(|| Mutex::new(None));
            *slot.lock() = Some(rt.clone());
            self.runtime = Some(rt);
            // Lazy hook install. `install` is idempotent — repeat calls
            // (e.g. successive pipe-client connections that each run a
            // Lua script) are no-ops after the first. Bailing on
            // `process_event_addr == 0` matches the existing `reflection`
            // guard so we never install with a null target.
            if process_event_addr != 0 {
                let cb: Arc<dyn Fn() + Send + Sync> = Arc::new(drain_active_lua);
                crate::game_thread_hook::install(process_event_addr, cb);
            }
        }
        self.runtime.as_ref()
    }

    pub fn run(&mut self, engine: Option<Arc<UeEngine>>, script: String, name: String) -> Response {
        match self.ensure(engine) {
            None => Response::Error("layout not resolved".into()),
            Some(rt) => match rt.run(script, name) {
                Ok(()) => Response::LuaStarted,
                Err(e) => Response::LuaError { message: e },
            },
        }
    }

    pub fn stop(&mut self) -> Response {
        match &self.runtime {
            None => Response::LuaStopped, // never started -> already stopped
            Some(rt) => match rt.stop() {
                Ok(()) => Response::LuaStopped,
                Err(e) => Response::LuaError { message: e },
            },
        }
    }

    pub fn status(&self) -> Response {
        match &self.runtime {
            None => Response::LuaStatusInfo(LuaScriptStatus {
                running: false,
                name: None,
                last_error: None,
            }),
            Some(rt) => Response::LuaStatusInfo(rt.status()),
        }
    }

    pub fn drain_output(&self, max_lines: u32) -> Response {
        let max = if max_lines == 0 {
            usize::MAX
        } else {
            max_lines as usize
        };
        let lines = match &self.runtime {
            None => Vec::new(),
            Some(rt) => rt.drain_output(max),
        };
        Response::LuaOutput { lines }
    }
}

impl Default for LuaState {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LuaState {
    fn drop(&mut self) {
        // Clear ACTIVE_LUA only if it still points to OUR runtime. The
        // named-pipe server is single-instance so concurrent connections
        // can't actually race here, but `Arc::ptr_eq` makes the cleanup
        // robust if that ever changes (a newer connection would have
        // overwritten the slot; we'd then incorrectly clear its entry
        // without this check).
        if let (Some(my_rt), Some(slot)) = (&self.runtime, ACTIVE_LUA.get()) {
            let mut guard = slot.lock();
            if let Some(active) = guard.as_ref()
                && Arc::ptr_eq(active, my_rt)
            {
                *guard = None;
            }
        }
    }
}
