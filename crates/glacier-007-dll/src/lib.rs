//! `openforge-glacier-007-dll`: cdylib loaded into 007 First Light (Glacier 2
//! engine). Spins up a worker thread that serves the `ZTypeRegistry`
//! reflection walk + fault-isolated memory ops over a named pipe to the
//! out-of-process trainer.
//!
//! The structural decode is NOT reimplemented here: the worker builds an
//! in-process `openforge_core::Ctx` ([`local_ctx::LocalCtx`], backed by
//! `ReadProcessMemory(GetCurrentProcess())`) and drives the shared
//! `openforge-glacier-host` `GlacierReflection` engine against the game's own
//! address space. That is the whole architectural bet: the same reflection
//! code the discover CLI runs over external RPM runs verbatim in-process here,
//! where heap scans are ~100× faster and live entity pointers are directly
//! reachable.
//!
//! Denuvo note: this DLL performs data reads/writes and reflection only. It
//! installs no `.text` detours and does not patch code — Denuvo Anti-Tamper
//! integrity checks have nothing to trip on. We never modify the protected
//! image; we attach to the legitimately-running, already-decrypted process.
//!
//! See `crates/glacier-protocol` for the wire format and `worker.rs` for the
//! request-handling loop.

#![cfg(windows)]

mod dll_log;
mod local_ctx;
mod local_reader;
mod log_ring;
mod panic_guard;
mod pe;
mod reflection;
mod scan;
mod worker;

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use parking_lot::Once;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, TRUE};
use windows::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::BOOL;

use crate::log_ring::LogRing;

static WORKER_ONCE: Once = Once::new();
/// Set by DllMain on `DLL_PROCESS_DETACH`; the worker loop checks it each
/// iteration and exits cleanly.
pub(crate) static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// Duplicated worker thread HANDLE so DllMain can `CancelSynchronousIo` to
/// unblock it from `ConnectNamedPipe` during shutdown. Stored as `isize`
/// because `HANDLE` is `!Send + !Sync`.
pub(crate) static WORKER_THREAD_HANDLE: AtomicIsize = AtomicIsize::new(0);

/// Standard cdylib entry point. Spawns the worker thread on
/// `DLL_PROCESS_ATTACH` and returns immediately so we never do heavy work
/// under the loader lock.
///
/// # Safety
///
/// Called by the Windows loader with `hinst` = the just-loaded module handle.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(hinst: HMODULE, reason: u32, _reserved: *mut c_void) -> BOOL {
    match reason {
        x if x == DLL_PROCESS_ATTACH => {
            // SAFETY: hinst is the module handle the loader just produced.
            unsafe {
                let _ = DisableThreadLibraryCalls(hinst);
            }
            WORKER_ONCE.call_once(|| {
                crate::dll_log::init();
                crate::flog!("INFO", "DllMain DLL_PROCESS_ATTACH; spawning worker");
                let log = Arc::new(LogRing::new());
                match std::thread::Builder::new()
                    .name("openforge-glacier-worker".into())
                    .spawn(move || worker::worker_entry(log))
                {
                    Ok(jh) => {
                        let raw = jh.as_raw_handle() as isize;
                        WORKER_THREAD_HANDLE.store(raw, Ordering::SeqCst);
                        // Leak the JoinHandle so the OS handle outlives DllMain;
                        // we re-wrap it on detach to wait / close.
                        std::mem::forget(jh);
                        crate::flog!("INFO", "worker thread spawned (handle=0x{raw:X})");
                    }
                    Err(e) => crate::flog!("ERROR", "worker thread spawn failed: {e}"),
                }
            });
        }
        x if x == DLL_PROCESS_DETACH => {
            SHUTDOWN.store(true, Ordering::SeqCst);
            crate::flog!("INFO", "DllMain DLL_PROCESS_DETACH; signaling worker");
            let h = WORKER_THREAD_HANDLE.swap(0, Ordering::SeqCst);
            if h != 0 {
                let handle = HANDLE(h as *mut c_void);
                // SAFETY: handle is a valid thread handle we duplicated at
                // ATTACH. CancelSynchronousIo is a no-op if no IO is pending.
                unsafe {
                    let _ = windows::Win32::System::IO::CancelSynchronousIo(handle);
                }
                // Wait up to 1s for the worker to exit cleanly before the
                // loader unmaps our image.
                // SAFETY: handle valid.
                let _ = unsafe { WaitForSingleObject(handle, 1000) };
                // SAFETY: we own the handle.
                unsafe {
                    let _ = CloseHandle(handle);
                }
            }
            crate::flog!("INFO", "DllMain DLL_PROCESS_DETACH complete");
        }
        _ => {}
    }
    TRUE
}
