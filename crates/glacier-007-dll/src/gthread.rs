//! Game-thread executor — run an engine call on the game's OWN thread.
//!
//! Glacier logic-node actuation (`SignalInputPin`, and later `SetPropertyValue`)
//! touches thread-affine graph state and FAULTS when invoked from the pipe-worker
//! thread — proven live: every `ZEntityRef` convention and pin id SEH-faults when
//! called off-thread. To run such a call safely we hijack the game thread the
//! same Denuvo-safe way [`crate::find_writer`] does — **no `DebugActiveProcess`,
//! no `.text` detour, no MinHook**:
//!
//! 1. Arm a HARDWARE EXECUTE breakpoint (DR0, RW=00 / LEN=00) on a function the
//!    game itself calls every frame on its logic thread — `SignalInputPin` — on
//!    every thread except our own.
//! 2. Install a vectored exception handler.
//! 3. When the game naturally reaches that entry we are *on the game thread at a
//!    lock-free prologue* (instruction 0 — `mov rbp,[rcx]` — before the function
//!    acquires any graph lock). The handler runs our queued engine call there,
//!    records the result, self-disarms DR0, and continues, so the game's own
//!    call then proceeds untouched.
//!
//! The rendezvous (`SignalInputPin`) is universal: any per-frame game-thread
//! entry works to *gain* the thread; we then call whatever `JOB_FN` we like. For
//! the equip feature `JOB_FN == SignalInputPin == trap`, so our call re-traps
//! DR0 once — handled by the `CLAIMED` guard (the nested hit finds the slot
//! claimed, just disarms its own context and continues).
//!
//! Only one call runs at a time: the worker serves a single client request
//! synchronously, so the handoff statics are sound (mirrors `find_writer`).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS, RemoveVectoredExceptionHandler,
};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};

use openforge_dll_common::local_reader::LocalReader;

use crate::engine;
use crate::find_writer::{DR7_SLOT0_MASK, list_threads, with_thread_context_mut};
use crate::seh;

/// `EXCEPTION_SINGLE_STEP` NTSTATUS (a HW breakpoint surfaces as one).
const NT_SINGLE_STEP: u32 = 0x8000_0004;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

// --- VEH ↔ caller handoff (single concurrent call; see module docs) ---------
static ARMED: AtomicBool = AtomicBool::new(false);
static CLAIMED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static FAULTED: AtomicBool = AtomicBool::new(false);
static JOB_FN: AtomicU64 = AtomicU64::new(0);
static JOB_A0: AtomicU64 = AtomicU64::new(0);
static JOB_A1: AtomicU64 = AtomicU64::new(0);
static JOB_A2: AtomicU64 = AtomicU64::new(0);
static JOB_A3: AtomicU64 = AtomicU64::new(0);
static JOB_RET: AtomicU64 = AtomicU64::new(0);

// Diagnostic capture of the GAME's own pending SignalInputPin call at the
// rendezvous trap — RCX/RDX/R8 reveal the real ZEntityRef shape, pin id, and
// objref the engine actually uses, so we can fix our own call's convention
// instead of guessing. Captured once per `run_on_game_thread`.
static CAPTURED: AtomicBool = AtomicBool::new(false);
static CAP_RCX: AtomicU64 = AtomicU64::new(0);
static CAP_RDX: AtomicU64 = AtomicU64::new(0);
static CAP_R8: AtomicU64 = AtomicU64::new(0);
static CAP_RIP: AtomicU64 = AtomicU64::new(0);
// First 4 qwords pointed at by RCX / RDX / R8 — the real arg-struct contents.
static CAP_RCX_MEM: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static CAP_RDX_MEM: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static CAP_R8_MEM: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Fault-isolated read of the 4 qwords at `ptr` into `slots` (via
/// `LocalReader`, so an unmapped pointer is a silent miss, never a fault).
fn snap_ptr_mem(ptr: u64, slots: &[AtomicU64; 4]) {
    if ptr <= 0x1_0000 {
        return;
    }
    let mut buf = [0u8; 32];
    if LocalReader::new()
        .read_bytes(ptr as usize, &mut buf)
        .is_ok()
    {
        for (i, slot) in slots.iter().enumerate() {
            let q = u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap());
            slot.store(q, Ordering::SeqCst);
        }
    }
}

// --- probe mode: sample DISTINCT entities signaled through the rendezvous ---
// (the "is 0x14C767890 a generic entity-signal dispatcher, and what does a ZCL
// logic-node call look like through it?" gamble). The worker re-arms in a loop;
// each hit captures one distinct target (deduped by entity ptr) into the ring.
const RING: usize = 48;
/// Main module bounds (no ASLR on this title): base 0x140000000, size 0x16B1E000.
const MODULE_BASE: u64 = 0x1_4000_0000;
const MODULE_END: u64 = MODULE_BASE + 0x16B1_E000;
static PROBE: AtomicBool = AtomicBool::new(false);
static RING_N: AtomicUsize = AtomicUsize::new(0);
static R_ENTITY: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static R_ENTVT: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static R_RCX: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static R_RDX: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static R_R8: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];

fn ru64(addr: u64) -> Option<u64> {
    if addr <= 0x1_0000 {
        return None;
    }
    let mut b = [0u8; 8];
    LocalReader::new()
        .read_bytes(addr as usize, &mut b)
        .ok()
        .map(|_| u64::from_le_bytes(b))
}

fn in_module(p: u64) -> bool {
    (MODULE_BASE..MODULE_END).contains(&p)
}

/// Given the two qwords of a ZEntityRef-like pair, return `(entity_ptr, vtable)`
/// for whichever member points at an object whose vtable is in the main module.
fn pick_entity(q0: u64, q1: u64) -> (u64, u64) {
    for q in [q0, q1] {
        if let Some(vt) = ru64(q)
            && in_module(vt)
        {
            return (q, vt);
        }
    }
    (0, 0)
}

/// Vectored handler: on our DR0 execute-trap, if a job is pending and unclaimed,
/// run it ON THIS (game) THREAD, store the result, and mark it done. Always
/// self-disarms DR0 on the faulting thread so resuming doesn't re-trap.
unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let rec = unsafe { (*info).ExceptionRecord };
    let ctx = unsafe { (*info).ContextRecord };
    if rec.is_null() || ctx.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = unsafe { (*rec).ExceptionCode.0 } as u32;
    if code != NT_SINGLE_STEP {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // DR6 bit 0 set ⇒ DR0 (our execute breakpoint) is what fired.
    if unsafe { (*ctx).Dr6 } & 0b1 == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    // Probe mode: capture one DISTINCT signaled entity (deduped) into the ring,
    // then self-disarm + continue (the worker re-arms for the next sample).
    if PROBE.load(Ordering::SeqCst) {
        let (rcx, rdx, r8) = unsafe { ((*ctx).Rcx, (*ctx).Rdx, (*ctx).R8) };
        if let (Some(q0), Some(q1)) = (ru64(rcx), ru64(rcx.wrapping_add(8))) {
            let (ent, vt) = pick_entity(q0, q1);
            if ent != 0 {
                let n = RING_N.load(Ordering::SeqCst);
                let mut seen = false;
                for e in R_ENTITY.iter().take(n.min(RING)) {
                    if e.load(Ordering::SeqCst) == ent {
                        seen = true;
                        break;
                    }
                }
                if !seen {
                    let slot = RING_N.fetch_add(1, Ordering::SeqCst);
                    if slot < RING {
                        R_ENTITY[slot].store(ent, Ordering::SeqCst);
                        R_ENTVT[slot].store(vt, Ordering::SeqCst);
                        R_RCX[slot].store(rcx, Ordering::SeqCst);
                        R_RDX[slot].store(rdx, Ordering::SeqCst);
                        R_R8[slot].store(r8, Ordering::SeqCst);
                    }
                }
            }
        }
        unsafe {
            (*ctx).Dr6 = 0;
            (*ctx).Dr0 = 0;
            (*ctx).Dr7 &= !DR7_SLOT0_MASK;
        }
        return EXCEPTION_CONTINUE_EXECUTION;
    }

    // Capture the GAME's own pending call args once. At this trap RIP is at
    // SignalInputPin's first instruction, so RCX/RDX/R8 are the real (valid)
    // ZEntityRef / pin id / objref the engine is about to use — our ground
    // truth for the calling convention.
    if ARMED.load(Ordering::SeqCst) && !CAPTURED.swap(true, Ordering::SeqCst) {
        let (rcx, rdx, r8) = unsafe {
            CAP_RCX.store((*ctx).Rcx, Ordering::SeqCst);
            CAP_RDX.store((*ctx).Rdx, Ordering::SeqCst);
            CAP_R8.store((*ctx).R8, Ordering::SeqCst);
            CAP_RIP.store((*ctx).Rip, Ordering::SeqCst);
            ((*ctx).Rcx, (*ctx).Rdx, (*ctx).R8)
        };
        // Snapshot the struct each arg points at — the real ZEntityRef / pin /
        // objref layout (fault-isolated; an integer arg just reads as garbage).
        snap_ptr_mem(rcx, &CAP_RCX_MEM);
        snap_ptr_mem(rdx, &CAP_RDX_MEM);
        snap_ptr_mem(r8, &CAP_R8_MEM);
    }

    // Claim the job exactly once. The winning thread is, by construction, the
    // game's own logic thread at `SignalInputPin`'s first instruction — the
    // correct context for the engine call that faults off-thread.
    if ARMED.load(Ordering::SeqCst)
        && !DONE.load(Ordering::SeqCst)
        && CLAIMED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let fn_va = JOB_FN.load(Ordering::SeqCst);
        let a0 = JOB_A0.load(Ordering::SeqCst);
        let a1 = JOB_A1.load(Ordering::SeqCst);
        let a2 = JOB_A2.load(Ordering::SeqCst);
        let a3 = JOB_A3.load(Ordering::SeqCst);
        // SEH-guarded: a bad call yields None here rather than killing the game.
        match seh::seh_call4(fn_va, a0, a1, a2, a3) {
            Some(r) => JOB_RET.store(r, Ordering::SeqCst),
            None => {
                FAULTED.store(true, Ordering::SeqCst);
            }
        }
        DONE.store(true, Ordering::SeqCst);
    }

    // Self-disarm DR0 on this thread (clear DR6 + DR0 + slot-0 enable) so the
    // resumed thread — including our own re-entrant call to the rendezvous —
    // does not immediately re-trap. Other threads are disarmed by teardown.
    unsafe {
        (*ctx).Dr6 = 0;
        (*ctx).Dr0 = 0;
        (*ctx).Dr7 &= !DR7_SLOT0_MASK;
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Arm DR0 = `addr` as an *execute* breakpoint (RW=00, LEN=00) on thread `tid`.
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

/// Probe: over `timeout_ms`, re-arm the SignalInputPin rendezvous in a loop and
/// log every DISTINCT entity the game signals through it (with its RTTI-bearing
/// vtable + the raw RCX/RDX/R8). Reveals whether `0x14C767890` is a generic
/// entity-signal dispatcher and what a ZCL logic-node call looks like through
/// it. Pure observation — fires nothing.
#[allow(dead_code)] // diagnostic tooling; retained for future ABI investigation
pub fn probe_signal_calls(timeout_ms: u32) {
    let Some(trap) = engine::signal_input_pin() else {
        crate::flog!("ERROR", "probe: SignalInputPin unresolved");
        return;
    };
    RING_N.store(0, Ordering::SeqCst);
    for s in R_ENTITY.iter() {
        s.store(0, Ordering::SeqCst);
    }
    PROBE.store(true, Ordering::SeqCst);

    let handle = unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
    if handle.is_null() {
        PROBE.store(false, Ordering::SeqCst);
        return;
    }
    let pid = unsafe { GetCurrentProcessId() };
    let me = unsafe { GetCurrentThreadId() };
    let threads: Vec<u32> = list_threads(pid).into_iter().filter(|&t| t != me).collect();
    crate::flog!(
        "INFO",
        "probe: sampling distinct signals on 0x{:X} across {} threads for {}ms",
        trap,
        threads.len(),
        timeout_ms
    );

    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    while Instant::now() < deadline {
        for &tid in &threads {
            arm_exec(tid, trap);
        }
        if RING_N.load(Ordering::SeqCst) >= RING {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    PROBE.store(false, Ordering::SeqCst);
    for &tid in &threads {
        disarm(tid);
    }
    unsafe {
        RemoveVectoredExceptionHandler(handle);
    }

    let n = RING_N.load(Ordering::SeqCst).min(RING);
    crate::flog!(
        "INFO",
        "probe: captured {n} distinct signaled entit(y/ies):"
    );
    for i in 0..n {
        crate::flog!(
            "INFO",
            "  ent=0x{:X} vt=0x{:X}  RCX=0x{:X} RDX=0x{:X} R8=0x{:X}",
            R_ENTITY[i].load(Ordering::SeqCst),
            R_ENTVT[i].load(Ordering::SeqCst),
            R_RCX[i].load(Ordering::SeqCst),
            R_RDX[i].load(Ordering::SeqCst),
            R_R8[i].load(Ordering::SeqCst),
        );
    }
}

/// Run `fn_va(a0, a1, a2, a3)` on the game's logic thread and return its RAX.
///
/// Returns `None` on timeout (the rendezvous wasn't hit — e.g. no active logic
/// graph, as in a menu/cutscene) or if the engine call SEH-faulted. The call is
/// scheduled at `SignalInputPin`'s lock-free entry; see the module docs.
pub fn run_on_game_thread(
    fn_va: u64,
    args: [u64; 4],
    timeout_ms: u32,
    rendezvous_va: u64,
) -> Option<u64> {
    // The rendezvous is a per-frame game-thread fn we trap to seize the thread.
    // The default (`SignalInputPin`) is a physics/bfx-subsystem fn that may be
    // hit on the WRONG thread for thread-affine ZCL graph handlers (they fault
    // off the logic thread). Callers can override with a logic-thread fn — e.g.
    // the deferred-command drain `0x14135C6A0` — to seize the correct thread.
    let trap = if rendezvous_va != 0 {
        rendezvous_va
    } else {
        engine::signal_input_pin()?
    };

    // Publish the job, then arm. Order matters: ARMED gates the handler.
    DONE.store(false, Ordering::SeqCst);
    FAULTED.store(false, Ordering::SeqCst);
    CLAIMED.store(false, Ordering::SeqCst);
    CAPTURED.store(false, Ordering::SeqCst);
    JOB_FN.store(fn_va, Ordering::SeqCst);
    JOB_A0.store(args[0], Ordering::SeqCst);
    JOB_A1.store(args[1], Ordering::SeqCst);
    JOB_A2.store(args[2], Ordering::SeqCst);
    JOB_A3.store(args[3], Ordering::SeqCst);
    JOB_RET.store(0, Ordering::SeqCst);

    // SAFETY: `veh` is a valid handler fn; first=1 installs it at the front.
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
    if handle.is_null() {
        return None;
    }
    ARMED.store(true, Ordering::SeqCst);

    let pid = unsafe { GetCurrentProcessId() };
    let me = unsafe { GetCurrentThreadId() };
    let mut armed: Vec<u32> = Vec::new();
    for tid in list_threads(pid) {
        if tid == me {
            continue; // never trap the worker thread that's running this
        }
        if arm_exec(tid, trap) {
            armed.push(tid);
        }
    }
    if armed.is_empty() {
        ARMED.store(false, Ordering::SeqCst);
        unsafe {
            RemoveVectoredExceptionHandler(handle);
        }
        crate::flog!("ERROR", "gthread: could not arm DR0 on any game thread");
        return None;
    }
    crate::flog!(
        "INFO",
        "gthread: armed {} thread(s) on rendezvous 0x{:X}; fn=0x{:X} timeout={}ms",
        armed.len(),
        trap,
        fn_va,
        timeout_ms
    );

    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    while Instant::now() < deadline {
        if DONE.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    // Teardown: ungate the handler, disarm every thread we armed, drop handler.
    ARMED.store(false, Ordering::SeqCst);
    for tid in &armed {
        disarm(*tid);
    }
    unsafe {
        RemoveVectoredExceptionHandler(handle);
    }

    // Report the real call we observed (if any) — the convention ground truth.
    if CAPTURED.load(Ordering::SeqCst) {
        let m = |s: &[AtomicU64; 4]| {
            (
                s[0].load(Ordering::SeqCst),
                s[1].load(Ordering::SeqCst),
                s[2].load(Ordering::SeqCst),
                s[3].load(Ordering::SeqCst),
            )
        };
        let (ra, rb, rc, rd) = m(&CAP_RCX_MEM);
        let (da, db, dc, dd) = m(&CAP_RDX_MEM);
        let (ea, eb, ec, ed) = m(&CAP_R8_MEM);
        crate::flog!(
            "INFO",
            "gthread: REAL call @rip=0x{:X}\n  RCX=0x{:X} -> [{:016X} {:016X} {:016X} {:016X}]\n  RDX=0x{:X} -> [{:016X} {:016X} {:016X} {:016X}]\n  R8 =0x{:X} -> [{:016X} {:016X} {:016X} {:016X}]",
            CAP_RIP.load(Ordering::SeqCst),
            CAP_RCX.load(Ordering::SeqCst),
            ra,
            rb,
            rc,
            rd,
            CAP_RDX.load(Ordering::SeqCst),
            da,
            db,
            dc,
            dd,
            CAP_R8.load(Ordering::SeqCst),
            ea,
            eb,
            ec,
            ed,
        );
    }

    if !DONE.load(Ordering::SeqCst) {
        crate::flog!("WARN", "gthread: rendezvous not hit within {timeout_ms}ms");
        return None;
    }
    if FAULTED.load(Ordering::SeqCst) {
        let (frip, faddr, code) = seh::last_fault();
        crate::flog!(
            "WARN",
            "gthread: engine call SEH-faulted on game thread — code=0x{:08X} fault_rip=0x{:X} (module+0x{:X}) access_addr=0x{:X}",
            code,
            frip,
            frip.wrapping_sub(MODULE_BASE),
            faddr
        );
        return None;
    }
    let ret = JOB_RET.load(Ordering::SeqCst);
    crate::flog!("INFO", "gthread: call returned 0x{ret:X}");
    Some(ret)
}
