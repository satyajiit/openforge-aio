//! ProcessEvent detour that drains the Lua game-thread queue on every
//! engine call.
//!
//! Why ProcessEvent: it fires dozens of times per game frame for input
//! handling, blueprint events, animation notifies — guaranteed to run on
//! the engine's game thread for any state-mutating call. Hooking it gives
//! Lua's ExecuteInGameThread real game-thread semantics without needing
//! UGameViewportClient::Tick (whose RVA we don't have for LotDK).
//!
//! Re-entrancy: a closure draining from the hook can itself trigger a
//! ProcessEvent (e.g. it calls obj:UnlockOutfit() → engine fires the
//! UnlockOutfit UFunction → our hook fires again). A TLS guard prevents
//! infinite recursion: only the outermost ProcessEvent on any given thread
//! runs the drain callback; nested calls fall through to the trampoline.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering as AOrdering};

use retour::RawDetour;

/// One-shot slot for the active detour. `RawDetour` owns the trampoline
/// allocation and the inline patch; dropping it would restore the original
/// bytes, which is exactly what `uninstall` does.
static HOOK: OnceLock<RawDetour> = OnceLock::new();

/// User-supplied callback invoked once per non-reentrant ProcessEvent call.
/// Wrapped in an `Arc` so the same callback can be referenced from both
/// the installer side and the detour body without lifetime gymnastics.
static TICK: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Set to true by `install`, false by `uninstall`. The detour body checks
/// this before invoking the tick — once `uninstall` runs we want all
/// in-flight game-thread calls to fall straight through.
static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Re-entrancy guard. `true` while the current thread is already inside
    /// the drain callback (or any ProcessEvent invoked transitively from
    /// it). The detour only runs the drain when this is false on entry.
    static IN_DRAIN: Cell<bool> = const { Cell::new(false) };
}

/// Install the ProcessEvent detour. Idempotent: subsequent calls (with any
/// args) are no-ops — the first install wins. Safe to call from any thread
/// after engine attach has completed.
///
/// # Safety implications
///
/// This rewrites the first bytes of UE5's `UObject::ProcessEvent` to JMP
/// into our trampoline. Once installed, every ProcessEvent call in the
/// game runs through our detour. The detour must NEVER call back into
/// code that may itself call ProcessEvent without first checking the TLS
/// guard — `drain_game_thread` already does the right thing.
pub fn install(process_event_addr: usize, on_tick: Arc<dyn Fn() + Send + Sync>) {
    // Stash the tick callback first so a racing detour call (vanishingly
    // unlikely — the hook isn't live yet) sees a valid callback.
    let _ = TICK.set(on_tick);

    // Build + enable the detour. If anything fails we log and bail; the
    // game keeps running, Lua's ExecuteInGameThread just becomes a no-op
    // queue (closures accumulate but never fire). Acceptable degradation
    // — the trainer surfaces a "game-thread hook unavailable" warning
    // separately.
    if HOOK.get().is_some() {
        crate::flog!(
            "INFO",
            "game_thread_hook::install: already installed; skipping"
        );
        return;
    }

    // SAFETY: `process_event_addr` is the resolved RVA of
    // `UObject::ProcessEvent`. The signature is well-known UE5 ABI:
    // `void(*)(UObject*, UFunction*, void*)`. `RawDetour::new` does the
    // size-classified inline patch + trampoline relocation; if the
    // prologue contains an instruction shape it can't relocate it returns
    // an error.
    let detour = match unsafe {
        RawDetour::new(
            process_event_addr as *const (),
            process_event_detour as *const (),
        )
    } {
        Ok(d) => d,
        Err(e) => {
            crate::flog!(
                "ERROR",
                "game_thread_hook::install: RawDetour::new failed: {e}"
            );
            return;
        }
    };

    // SAFETY: enabling the detour rewrites the first bytes of
    // ProcessEvent atomically (single CMPXCHG on the cache line). Other
    // threads that are *currently executing* inside ProcessEvent are
    // unaffected — they're past the patch point. Threads that enter
    // after this point go through our detour.
    if let Err(e) = unsafe { detour.enable() } {
        crate::flog!(
            "ERROR",
            "game_thread_hook::install: detour.enable failed: {e}"
        );
        return;
    }

    if HOOK.set(detour).is_err() {
        // Lost a race — another caller already installed. The detour we
        // just built drops here (which disables + restores the original
        // bytes). The original install's hook remains live.
        crate::flog!(
            "WARN",
            "game_thread_hook::install: lost install race; our detour dropped"
        );
        return;
    }
    ENABLED.store(true, AOrdering::Release);
    crate::flog!(
        "INFO",
        "game_thread_hook::install: detour live at 0x{process_event_addr:X}"
    );
}

/// Disable the detour (restoring the original ProcessEvent bytes). Called
/// during DLL detach so the loader doesn't unmap our image while a hooked
/// ProcessEvent call is still pending.
///
/// Note: the trampoline + bookkeeping aren't freed (we leak the `RawDetour`
/// in `HOOK` because OnceLock has no `take`), but that's harmless — the
/// memory comes back when the process exits.
pub fn uninstall() {
    ENABLED.store(false, AOrdering::Release);
    if let Some(hook) = HOOK.get() {
        // SAFETY: the detour is live; `disable` restores the original
        // bytes via CMPXCHG. Pending calls that are past the patch point
        // (i.e. already inside the trampoline) finish through their own
        // copy of the prologue.
        if let Err(e) = unsafe { hook.disable() } {
            crate::flog!(
                "WARN",
                "game_thread_hook::uninstall: detour.disable failed: {e}"
            );
        } else {
            crate::flog!("INFO", "game_thread_hook::uninstall: detour disabled");
        }
    }
}

/// The trampoline. Called by the engine in place of the original
/// `UObject::ProcessEvent`. Must call the original eventually (otherwise
/// the engine deadlocks waiting for the UFunction to complete).
///
/// # Safety
///
/// Called by the engine with the original ABI. We pass the args through
/// to the trampoline unchanged. The TLS guard ensures the drain callback
/// only fires at the outermost level of the call stack, so nested
/// ProcessEvent calls (triggered by drained closures) don't re-enter the
/// drain.
unsafe extern "system" fn process_event_detour(
    obj: *mut c_void,
    func: *mut c_void,
    parms: *mut c_void,
) {
    let was_in = IN_DRAIN.with(|c| {
        let prev = c.get();
        c.set(true);
        prev
    });

    if !was_in
        && ENABLED.load(AOrdering::Acquire)
        && let Some(cb) = TICK.get()
    {
        // The drain callback handles its own panic catching, but we
        // belt-and-brace here so a `Send + Sync` impl that panics
        // before the inner catch_unwind can't escape into the
        // engine's call stack.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb()));
    }

    // Reset the guard BEFORE calling the trampoline so the trampoline
    // call doesn't see a stale `true` (engine-internal ProcessEvent calls
    // initiated from inside ProcessEvent itself are expected to run
    // unhooked from our perspective — they're not "new" game-thread
    // ticks).
    IN_DRAIN.with(|c| c.set(was_in));

    // Call the trampoline (original ProcessEvent code path). If the hook
    // isn't installed (race during teardown), there's nothing safe to
    // call — return without touching engine state. In practice this
    // path is unreachable: the hook can only be invoked while installed.
    let trampoline_ptr = match HOOK.get() {
        Some(h) => h.trampoline() as *const () as usize,
        None => {
            crate::flog!(
                "ERROR",
                "process_event_detour: HOOK not set; cannot call trampoline"
            );
            return;
        }
    };
    // SAFETY: trampoline_ptr is a code address produced by `RawDetour`
    // pointing at relocated original-prologue + a JMP back to the post-
    // patch instruction. ABI matches the original ProcessEvent.
    let trampoline: extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
        unsafe { std::mem::transmute(trampoline_ptr) };
    trampoline(obj, func, parms);
}
