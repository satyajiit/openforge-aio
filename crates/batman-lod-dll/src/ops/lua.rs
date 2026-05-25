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

use std::sync::Arc;

use openforge_ue5_lua::LuaRuntime;
use openforge_ue5_protocol::{LuaScriptStatus, Response};

use crate::engine::UeEngine;
use crate::lua_host::EngineHost;

/// Per-connection Lua state. Lazily initialized on first `RunLua` to keep
/// the cost zero for connections that never use scripting (the mlua VM is
/// ~150 KiB resident even at idle).
pub struct LuaState {
    runtime: Option<LuaRuntime>,
}

impl LuaState {
    pub fn new() -> Self {
        Self { runtime: None }
    }

    /// Get-or-create the runtime, given the engine. Returns `None` only
    /// when `engine` is `None` (reflection not resolved) — every other path
    /// is infallible.
    fn ensure(&mut self, engine: Option<Arc<UeEngine>>) -> Option<&LuaRuntime> {
        if self.runtime.is_none() {
            let engine = engine?;
            let host = Arc::new(EngineHost::new(engine));
            self.runtime = Some(LuaRuntime::new(host));
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
