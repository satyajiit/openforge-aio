//! Lifecycle owner for the per-game Lua VM.
//!
//! [`LuaRuntime`] owns three things on the host side:
//!   1. A worker thread that owns the `mlua::Lua` and is the *only* thread
//!      ever allowed to touch it. The VM is decidedly not `Sync`, and even
//!      with mlua's `send` feature we still need a single-threaded driver
//!      to keep callbacks predictable.
//!   2. A delayed-callback timer thread (see [`crate::workers`]) that fires
//!      `Command::DispatchDelayed` when an `ExecuteWithDelay` schedule
//!      matures.
//!   3. A keybind poll thread that edge-detects `GetAsyncKeyState`
//!      transitions and fires `Command::DispatchKey`.
//!
//! All three communicate with the worker through a single
//! `mpsc::Sender<Command>`. The worker is a simple recv loop that locks
//! the VM-state mutex around each command body.
//!
//! ## Why one channel
//!
//! Reusing the worker's inbox for callbacks (rather than maintaining a
//! separate "callback queue") means a script that registers 5 keybinds and
//! 3 timers automatically gets serialized fairness: the worker processes
//! commands oldest-first, so a runaway timer cannot starve a keybind.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AOrdering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use mlua::Lua;
use openforge_ue5_protocol::{LuaOutputLine, LuaScriptStatus};
use parking_lot::Mutex;
use tracing::{debug, error, trace};

use crate::bindings::{self, BindingsCtx};
use crate::host::LuaEngineHost;
use crate::keys;
use crate::output::OutputBuffer;
use crate::workers::{self, Workers};

/// One-shot ack channel for [`Command::Run`] / [`Command::Stop`]. Boxed
/// so [`Command`] stays `Send` even though `Sender<Result<...>>` already
/// is — keeps the variant size small.
type Ack = Sender<Result<(), String>>;

/// Inbox messages for the Lua worker thread.
pub(crate) enum Command {
    /// Replace any in-flight script with a fresh chunk. Ack fires once
    /// the chunk's main body has returned (which for keybind-only
    /// scripts means "registrations are in place").
    Run {
        script: String,
        name: String,
        ack: Ack,
    },
    /// Cancel the current script + drop the VM (a new one is created on
    /// the next `Run`). Always succeeds.
    Stop { ack: Ack },
    /// Final teardown — the worker exits its recv loop.
    Shutdown,
    /// Fire a previously scheduled `ExecuteWithDelay` callback.
    DispatchDelayed { id: u64 },
    /// Fire a `RegisterKeyBind` callback.
    DispatchKey { vk: u32 },
}

/// What state the worker is in, from the runtime's perspective.
#[derive(Debug, Clone)]
pub(crate) enum RuntimeState {
    /// VM is alive but no script is running. Initial state and the state
    /// after a successful [`LuaRuntime::stop`].
    Idle,
    /// A script's main body is executing or its callbacks remain armed.
    Running { name: String },
    /// The most recent script terminated with an error. `last_error`
    /// surfaces the traceback to the host via [`LuaRuntime::status`].
    Stopped { last_error: Option<String> },
}

/// Shared state between the public [`LuaRuntime`] handle and the worker
/// threads. Wrapped in an `Arc` so the worker, the timer, and the key
/// poller can all hold it.
pub(crate) struct RuntimeInner {
    pub host: Arc<dyn LuaEngineHost>,
    pub output: Arc<OutputBuffer>,
    pub state: Mutex<RuntimeState>,
    pub workers: Arc<Workers>,
    pub cmd_tx: Sender<Command>,
    pub shutdown: Arc<AtomicBool>,
}

/// Public handle to a running Lua VM.
///
/// Construct via [`LuaRuntime::new`]; the constructor spawns all three
/// background threads. Drop the runtime to tear them down — the worker
/// joins synchronously so the destructor blocks until the VM is fully
/// down, which is the behavior the DLL needs on `DLL_PROCESS_DETACH`.
pub struct LuaRuntime {
    inner: Arc<RuntimeInner>,
    worker: Option<JoinHandle<()>>,
    delayed: Option<JoinHandle<()>>,
    keys: Option<JoinHandle<()>>,
}

impl LuaRuntime {
    /// Spawn the worker + timer + keybind threads, install the
    /// UE4SS-compat bindings, and return the handle. Cheap (no script
    /// is loaded yet).
    pub fn new(host: Arc<dyn LuaEngineHost>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let workers = Arc::new(Workers::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let output = Arc::new(OutputBuffer::new());

        let inner = Arc::new(RuntimeInner {
            host,
            output,
            state: Mutex::new(RuntimeState::Idle),
            workers: workers.clone(),
            cmd_tx: cmd_tx.clone(),
            shutdown: shutdown.clone(),
        });

        let worker_inner = inner.clone();
        let worker = std::thread::Builder::new()
            .name("openforge-lua-worker".into())
            .spawn(move || worker_loop(worker_inner, cmd_rx))
            .expect("spawn lua worker");

        let delayed =
            workers::spawn_delayed_thread(workers.clone(), cmd_tx.clone(), shutdown.clone());
        let keys = workers::spawn_key_poll_thread(workers, cmd_tx, shutdown);

        Self {
            inner,
            worker: Some(worker),
            delayed: Some(delayed),
            keys: Some(keys),
        }
    }

    /// Submit `script` for execution under chunk `name`. Blocks until the
    /// main chunk's `exec()` returns (which for keybind-only scripts is
    /// "as soon as the registrations are in place"). Returns the script's
    /// error formatted with a traceback on failure.
    pub fn run(&self, script: String, name: String) -> Result<(), String> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.inner
            .cmd_tx
            .send(Command::Run {
                script,
                name,
                ack: ack_tx,
            })
            .map_err(|_| "lua worker has exited".to_string())?;
        ack_rx
            .recv()
            .map_err(|_| "lua worker dropped ack channel".to_string())?
    }

    /// Cancel any in-flight script, drop the VM, and recreate a fresh
    /// one. Blocks until the worker confirms.
    pub fn stop(&self) -> Result<(), String> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.inner
            .cmd_tx
            .send(Command::Stop { ack: ack_tx })
            .map_err(|_| "lua worker has exited".to_string())?;
        ack_rx
            .recv()
            .map_err(|_| "lua worker dropped ack channel".to_string())?
    }

    /// Snapshot of the VM's lifecycle for the host's `LuaStatus` RPC.
    pub fn status(&self) -> LuaScriptStatus {
        match &*self.inner.state.lock() {
            RuntimeState::Idle => LuaScriptStatus {
                running: false,
                name: None,
                last_error: None,
            },
            RuntimeState::Running { name } => LuaScriptStatus {
                running: true,
                name: Some(name.clone()),
                last_error: None,
            },
            RuntimeState::Stopped { last_error } => LuaScriptStatus {
                running: false,
                name: None,
                last_error: last_error.clone(),
            },
        }
    }

    /// Drain up to `max` captured `print()` lines, oldest first.
    pub fn drain_output(&self, max: usize) -> Vec<LuaOutputLine> {
        self.inner.output.drain(max)
    }

    /// Number of currently buffered output lines (cheap, doesn't drain).
    pub fn buffered_output(&self) -> usize {
        self.inner.output.len()
    }
}

impl Drop for LuaRuntime {
    fn drop(&mut self) {
        // Signal both background threads to exit at their next poll. We
        // do this *before* sending Shutdown so a slow worker can't queue
        // a fresh delayed dispatch after the timer has exited.
        self.inner.shutdown.store(true, AOrdering::Release);
        // Best-effort shutdown: ignore the send error (worker already
        // gone is fine).
        let _ = self.inner.cmd_tx.send(Command::Shutdown);

        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        if let Some(h) = self.delayed.take() {
            let _ = h.join();
        }
        if let Some(h) = self.keys.take() {
            let _ = h.join();
        }
    }
}

fn build_lua_state(inner: &Arc<RuntimeInner>) -> mlua::Result<Lua> {
    let lua = Lua::new();
    keys::load_key_table(&lua)?;
    let ctx = Arc::new(BindingsCtx {
        host: inner.host.clone(),
        output: inner.output.clone(),
        workers: inner.workers.clone(),
        shutdown: inner.shutdown.clone(),
    });
    bindings::install(&lua, ctx)?;
    Ok(lua)
}

fn worker_loop(inner: Arc<RuntimeInner>, rx: Receiver<Command>) {
    // Build the initial VM. If this fails the runtime is unusable —
    // surface via Error state and keep the loop alive so subsequent
    // commands can fail cleanly (rather than the channel hanging).
    let mut lua = match build_lua_state(&inner) {
        Ok(l) => l,
        Err(e) => {
            error!("openforge-lua: failed to build initial VM: {e}");
            *inner.state.lock() = RuntimeState::Stopped {
                last_error: Some(format!("failed to build Lua VM: {e}")),
            };
            // Run a degraded loop that only handles Shutdown.
            for cmd in rx {
                if matches!(cmd, Command::Shutdown) {
                    return;
                }
            }
            return;
        }
    };

    // The 60s recv timeout exists so a fully idle VM still wakes up
    // periodically to observe the shutdown flag.
    loop {
        let cmd = match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if inner.shutdown.load(AOrdering::Acquire) {
                    return;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match cmd {
            Command::Shutdown => return,
            Command::Run { script, name, ack } => {
                trace!("openforge-lua: running script `{name}` ({} bytes)", script.len());
                // Cancel any in-flight script: drop the VM, drop all
                // registered callbacks, build a fresh state. This is
                // simpler than trying to surgically unregister.
                inner.workers.clear();
                lua = match build_lua_state(&inner) {
                    Ok(l) => l,
                    Err(e) => {
                        let msg = format!("failed to rebuild Lua VM: {e}");
                        *inner.state.lock() = RuntimeState::Stopped {
                            last_error: Some(msg.clone()),
                        };
                        let _ = ack.send(Err(msg));
                        continue;
                    }
                };
                *inner.state.lock() = RuntimeState::Running { name: name.clone() };
                let exec_result = lua.load(&script).set_name(&name).exec();
                match exec_result {
                    Ok(()) => {
                        // The main chunk returned. The script may still
                        // be "running" via keybinds / delays; we keep
                        // the state as Running until Stop or until all
                        // callbacks have drained. v1: leave it as
                        // Running so the host's LuaStatus reflects "I
                        // have keybinds armed".
                        let _ = ack.send(Ok(()));
                    }
                    Err(e) => {
                        let msg = format_lua_error(&e);
                        inner.output.push_error(format!("script `{name}` error: {msg}"));
                        *inner.state.lock() = RuntimeState::Stopped {
                            last_error: Some(msg.clone()),
                        };
                        inner.workers.clear();
                        let _ = ack.send(Err(msg));
                    }
                }
            }
            Command::Stop { ack } => {
                debug!("openforge-lua: stop requested");
                inner.workers.clear();
                lua = match build_lua_state(&inner) {
                    Ok(l) => l,
                    Err(e) => {
                        let msg = format!("failed to rebuild Lua VM after stop: {e}");
                        *inner.state.lock() = RuntimeState::Stopped {
                            last_error: Some(msg.clone()),
                        };
                        let _ = ack.send(Err(msg));
                        continue;
                    }
                };
                *inner.state.lock() = RuntimeState::Idle;
                let _ = ack.send(Ok(()));
            }
            Command::DispatchDelayed { id } => {
                if let Err(e) = bindings::dispatch_delayed(&lua, &inner.workers, id) {
                    let msg = format_lua_error(&e);
                    inner.output.push_error(format!("delayed callback error: {msg}"));
                    *inner.state.lock() = RuntimeState::Stopped {
                        last_error: Some(msg),
                    };
                }
            }
            Command::DispatchKey { vk } => {
                if let Err(e) = bindings::dispatch_key(&lua, &inner.workers, vk) {
                    let msg = format_lua_error(&e);
                    inner.output.push_error(format!("keybind (vk=0x{vk:X}) error: {msg}"));
                    *inner.state.lock() = RuntimeState::Stopped {
                        last_error: Some(msg),
                    };
                }
            }
        }
        if inner.shutdown.load(AOrdering::Acquire) {
            return;
        }
    }
}

/// Format an mlua error with its traceback (when present). mlua's `Debug`
/// impl includes the traceback already, so we just stringify it.
fn format_lua_error(e: &mlua::Error) -> String {
    format!("{e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use openforge_ue5_protocol::{NamePredicate, PropKind, PropValue, ResolvedProperty};
    use std::sync::Mutex as StdMutex;

    use crate::host::FoundObject;

    /// Test double: every method records its call args and returns a
    /// canned response. The runtime is single-threaded so we don't worry
    /// about reorderings.
    #[derive(Default)]
    struct MockHost {
        calls: StdMutex<Vec<String>>,
        find_one_result: StdMutex<Option<(u64, u64)>>,
        find_all_result: StdMutex<Vec<FoundObject>>,
        full_name: StdMutex<String>,
        class_name: StdMutex<String>,
    }

    impl MockHost {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: StdMutex::new(Vec::new()),
                find_one_result: StdMutex::new(Some((0xDEAD_BEEF, 0xCAFE_F00D))),
                find_all_result: StdMutex::new(vec![FoundObject {
                    obj_addr: 0x111,
                    class_addr: 0x222,
                    fqn: "/Game/Foo/Bar.Bar_C".into(),
                }]),
                full_name: StdMutex::new("/Game/Foo/Bar.Bar_C".into()),
                class_name: StdMutex::new("ULegoPlayerState".into()),
            })
        }
    }

    impl LuaEngineHost for MockHost {
        fn find_uobject(
            &self,
            class_path: &str,
            predicate: &NamePredicate,
        ) -> Result<Option<(u64, u64)>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("find_uobject({class_path}, {predicate:?})"));
            Ok(*self.find_one_result.lock().unwrap())
        }
        fn find_all_uobjects(
            &self,
            class_path: &str,
            _predicate: &NamePredicate,
            _max_results: usize,
        ) -> Result<Vec<FoundObject>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("find_all_uobjects({class_path})"));
            Ok(self.find_all_result.lock().unwrap().clone())
        }
        fn resolve_property(
            &self,
            _class_addr: u64,
            name: &str,
        ) -> Result<Option<ResolvedProperty>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("resolve_property({name})"));
            Ok(Some(ResolvedProperty {
                offset: 0x10,
                size: 4,
                kind: PropKind::I32,
                defined_in_class: "UTestClass".into(),
            }))
        }
        fn read_property(&self, _addr: u64, _kind: PropKind) -> Result<PropValue, String> {
            Ok(PropValue::I32(42))
        }
        fn write_property(&self, _addr: u64, _value: PropValue) -> Result<(), String> {
            Ok(())
        }
        fn call_ufunction(
            &self,
            _obj_addr: u64,
            _class_addr: u64,
            fn_name: &str,
            _params: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("call_ufunction({fn_name})"));
            Ok(vec![])
        }
        fn full_name_of(&self, _obj_addr: u64) -> Result<String, String> {
            Ok(self.full_name.lock().unwrap().clone())
        }
        fn class_name_of(&self, _obj_addr: u64) -> Result<String, String> {
            Ok(self.class_name.lock().unwrap().clone())
        }
    }

    #[test]
    fn print_round_trips_through_output_buffer() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host);
        rt.run("print('hi')".into(), "t".into()).unwrap();
        let lines = rt.drain_output(usize::MAX);
        assert!(
            lines.iter().any(|l| l.message == "hi"),
            "expected 'hi' line, got {lines:?}"
        );
    }

    #[test]
    fn run_error_surfaces_in_status_and_returns_err() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host);
        let err = rt
            .run("error('boom')".into(), "t".into())
            .expect_err("expected error");
        assert!(err.contains("boom"), "got: {err}");
        let st = rt.status();
        assert!(!st.running);
        let last = st.last_error.unwrap_or_default();
        assert!(last.contains("boom"), "status.last_error = {last}");
    }

    #[test]
    fn static_find_object_routes_to_host_and_returns_handle() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host.clone());
        rt.run(
            "local o = StaticFindObject('/Game/Foo'); print(o:GetFullName())".into(),
            "t".into(),
        )
        .unwrap();
        let lines = rt.drain_output(usize::MAX);
        assert!(
            lines
                .iter()
                .any(|l| l.message == "/Game/Foo/Bar.Bar_C"),
            "got lines: {lines:?}"
        );
        let calls = host.calls.lock().unwrap();
        assert!(
            calls.iter().any(|c| c.starts_with("find_uobject")),
            "calls: {calls:?}"
        );
    }

    #[test]
    fn output_capacity_drains_with_marker() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host);
        // Drive 5000 prints via a Lua loop — the buffer caps at 4096
        // and pushes a `<dropped N lines>` warn marker at the boundary.
        rt.run(
            "for i=1,5000 do print(i) end".into(),
            "overflow".into(),
        )
        .unwrap();
        let lines = rt.drain_output(usize::MAX);
        assert_eq!(lines.len(), crate::output::OUTPUT_CAPACITY);
        assert!(
            lines
                .iter()
                .any(|l| l.message.starts_with("<dropped ")),
            "expected a dropped-lines marker"
        );
    }

    #[test]
    fn stop_returns_vm_to_idle() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host);
        rt.run(
            "RegisterKeyBind(Key.F5, function() end)".into(),
            "kb".into(),
        )
        .unwrap();
        assert!(rt.status().running);
        rt.stop().unwrap();
        let st = rt.status();
        assert!(!st.running);
        assert!(st.last_error.is_none());
    }

    #[test]
    fn key_table_is_preloaded() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host);
        rt.run(
            "if Key.F5 == nil then error('missing Key.F5') end print(Key.F5)".into(),
            "kb".into(),
        )
        .unwrap();
        let lines = rt.drain_output(usize::MAX);
        // Win32 VK_F5 == 0x74 == 116.
        assert!(
            lines.iter().any(|l| l.message == "116"),
            "lines: {lines:?}"
        );
    }

    #[test]
    fn loop_async_uses_execute_with_delay() {
        let host = MockHost::new();
        let rt = LuaRuntime::new(host);
        // Don't actually wait for the timer — just confirm registration
        // succeeds without raising. The bootstrap snippet defines
        // LoopAsync.
        rt.run(
            "LoopAsync(1000, function() return true end)".into(),
            "loop".into(),
        )
        .unwrap();
    }
}
