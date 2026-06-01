//! In-process hardware write-watchpoint — protocol v5 `FindWriter`.
//!
//! Locates the instruction that writes a target address WITHOUT attaching a
//! debugger. We arm an x86-64 hardware data breakpoint (DR0 + DR7, "break on
//! write") on the address across every game thread via `SetThreadContext`,
//! install a vectored exception handler, and wait for the single-step trap.
//! Because there is no `DebugActiveProcess`, Denuvo's anti-debug never sees a
//! debugger attach — this is the Denuvo-safe analogue of the discover
//! pipeline's external `watch_write` (whose DR-arming math we mirror here).
//!
//! Used to find the ammo-decrement / recoil-write instruction so it can be
//! code-patched. Only one watch runs at a time: the worker serves a single
//! client request synchronously, so the handoff statics below are sound.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use openforge_dll_common::local_reader::LocalReader;
use openforge_glacier_protocol::Response;

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
/// `EXCEPTION_SINGLE_STEP` NTSTATUS (a HW data breakpoint surfaces as one).
const NT_SINGLE_STEP: u32 = 0x8000_0004;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// DR7 slot-0 control bits: L0 (bit0), LE (bit8), RW0 (bits16-17), LEN0 (18-19).
const DR7_SLOT0_MASK: u64 = (1 << 0) | (1 << 8) | (0b11 << 16) | (0b11 << 18);

/// Byte window captured around the trapped RIP (host walks it to recover the
/// writer instruction's own start — HW breakpoints trap *after* the write).
const BYTE_WINDOW: usize = 64;

// --- VEH ↔ caller handoff (single concurrent watch; see module docs) --------
static HIT: AtomicBool = AtomicBool::new(false);
static HIT_RIP: AtomicU64 = AtomicU64::new(0);
static HIT_TID: AtomicU32 = AtomicU32::new(0);
static HIT_REGS: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

/// Vectored handler: on our DR0 single-step trap, record the writer once and
/// self-disarm DR on the faulting thread (so it doesn't re-trap before
/// teardown). Any non-DR0 exception is passed through untouched.
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
    // DR6 bit 0 set ⇒ DR0 (our watchpoint) is what fired.
    if unsafe { (*ctx).Dr6 } & 0b1 == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if !HIT.swap(true, Ordering::SeqCst) {
        unsafe {
            HIT_RIP.store((*ctx).Rip, Ordering::SeqCst);
            HIT_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
            let r = &HIT_REGS;
            r[0].store((*ctx).Rax, Ordering::SeqCst);
            r[1].store((*ctx).Rbx, Ordering::SeqCst);
            r[2].store((*ctx).Rcx, Ordering::SeqCst);
            r[3].store((*ctx).Rdx, Ordering::SeqCst);
            r[4].store((*ctx).Rsi, Ordering::SeqCst);
            r[5].store((*ctx).Rdi, Ordering::SeqCst);
            r[6].store((*ctx).Rbp, Ordering::SeqCst);
            r[7].store((*ctx).Rsp, Ordering::SeqCst);
            r[8].store((*ctx).R8, Ordering::SeqCst);
            r[9].store((*ctx).R9, Ordering::SeqCst);
            r[10].store((*ctx).R10, Ordering::SeqCst);
            r[11].store((*ctx).R11, Ordering::SeqCst);
            r[12].store((*ctx).R12, Ordering::SeqCst);
            r[13].store((*ctx).R13, Ordering::SeqCst);
            r[14].store((*ctx).R14, Ordering::SeqCst);
            r[15].store((*ctx).R15, Ordering::SeqCst);
        }
    }
    // Self-disarm on this thread: clear DR6 and DR0's enable so resuming this
    // thread doesn't immediately re-trap. Other threads are disarmed by the
    // caller's teardown loop.
    unsafe {
        (*ctx).Dr6 = 0;
        (*ctx).Dr0 = 0;
        (*ctx).Dr7 &= !DR7_SLOT0_MASK;
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Handle [`Request::FindWriter`](openforge_glacier_protocol::Request::FindWriter).
pub fn find_writer(addr: u64, width: u8, timeout_ms: u32) -> Response {
    if !matches!(width, 1 | 2 | 4 | 8) {
        return Response::Error(format!("bad watch width {width} (want 1/2/4/8)"));
    }
    let addr_us = addr as usize;

    HIT.store(false, Ordering::SeqCst);

    // SAFETY: `veh` is a valid handler fn; first=1 installs it at the front.
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
    if handle.is_null() {
        return Response::Error("AddVectoredExceptionHandler failed".into());
    }

    let pid = unsafe { GetCurrentProcessId() };
    let me = unsafe { GetCurrentThreadId() };
    let threads = list_threads(pid);
    let mut armed: Vec<u32> = Vec::new();
    for tid in threads {
        if tid == me {
            continue; // never watch the worker thread that's running this
        }
        if set_dr0(tid, addr_us, width) {
            armed.push(tid);
        }
    }
    if armed.is_empty() {
        unsafe {
            RemoveVectoredExceptionHandler(handle);
        }
        return Response::Error("could not arm DR0 on any game thread".into());
    }
    crate::flog!(
        "INFO",
        "find_writer: armed {} threads on 0x{:X} width={} timeout={}ms",
        armed.len(),
        addr,
        width,
        timeout_ms
    );

    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    while Instant::now() < deadline {
        if HIT.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let hit = HIT.load(Ordering::SeqCst);

    // Teardown: disarm every thread we armed, then drop the handler.
    for tid in &armed {
        clear_dr0(*tid);
    }
    unsafe {
        RemoveVectoredExceptionHandler(handle);
    }

    if !hit {
        return Response::Error(format!(
            "no write to 0x{addr:X} observed in {timeout_ms} ms"
        ));
    }

    let rip = HIT_RIP.load(Ordering::SeqCst);
    let thread_id = HIT_TID.load(Ordering::SeqCst);
    let mut regs = [0u64; 16];
    for (i, slot) in regs.iter_mut().enumerate() {
        *slot = HIT_REGS[i].load(Ordering::SeqCst);
    }

    // Read a window around RIP so the host can recover the writer's start +
    // synthesise an AOB. RIP is the *next* instruction (trap-after-write).
    let start = (rip as usize).saturating_sub(BYTE_WINDOW / 2);
    let mut buf = vec![0u8; BYTE_WINDOW];
    let bytes = match LocalReader::new().read_bytes(start, &mut buf) {
        Ok(()) => buf,
        Err(_) => Vec::new(),
    };

    crate::flog!(
        "INFO",
        "find_writer: HIT rip=0x{:X} tid={} ({} bytes @ 0x{:X})",
        rip,
        thread_id,
        bytes.len(),
        start
    );

    Response::WriterFound {
        rip,
        bytes_start: start as u64,
        bytes,
        thread_id,
        regs,
    }
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

/// Arm DR0 = `addr` with a "break on write" of `size` bytes on thread `tid`.
fn set_dr0(tid: u32, addr: usize, size: u8) -> bool {
    with_thread_context_mut(tid, |ctx| {
        ctx.Dr0 = addr as u64;
        let len_bits: u64 = match size {
            1 => 0b00,
            2 => 0b01,
            4 => 0b11,
            8 => 0b10,
            _ => 0b11,
        };
        // RW0 = 0b01 (write), LEN0 = len_bits, L0 + LE enabled.
        let value: u64 = (1 << 0) | (1 << 8) | (0b01 << 16) | (len_bits << 18);
        ctx.Dr7 = (ctx.Dr7 & !DR7_SLOT0_MASK) | value;
    })
}

/// Disarm DR0 (and clear DR6) on thread `tid`.
fn clear_dr0(tid: u32) -> bool {
    with_thread_context_mut(tid, |ctx| {
        ctx.Dr0 = 0;
        ctx.Dr6 = 0;
        ctx.Dr7 &= !DR7_SLOT0_MASK;
    })
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
