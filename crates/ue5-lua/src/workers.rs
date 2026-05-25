//! Background workers driving timed + key-bound callbacks.
//!
//! The Lua VM lives on a single dedicated worker thread (see
//! [`crate::runtime`]). Anything that fires a callback *into* the VM has to
//! arrive as a `Command` on that worker's mpsc — we never poke mlua from a
//! foreign thread, because the VM is decidedly not `Sync`.
//!
//! This module provides:
//!   * [`Workers`] — shared state: the delayed-task min-heap and the
//!     vk→`RegistryKey` keybind map.
//!   * [`spawn_delayed_thread`] — fires `Command::DispatchDelayed` when each
//!     scheduled task's deadline arrives.
//!   * [`spawn_key_poll_thread`] — polls `GetAsyncKeyState` at ~33 Hz on
//!     Windows, edge-detects press transitions, and fires
//!     `Command::DispatchKey`.
//!
//! Storing `mlua::RegistryKey` here (rather than `Function`) keeps the
//! callbacks alive across `Lua` mutability without us holding a `&Lua`
//! reference outside the worker thread.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AOrdering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mlua::RegistryKey;
use parking_lot::Mutex;

/// Inbox slots for the Lua worker thread. Re-declared here (instead of
/// referencing `runtime::Command`) only at compile time — we depend on the
/// command shape via a small trait-less newtype pattern: the runtime owns
/// the Sender and gives `Workers` a closure-free clone of it.
///
/// In practice we just import `Command` from the runtime; this module is
/// internal-only so the cyclic dependency cost is zero.
use crate::runtime::Command;

/// Identifier for a scheduled delayed task. Monotonic, never reused.
pub type DelayedId = u64;

/// One pending `ExecuteWithDelay`. Ordered by `deadline` ascending —
/// `BinaryHeap` is a max-heap, so we invert the comparison in [`Ord`].
pub struct DelayedTask {
    pub deadline: Instant,
    pub id: DelayedId,
}

impl PartialEq for DelayedTask {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}

impl Eq for DelayedTask {}

impl PartialOrd for DelayedTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DelayedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Invert so BinaryHeap acts as a min-heap on `deadline`.
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.id.cmp(&self.id))
    }
}

/// Shared state between the runtime, the timer thread, and the keybind
/// poller. Cheap to clone (everything's behind `Arc`/`Mutex`).
pub struct Workers {
    /// Min-heap of scheduled delayed dispatches.
    pub delayed: Mutex<BinaryHeap<DelayedTask>>,
    /// vk-code → registry slot for the user's callback closure.
    pub keybinds: Mutex<HashMap<u32, RegistryKey>>,
    /// `Lua::registry_key`s for delayed callbacks, keyed by id.
    pub delayed_callbacks: Mutex<HashMap<DelayedId, RegistryKey>>,
    /// Source of fresh `DelayedId`s.
    pub next_delayed_id: AtomicU64,
}

impl Default for Workers {
    fn default() -> Self {
        Self::new()
    }
}

impl Workers {
    pub fn new() -> Self {
        Self {
            delayed: Mutex::new(BinaryHeap::new()),
            keybinds: Mutex::new(HashMap::new()),
            delayed_callbacks: Mutex::new(HashMap::new()),
            next_delayed_id: AtomicU64::new(1),
        }
    }

    pub fn alloc_delayed_id(&self) -> DelayedId {
        self.next_delayed_id.fetch_add(1, AOrdering::Relaxed)
    }

    /// Drop every pending callback. Called on `Stop` and `Shutdown` to
    /// stop firing into a torn-down VM. Note: the `RegistryKey`s are
    /// dropped without `Lua::remove_registry_value` because we always
    /// pair this with replacing the VM itself — the registry goes away
    /// with it.
    pub fn clear(&self) {
        self.delayed.lock().clear();
        self.delayed_callbacks.lock().clear();
        self.keybinds.lock().clear();
    }
}

/// Spawn the delayed-task timer thread.
///
/// Sleeps until the next deadline (capped at 100 ms so the shutdown flag
/// is checked frequently). On wake, dispatches every task whose deadline
/// has passed, popping them off the heap.
pub fn spawn_delayed_thread(
    workers: Arc<Workers>,
    cmd_tx: Sender<Command>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("openforge-lua-delayed".into())
        .spawn(move || {
            const MAX_SLEEP: Duration = Duration::from_millis(100);
            while !shutdown.load(AOrdering::Acquire) {
                // Snapshot the next deadline without holding the lock
                // while we sleep.
                let next_deadline = workers.delayed.lock().peek().map(|t| t.deadline);

                let sleep_for = match next_deadline {
                    None => MAX_SLEEP,
                    Some(d) => d.saturating_duration_since(Instant::now()).min(MAX_SLEEP),
                };
                if sleep_for > Duration::ZERO {
                    std::thread::sleep(sleep_for);
                }

                // Dispatch every task whose deadline has now passed.
                loop {
                    let due = {
                        let mut heap = workers.delayed.lock();
                        match heap.peek() {
                            Some(t) if t.deadline <= Instant::now() => heap.pop(),
                            _ => None,
                        }
                    };
                    let Some(task) = due else { break };
                    if cmd_tx
                        .send(Command::DispatchDelayed { id: task.id })
                        .is_err()
                    {
                        // Worker thread is gone — nothing left to do.
                        return;
                    }
                }
            }
        })
        .expect("spawn delayed thread")
}

/// Spawn the keybind poll thread (Windows only).
///
/// Polls `GetAsyncKeyState` at ~33 Hz for each registered vk, edge-detects
/// the press transition, and fires `Command::DispatchKey`. On non-Windows
/// targets this is a no-op thread that immediately exits — the rest of
/// the runtime keeps working so unit tests can run on Linux CI.
pub fn spawn_key_poll_thread(
    workers: Arc<Workers>,
    cmd_tx: Sender<Command>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        std::thread::Builder::new()
            .name("openforge-lua-keys".into())
            .spawn(move || {
                let mut prev: HashMap<u32, bool> = HashMap::new();
                while !shutdown.load(AOrdering::Acquire) {
                    // Snapshot registered vks. The poll is short — under
                    // a microsecond per key — so we read+release quickly.
                    let vks: Vec<u32> = workers.keybinds.lock().keys().copied().collect();
                    for vk in &vks {
                        // SAFETY: `GetAsyncKeyState` is a simple Win32
                        // import with no preconditions; it returns the
                        // key state as a `SHORT`. We only use the
                        // high-bit ("currently down") flag.
                        let pressed = unsafe { GetAsyncKeyState(*vk as i32) as u16 & 0x8000 } != 0;
                        let was = prev.get(vk).copied().unwrap_or(false);
                        if pressed
                            && !was
                            && cmd_tx.send(Command::DispatchKey { vk: *vk }).is_err()
                        {
                            return;
                        }
                        prev.insert(*vk, pressed);
                    }
                    // Forget any vks that have since been unregistered so
                    // `prev` doesn't grow unbounded across re-bindings.
                    prev.retain(|k, _| vks.contains(k));

                    std::thread::sleep(Duration::from_millis(30));
                }
            })
            .expect("spawn key poll thread")
    }
    #[cfg(not(windows))]
    {
        // The non-Windows build has no `GetAsyncKeyState`. The thread
        // sleeps in a shutdown-respecting loop so tests that exercise the
        // runtime's lifecycle (start → drop) don't see a phantom join.
        let _ = workers;
        let _ = cmd_tx;
        std::thread::Builder::new()
            .name("openforge-lua-keys".into())
            .spawn(move || {
                while !shutdown.load(AOrdering::Acquire) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .expect("spawn key poll thread")
    }
}
