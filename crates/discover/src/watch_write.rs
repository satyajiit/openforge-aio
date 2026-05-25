//! Hardware-breakpoint capture of the instruction that writes to a target
//! memory address.
//!
//! Uses x86-64 debug registers (DR0 + DR7) to set a write watchpoint on the
//! tracked address. The Win32 debugger API (`DebugActiveProcess` +
//! `WaitForDebugEvent`) surfaces the resulting `EXCEPTION_SINGLE_STEP`; we
//! capture the offending thread's `Rip` and read a window of bytes around it.
//!
//! DR registers are cleared on every known thread before detach — even if a
//! caller panics — via a `Drop` guard on `DebugSession`. Leaving DR set on
//! game threads would destabilise the process.

#![cfg(windows)]

use std::collections::HashSet;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use windows::Win32::Foundation::{
    CloseHandle, DBG_CONTINUE, DBG_EXCEPTION_NOT_HANDLED, HANDLE, NTSTATUS,
};
use windows::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_FLAGS, ContinueDebugEvent, DEBUG_EVENT, DebugActiveProcess,
    DebugActiveProcessStop, DebugSetProcessKillOnExit, GetThreadContext, ReadProcessMemory,
    SetThreadContext, WaitForDebugEvent,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenThread, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, ResumeThread,
    SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SET_CONTEXT,
    THREAD_SUSPEND_RESUME,
};

/// `CONTEXT_AMD64 | CONTEXT_DEBUG_REGISTERS` (0x00100010). Used when arming
/// or clearing the DR registers — we only need DR0..DR7 to be filled.
const CTX_DEBUG: u32 = 0x0010_0010;

/// `CONTEXT_AMD64 | CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_DEBUG_REGISTERS`
/// (0x00100013). Used when capturing a hit — we need DR6 (to confirm DR0 was
/// the trigger), Rip (so we can read the writing instruction's bytes), and
/// Rax..R15 (so pointer-chain discovery can record the values held in registers
/// at the moment of the write). Worth the extra cost only at hit time — not
/// during DR0 arming.
const CTX_CAPTURE_FULL: u32 = 0x0010_0013;

/// Windows NT status codes are stable u32 values; tying our match arms to
/// integer constants keeps the code resilient to upstream rename churn in
/// `windows-rs` (which has moved some of these between modules across minor
/// versions).
const NT_EXCEPTION_BREAKPOINT: u32 = 0x8000_0003;
const NT_EXCEPTION_SINGLE_STEP: u32 = 0x8000_0004;

const CREATE_PROCESS_EVT: u32 = 3;
const CREATE_THREAD_EVT: u32 = 2;
const EXCEPTION_EVT: u32 = 1;
const EXIT_PROCESS_EVT: u32 = 5;
const EXIT_THREAD_EVT: u32 = 4;
const LOAD_DLL_EVT: u32 = 6;
const UNLOAD_DLL_EVT: u32 = 7;
const OUTPUT_DEBUG_STRING_EVT: u32 = 8;
const RIP_EVT: u32 = 9;

#[derive(Debug, Clone)]
pub struct WatchWriteResult {
    /// Instruction-pointer of the writer at the moment the watchpoint fired.
    pub rip: usize,
    /// Bytes captured starting at `bytes_start_addr`.
    pub bytes: Vec<u8>,
    /// Absolute address of the first captured byte.
    pub bytes_start_addr: usize,
    /// Thread that performed the write.
    pub thread_id: u32,
    /// Address that was actually written (== `target_addr` passed in).
    pub hit_address: usize,
    /// General-purpose registers (Rax..R15) at the trap. Useful for pointer
    /// chain discovery: one of these likely holds the base of the object that
    /// owns the written field.
    pub registers: CapturedRegisters,
}

/// AMD64 general-purpose register snapshot. Serialised so it survives the
/// session round-trip; the values are typically pointers and/or scratch
/// integers, so we store them as raw u64.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapturedRegisters {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// Block until a write to `target_addr` is observed (or `timeout` expires),
/// returning the captured RIP + surrounding bytes.
///
/// `size_bytes` must be 1, 2, 4, or 8. `bytes_window` controls how much
/// instruction stream to read around the RIP — 48 bytes is plenty for the
/// AOB synthesiser; 64 if you want extra context.
pub fn watch_write(
    pid: u32,
    target_addr: usize,
    size_bytes: u8,
    timeout: Duration,
    bytes_window: usize,
) -> Result<WatchWriteResult> {
    let proc_handle = unsafe {
        OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid)
            .map_err(|e| anyhow!("OpenProcess(pid={pid}): {e}"))?
    };

    let mut session = DebugSession::attach(pid)?;

    // Drain the initial event burst (CREATE_PROCESS + LOAD_DLL × N + initial
    // breakpoint) before touching thread contexts. Calling GetThreadContext
    // before these are continued can fail with ERROR_NOACCESS on every thread.
    drain_initial_events(&mut session, Duration::from_millis(800));

    let initial_threads = list_threads(pid)?;
    if initial_threads.is_empty() {
        bail!("no threads found in pid {pid}");
    }
    let mut armed = 0usize;
    let mut first_err: Option<anyhow::Error> = None;
    for &tid in &initial_threads {
        match set_dr0(tid, target_addr, size_bytes) {
            Ok(()) => {
                armed += 1;
                session.track(tid);
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(anyhow!("{}", e));
                }
                debug!(tid, error = %e, "could not arm DR0 on thread");
            }
        }
    }
    if armed == 0 {
        bail!(
            "could not arm DR0 on any of {} threads. First error: {}",
            initial_threads.len(),
            first_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "<unknown>".into())
        );
    }
    info!(
        pid,
        armed,
        total = initial_threads.len(),
        addr = format!("0x{:X}", target_addr),
        timeout_ms = timeout.as_millis() as u64,
        "watching for writes"
    );

    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            unsafe {
                let _ = CloseHandle(proc_handle);
            }
            bail!(
                "no write to 0x{target_addr:X} observed in {} s",
                timeout.as_secs()
            );
        }

        let mut event = DEBUG_EVENT::default();
        let wait_ms: u32 = 200;
        if unsafe { WaitForDebugEvent(&mut event, wait_ms) }.is_err() {
            continue;
        }

        let mut status = DBG_CONTINUE;
        let mut hit: Option<WatchWriteResult> = None;

        match event.dwDebugEventCode.0 {
            CREATE_THREAD_EVT => {
                let tid = event.dwThreadId;
                debug!(tid, "new thread; arming DR0");
                let _ = set_dr0(tid, target_addr, size_bytes);
                session.track(tid);
            }
            CREATE_PROCESS_EVT => {
                // First event after attach. Track the main thread.
                let tid = event.dwThreadId;
                let _ = set_dr0(tid, target_addr, size_bytes);
                session.track(tid);
            }
            EXIT_THREAD_EVT => {
                session.forget(event.dwThreadId);
            }
            EXIT_PROCESS_EVT => {
                unsafe {
                    let _ = CloseHandle(proc_handle);
                }
                bail!("target process exited while watching");
            }
            LOAD_DLL_EVT | UNLOAD_DLL_EVT | OUTPUT_DEBUG_STRING_EVT | RIP_EVT => {
                // Routine — let the game continue.
            }
            EXCEPTION_EVT => {
                let info = unsafe { event.u.Exception };
                let code = info.ExceptionRecord.ExceptionCode.0 as u32;
                match code {
                    NT_EXCEPTION_BREAKPOINT => {
                        // Initial-attach probe breakpoint; swallow.
                    }
                    NT_EXCEPTION_SINGLE_STEP => {
                        let tid = event.dwThreadId;
                        match try_capture(tid, proc_handle, target_addr, bytes_window) {
                            CaptureOutcome::Captured(result) => {
                                hit = Some(result);
                                // status stays DBG_CONTINUE
                            }
                            CaptureOutcome::OursButFailed => {
                                // Our DR0 fired but we couldn't extract a
                                // usable capture (e.g. RIP=0 from a kernel
                                // transition). Continue normally — try again
                                // on the next write.
                            }
                            CaptureOutcome::NotOurs => {
                                // Some other single-step (game's own debug
                                // tooling, anti-cheat probe, etc). Forward
                                // to the process.
                                status = DBG_EXCEPTION_NOT_HANDLED;
                            }
                        }
                    }
                    _ => {
                        // Unrelated exception in the game; forward it.
                        status = DBG_EXCEPTION_NOT_HANDLED;
                    }
                }
            }
            _ => {}
        }

        let _ = unsafe { ContinueDebugEvent(event.dwProcessId, event.dwThreadId, status) };

        if let Some(result) = hit {
            unsafe {
                let _ = CloseHandle(proc_handle);
            }
            return Ok(result);
        }
    }
}

enum CaptureOutcome {
    /// DR0 fired, RIP read, bytes captured. Use the payload.
    Captured(WatchWriteResult),
    /// DR0 fired (DR6 bit 0 set) but we couldn't extract usable data —
    /// usually RIP=0 from a syscall transition, or ReadProcessMemory failed.
    /// The exception must still be DBG_CONTINUE'd so the game survives.
    OursButFailed,
    /// SINGLE_STEP from somewhere else (anti-cheat, game's own debug). The
    /// exception should propagate so the game's handler (if any) deals with it.
    NotOurs,
}

/// Examine the thread's debug registers + control state. Returns one of three
/// outcomes; the caller maps them to the right `DBG_*` status code.
fn try_capture(
    tid: u32,
    proc_handle: HANDLE,
    target_addr: usize,
    bytes_window: usize,
) -> CaptureOutcome {
    let ctx_box = match get_thread_context(tid, CTX_CAPTURE_FULL) {
        Ok(c) => c,
        Err(e) => {
            warn!(tid, error = %e, "GetThreadContext on hit failed");
            // We can't tell whose breakpoint this is. Conservative: claim it
            // as ours so the OS doesn't kill the game.
            return CaptureOutcome::OursButFailed;
        }
    };

    let dr6 = ctx_box.Dr6;
    if (dr6 & 0b1) == 0 {
        return CaptureOutcome::NotOurs;
    }

    // It's ours. Clear DR6 immediately so the same exception doesn't fire
    // again the moment we resume the thread.
    let _ = clear_dr6(tid);

    let rip = ctx_box.Rip as usize;
    if rip == 0 {
        warn!(
            tid,
            dr6 = format!("0x{dr6:X}"),
            "DR0 hit but RIP=0 (kernel transition?); skipping"
        );
        return CaptureOutcome::OursButFailed;
    }

    let registers = CapturedRegisters {
        rax: ctx_box.Rax,
        rbx: ctx_box.Rbx,
        rcx: ctx_box.Rcx,
        rdx: ctx_box.Rdx,
        rsi: ctx_box.Rsi,
        rdi: ctx_box.Rdi,
        rbp: ctx_box.Rbp,
        rsp: ctx_box.Rsp,
        r8: ctx_box.R8,
        r9: ctx_box.R9,
        r10: ctx_box.R10,
        r11: ctx_box.R11,
        r12: ctx_box.R12,
        r13: ctx_box.R13,
        r14: ctx_box.R14,
        r15: ctx_box.R15,
    };

    let half = bytes_window / 2;
    let read_start = rip.saturating_sub(half);
    let mut buf = vec![0u8; bytes_window];
    let mut nread: usize = 0;
    let read_ok = unsafe {
        ReadProcessMemory(
            proc_handle,
            read_start as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
            Some(&mut nread),
        )
        .is_ok()
    };

    if !read_ok || nread == 0 {
        warn!(
            rip = format!("0x{rip:X}"),
            "ReadProcessMemory around RIP failed"
        );
        return CaptureOutcome::OursButFailed;
    }
    buf.truncate(nread);

    CaptureOutcome::Captured(WatchWriteResult {
        rip,
        bytes: buf,
        bytes_start_addr: read_start,
        thread_id: tid,
        hit_address: target_addr,
        registers,
    })
}

struct DebugSession {
    pid: u32,
    threads: HashSet<u32>,
}

impl DebugSession {
    fn attach(pid: u32) -> Result<Self> {
        unsafe {
            let _ = DebugSetProcessKillOnExit(false);
            DebugActiveProcess(pid).map_err(|e| anyhow!("DebugActiveProcess({pid}): {e}"))?;
        }
        Ok(Self {
            pid,
            threads: HashSet::new(),
        })
    }

    fn track(&mut self, tid: u32) {
        self.threads.insert(tid);
    }

    fn forget(&mut self, tid: u32) {
        self.threads.remove(&tid);
    }
}

impl Drop for DebugSession {
    fn drop(&mut self) {
        for &tid in &self.threads {
            if let Err(e) = clear_dr0(tid) {
                debug!(tid, error = %e, "clear_dr0 on detach failed");
            }
        }
        unsafe {
            let _ = DebugActiveProcessStop(self.pid);
        }
    }
}

fn list_threads(pid: u32) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
            .map_err(|e| anyhow!("CreateToolhelp32Snapshot(THREAD): {e}"))?;
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
    Ok(out)
}

fn set_dr0(tid: u32, addr: usize, size: u8) -> Result<()> {
    with_thread_context_mut(tid, true, |ctx| {
        ctx.Dr0 = addr as u64;
        let len_bits: u64 = match size {
            1 => 0b00,
            2 => 0b01,
            4 => 0b11,
            8 => 0b10,
            _ => 0b11,
        };
        // Slot 0 fields in DR7:
        //   L0   = bit 0    (local enable)
        //   LE   = bit 8    (local exact, x86 legacy — harmless on AMD64)
        //   RW0  = bits 16-17  (00=exec, 01=write, 11=rw)
        //   LEN0 = bits 18-19  (00=1B, 01=2B, 10=8B, 11=4B)
        let mask: u64 = (1 << 0) | (1 << 8) | (0b11 << 16) | (0b11 << 18);
        let value: u64 = (1 << 0) | (1 << 8) | (0b01 << 16) | (len_bits << 18);
        ctx.Dr7 = (ctx.Dr7 & !mask) | value;
        Ok(())
    })
}

fn clear_dr0(tid: u32) -> Result<()> {
    with_thread_context_mut(tid, false, |ctx| {
        ctx.Dr0 = 0;
        let mask: u64 = (1 << 0) | (1 << 8) | (0b11 << 16) | (0b11 << 18);
        ctx.Dr7 &= !mask;
        ctx.Dr6 = 0;
        Ok(())
    })
}

fn clear_dr6(tid: u32) -> Result<()> {
    with_thread_context_mut(tid, false, |ctx| {
        ctx.Dr6 = 0;
        Ok(())
    })
}

fn get_thread_context(tid: u32, flags: u32) -> Result<Box<CONTEXT>> {
    let h = unsafe {
        OpenThread(THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, false, tid)
            .map_err(|e| anyhow!("OpenThread({tid}, GET_CONTEXT): {e}"))?
    };
    // Heap-allocate CONTEXT for guaranteed 16-byte alignment. GetThreadContext
    // returns ERROR_NOACCESS if the buffer isn't suitably aligned for the
    // XMM register fields embedded in CONTEXT on AMD64. Caller picks the flag
    // set: `CTX_CAPTURE` for RIP + DR6, `CTX_CAPTURE_FULL` to additionally
    // populate Rax..R15.
    let mut ctx_box: Box<CONTEXT> = Box::new(unsafe { std::mem::zeroed() });
    ctx_box.ContextFlags = CONTEXT_FLAGS(flags);
    let res = unsafe { GetThreadContext(h, &mut *ctx_box) };
    unsafe {
        let _ = CloseHandle(h);
    }
    res.map_err(|e| anyhow!("GetThreadContext({tid}): {e}"))?;
    Ok(ctx_box)
}

/// Open the thread, optionally suspend it, get context (debug-only), let the
/// closure mutate, then set context back. Uses a heap-allocated CONTEXT to
/// guarantee 16-byte alignment, which is required by GetThreadContext on AMD64.
fn with_thread_context_mut(
    tid: u32,
    suspend: bool,
    mut f: impl FnMut(&mut CONTEXT) -> Result<()>,
) -> Result<()> {
    let mut access = THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION;
    if suspend {
        access |= THREAD_SUSPEND_RESUME;
    }
    let h =
        unsafe { OpenThread(access, false, tid).map_err(|e| anyhow!("OpenThread({tid}): {e}"))? };
    let outcome = (|| -> Result<()> {
        if suspend {
            let prev = unsafe { SuspendThread(h) };
            if prev == u32::MAX {
                return Err(anyhow!("SuspendThread({tid}) failed"));
            }
        }
        let mut ctx_box: Box<CONTEXT> = Box::new(unsafe { std::mem::zeroed() });
        ctx_box.ContextFlags = CONTEXT_FLAGS(CTX_DEBUG);
        unsafe {
            GetThreadContext(h, &mut *ctx_box)
                .map_err(|e| anyhow!("GetThreadContext({tid}): {e}"))?;
        }
        ctx_box.ContextFlags = CONTEXT_FLAGS(CTX_DEBUG);
        f(&mut ctx_box)?;
        unsafe {
            SetThreadContext(h, &*ctx_box).map_err(|e| anyhow!("SetThreadContext({tid}): {e}"))?;
        }
        Ok(())
    })();
    if suspend {
        unsafe {
            let _ = ResumeThread(h);
        }
    }
    unsafe {
        let _ = CloseHandle(h);
    }
    outcome
}

/// Pull the first burst of debug events out of the queue without acting on
/// them. After `DebugActiveProcess` the OS injects an artificial
/// `CREATE_PROCESS` plus N `LOAD_DLL` plus initial breakpoint sequence;
/// threads stay in a pending state that returns `ERROR_NOACCESS` from
/// `GetThreadContext` until those have been continued.
fn drain_initial_events(session: &mut DebugSession, budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let mut event = DEBUG_EVENT::default();
        if unsafe { WaitForDebugEvent(&mut event, 50) }.is_err() {
            // Empty queue — give the OS a beat and try once more before
            // returning. Real events that show up later will be picked up by
            // the main loop.
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        match event.dwDebugEventCode.0 {
            CREATE_PROCESS_EVT | CREATE_THREAD_EVT => {
                session.track(event.dwThreadId);
            }
            EXIT_THREAD_EVT => {
                session.forget(event.dwThreadId);
            }
            _ => {}
        }
        let _ = unsafe { ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE) };
    }
}

// Re-export for callers that want to silence the unused-import warning in
// non-Windows builds (this whole module is gated on `cfg(windows)` so the
// real story is "fine on Windows, never compiled elsewhere").
#[allow(dead_code)]
fn _silence_unused_ntstatus(_n: NTSTATUS) {}
