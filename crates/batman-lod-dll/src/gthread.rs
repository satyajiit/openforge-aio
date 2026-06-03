//! Game-thread executor via a HARDWARE-BREAKPOINT rendezvous.
//!
//! UFunction dispatch (`ProcessEvent`) runs thread-affine Blueprint/engine code;
//! on this game build, calling it from the pipe-worker thread crashes the game
//! (minidump-confirmed: c0000005 in game code with ProcessEvent on our worker's
//! stack). It must run on the game's OWN thread.
//!
//! We do NOT inline-hook ProcessEvent: `retour::RawDetour::new` HANGS on this
//! build's ProcessEvent prologue (DLL log dies at "calling RawDetour::new" and
//! never returns). A hardware breakpoint touches no `.text` — it arms DR0 via
//! each thread's debug registers + a vectored exception handler — so it can't
//! deadlock in a hooking library and is invisible to AV/EDR. (This is the
//! Glacier executor's *idea*, not its code: there the rendezvous is needed
//! because Denuvo forbids `.text` patching; here it's needed because the
//! hooking library stalls. Same primitive, different reason.)
//!
//! Mechanism:
//!   1. Arm DR0 = ProcessEvent as an EXECUTE breakpoint on every game thread
//!      (all threads except our worker), via Suspend/SetThreadContext/Resume.
//!   2. Install a vectored exception handler.
//!   3. The next time the game calls ProcessEvent we trap ON THE GAME THREAD at
//!      its first instruction — a valid context for the dispatch. The handler
//!      runs our queued ProcessEvent call there (SEH-guarded), records the
//!      result, self-disarms DR0, and continues.
//!   4. Our queued call itself executes ProcessEvent (the armed address) so it
//!      re-traps once; the `CLAIMED` guard makes that nested hit just
//!      self-disarm and fall through, so the call proceeds.
//!
//! Single concurrent call: the named pipe is single-client and the worker
//! serves one request synchronously, so the handoff statics + the static parms
//! buffer are sound (same invariant `find_writer` / the Glacier executor rely
//! on). The parms buffer is `'static` so a job that completes right at the
//! deadline can never write through a freed pointer.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, CONTEXT, CONTEXT_FLAGS, EXCEPTION_POINTERS, GetThreadContext,
    RemoveVectoredExceptionHandler, SetThreadContext,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
    THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
};

/// `CONTEXT_AMD64 | CONTEXT_DEBUG_REGISTERS` — only DR0..DR7 need filling.
const CTX_DEBUG: u32 = 0x0010_0010;
/// `EXCEPTION_SINGLE_STEP` NTSTATUS (a HW execute breakpoint surfaces as one).
const NT_SINGLE_STEP: u32 = 0x8000_0004;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// DR7 slot-0 control bits: L0 (bit0), LE (bit8), RW0 (16-17), LEN0 (18-19).
const DR7_SLOT0_MASK: u64 = (1 << 0) | (1 << 8) | (0b11 << 16) | (0b11 << 18);

/// Max UFunction `ParmsSize` we marshal. Covers every shipped call
/// (GetGameProgressValue, K2_TeleportTo=48B, SetGameProgressValue, …); a larger
/// one returns an error rather than overflowing.
const PARMS_CAP: usize = 8192;

// --- VEH <-> caller handoff (single concurrent call; see module docs) --------
static ARMED: AtomicBool = AtomicBool::new(false);
static CLAIMED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static CALL_OK: AtomicBool = AtomicBool::new(false);
static JOB_PE: AtomicU64 = AtomicU64::new(0);
static JOB_OBJ: AtomicU64 = AtomicU64::new(0);
static JOB_UF: AtomicU64 = AtomicU64::new(0);
static JOB_LEN: AtomicUsize = AtomicUsize::new(0);
/// Tid of the thread that last claimed a job — i.e. the game thread. Cached so
/// repeated/bulk UFunction calls (mod_fly every frame, an unlock grant = N
/// calls) arm ONLY that one thread instead of all ~50. Arming every thread per
/// call suspends the whole process repeatedly and stalls ProcessEvent → the
/// rendezvous times out. `0` = unknown (first call, or invalidated as stale).
static GAME_TID: AtomicU32 = AtomicU32::new(0);

/// `'static` parms buffer the engine reads inputs from and writes outputs to.
/// Static (never freed) so a late job at the deadline can't write a dangling
/// pointer. Sound under the single-concurrent-call invariant.
struct ParmsBuf(UnsafeCell<[u8; PARMS_CAP]>);
// SAFETY: access is serialized by the single-concurrent-call invariant (one
// pipe client, synchronous worker). The worker fills it before arming and reads
// it after `DONE`; the VEH (one claimant) is the only other accessor, strictly
// between those points.
unsafe impl Sync for ParmsBuf {}
static PARMS: ParmsBuf = ParmsBuf(UnsafeCell::new([0u8; PARMS_CAP]));

fn parms_ptr() -> *mut u8 {
    PARMS.0.get() as *mut u8
}

/// Outcome of [`run_process_event_on_game_thread`].
pub enum GtOutcome {
    /// The call ran on the game thread. `true` = succeeded (out-params written
    /// back to the caller's buffer); `false` = it SEH-faulted (stale obj /
    /// wrong concrete subclass) — buffer contents are undefined.
    Ran(bool),
    /// The rendezvous wasn't hit within the timeout — the engine isn't calling
    /// ProcessEvent (paused / loading / menu). The call did NOT run.
    Timeout,
    /// Could not arm a hardware breakpoint on any game thread.
    NoThreads,
    /// `parms.len() > PARMS_CAP` — the UFunction's parameter block is larger
    /// than the static marshalling buffer.
    ParmsTooLarge,
}

/// Vectored handler: on our DR0 execute-trap, if the job is pending and
/// unclaimed, run it ON THIS (game) THREAD, store the result, mark done.
/// Always self-disarms DR0 on the faulting thread so the resumed thread —
/// including our own re-entrant call to ProcessEvent — doesn't re-trap.
unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let rec = unsafe { (*info).ExceptionRecord };
    let ctx = unsafe { (*info).ContextRecord };
    if rec.is_null() || ctx.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if unsafe { (*rec).ExceptionCode.0 } as u32 != NT_SINGLE_STEP {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // DR6 bit 0 set ⇒ DR0 (our execute breakpoint) fired.
    if unsafe { (*ctx).Dr6 } & 0b1 == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    // Claim exactly once. The winner is the game's own thread at ProcessEvent's
    // first instruction — the correct context for the dispatch. A nested hit
    // (our queued call re-executing ProcessEvent) finds CLAIMED set, so it
    // skips the job and just self-disarms below, letting the call proceed.
    if ARMED.load(Ordering::SeqCst)
        && !DONE.load(Ordering::SeqCst)
        && CLAIMED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let pe = JOB_PE.load(Ordering::SeqCst) as usize;
        let obj = JOB_OBJ.load(Ordering::SeqCst) as usize;
        let uf = JOB_UF.load(Ordering::SeqCst) as usize;
        // SEH-guarded: a stale obj / wrong subclass yields `false`, not a crash.
        let ok = crate::seh::seh_call_process_event(pe, obj, uf, parms_ptr());
        CALL_OK.store(ok, Ordering::SeqCst);
        // Remember which thread this was — it's the game thread, so the next
        // call can arm just it (fast path).
        GAME_TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        DONE.store(true, Ordering::SeqCst);
    }

    // Self-disarm DR0 on this thread (clear DR6 + DR0 + slot-0 enable) so the
    // resumed thread — including our own nested ProcessEvent call — does not
    // immediately re-trap. Other armed threads are cleared by teardown or by
    // their own (post-DONE) trap.
    unsafe {
        (*ctx).Dr6 = 0;
        (*ctx).Dr0 = 0;
        (*ctx).Dr7 &= !DR7_SLOT0_MASK;
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Enumerate the thread ids belonging to `pid`.
fn list_threads(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: snapshot handle is closed before return; entry is sized.
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
            Ok(s) => s,
            Err(_) => return out,
        };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        if Thread32First(snap, &mut entry).is_ok() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    out.push(entry.th32ThreadID);
                }
                if Thread32Next(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Suspend `tid`, read its debug-register context, let `f` mutate the DRs,
/// write it back, and resume. Returns `true` on success.
fn with_thread_context_mut(tid: u32, f: impl FnOnce(&mut CONTEXT)) -> bool {
    // SAFETY: handle closed before return; CONTEXT is heap-boxed for the
    // 16-byte alignment GetThreadContext requires on AMD64.
    unsafe {
        let h = match OpenThread(
            THREAD_GET_CONTEXT
                | THREAD_SET_CONTEXT
                | THREAD_QUERY_INFORMATION
                | THREAD_SUSPEND_RESUME,
            false,
            tid,
        ) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let mut ok = false;
        if SuspendThread(h) != u32::MAX {
            let mut ctx: Box<CONTEXT> = Box::new(std::mem::zeroed());
            ctx.ContextFlags = CONTEXT_FLAGS(CTX_DEBUG);
            if GetThreadContext(h, &mut *ctx).is_ok() {
                f(&mut ctx);
                ctx.ContextFlags = CONTEXT_FLAGS(CTX_DEBUG);
                ok = SetThreadContext(h, &*ctx).is_ok();
            }
            let _ = ResumeThread(h);
        }
        let _ = CloseHandle(h);
        ok
    }
}

/// Arm DR0 = `addr` as an *execute* breakpoint (RW=00 / LEN=00) on thread `tid`.
fn arm_exec(tid: u32, addr: u64) -> bool {
    with_thread_context_mut(tid, |ctx| {
        ctx.Dr0 = addr;
        // RW0 = 0b00 (execute), LEN0 = 0b00 (required for execute), L0 + LE.
        let value: u64 = (1 << 0) | (1 << 8);
        ctx.Dr7 = (ctx.Dr7 & !DR7_SLOT0_MASK) | value;
    })
}

/// Disarm DR0 (and clear DR6) on thread `tid`.
fn disarm(tid: u32) -> bool {
    with_thread_context_mut(tid, |ctx| {
        ctx.Dr0 = 0;
        ctx.Dr6 = 0;
        ctx.Dr7 &= !DR7_SLOT0_MASK;
    })
}

/// Run `ProcessEvent(obj, ufunction, parms)` on the game's own thread via a
/// hardware-breakpoint rendezvous on ProcessEvent. `parms` is updated in place
/// with the engine's out-parameters on success.
pub fn run_process_event_on_game_thread(
    pe: usize,
    obj: usize,
    ufunction: usize,
    parms: &mut [u8],
    timeout: Duration,
) -> GtOutcome {
    if parms.len() > PARMS_CAP {
        return GtOutcome::ParmsTooLarge;
    }

    // Publish the job. Copy inputs into the static buffer (the engine reads from
    // and writes back to it). Order matters: ARMED gates the handler.
    DONE.store(false, Ordering::SeqCst);
    CLAIMED.store(false, Ordering::SeqCst);
    CALL_OK.store(false, Ordering::SeqCst);
    JOB_PE.store(pe as u64, Ordering::SeqCst);
    JOB_OBJ.store(obj as u64, Ordering::SeqCst);
    JOB_UF.store(ufunction as u64, Ordering::SeqCst);
    JOB_LEN.store(parms.len(), Ordering::SeqCst);
    // SAFETY: single-concurrent invariant — no other accessor until DONE.
    unsafe {
        let dst = std::slice::from_raw_parts_mut(parms_ptr(), PARMS_CAP);
        dst[..parms.len()].copy_from_slice(parms);
    }

    // SAFETY: `veh` is a valid handler; first=1 installs it at the front.
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
    if handle.is_null() {
        crate::flog!("ERROR", "gthread: AddVectoredExceptionHandler failed");
        return GtOutcome::NoThreads;
    }
    ARMED.store(true, Ordering::SeqCst);

    let pid = unsafe { GetCurrentProcessId() };
    let me = unsafe { GetCurrentThreadId() };
    let deadline = Instant::now() + timeout;
    let mut armed: Vec<u32> = Vec::new();

    // Phase 1 — fast path: arm ONLY the cached game thread (one
    // Suspend/SetThreadContext/Resume) and wait briefly. This is what makes
    // per-frame (mod_fly) and bulk (an unlock grant = N calls) UFunction use
    // viable — arming all ~50 threads per call suspends the whole process
    // repeatedly and stalls ProcessEvent, which is what timed out the grant.
    let cached = GAME_TID.load(Ordering::SeqCst);
    if cached != 0 && cached != me && arm_exec(cached, pe as u64) {
        armed.push(cached);
        let sub = (Instant::now() + Duration::from_millis(750)).min(deadline);
        while Instant::now() < sub && !DONE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    // Phase 2 — cold path: no cache, or the cached thread didn't pump in time
    // (stale). Arm every game thread; the VEH records whichever one claims the
    // job into GAME_TID so the next call takes the fast path again.
    if !DONE.load(Ordering::SeqCst) {
        if cached != 0 {
            GAME_TID.store(0, Ordering::SeqCst); // invalidate the stale cache
        }
        for tid in list_threads(pid) {
            if tid == me || armed.contains(&tid) {
                continue; // never trap the worker; don't double-arm the cached tid
            }
            if arm_exec(tid, pe as u64) {
                armed.push(tid);
            }
        }
        if armed.is_empty() {
            ARMED.store(false, Ordering::SeqCst);
            unsafe {
                RemoveVectoredExceptionHandler(handle);
            }
            crate::flog!("ERROR", "gthread: could not arm DR0 on any game thread");
            return GtOutcome::NoThreads;
        }
        crate::flog!(
            "INFO",
            "gthread: cold path — armed {} thread(s) on ProcessEvent 0x{:X} (UFunction 0x{:X})",
            armed.len(),
            pe,
            ufunction
        );
        while Instant::now() < deadline && !DONE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    // If a job CLAIMED right at the deadline, give it a bounded grace window to
    // finish (it writes the static buffer) before we stop trusting DONE — so we
    // never read a half-written buffer, and the static buffer is never aliased.
    if !DONE.load(Ordering::SeqCst) && CLAIMED.load(Ordering::SeqCst) {
        let grace = Instant::now() + Duration::from_secs(2);
        while Instant::now() < grace && !DONE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    // Teardown: ungate the handler, disarm every thread we armed, drop handler.
    ARMED.store(false, Ordering::SeqCst);
    for tid in &armed {
        disarm(*tid);
    }
    unsafe {
        RemoveVectoredExceptionHandler(handle);
    }

    if !DONE.load(Ordering::SeqCst) {
        crate::flog!(
            "WARN",
            "gthread: ProcessEvent rendezvous not hit within timeout (engine paused/loading?)"
        );
        return GtOutcome::Timeout;
    }
    let ok = CALL_OK.load(Ordering::SeqCst);
    if ok {
        // Copy the engine's out-parameters back to the caller's buffer.
        // SAFETY: DONE is set, so the VEH is finished writing the static buffer.
        let n = JOB_LEN.load(Ordering::SeqCst).min(parms.len());
        unsafe {
            let src = std::slice::from_raw_parts(parms_ptr(), PARMS_CAP);
            parms[..n].copy_from_slice(&src[..n]);
        }
    }
    crate::flog!("INFO", "gthread: UFunction ran on game thread (ok={ok})");
    GtOutcome::Ran(ok)
}
