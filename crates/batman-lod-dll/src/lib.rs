//! `openforge-batman-lod-dll`: cdylib loaded into LEGO Batman: LotDK. Spins up a
//! worker thread that serves UE5 reflection over a named pipe to the
//! out-of-process trainer.
//!
//! See `crates/ue5-protocol` for the wire format and `worker.rs` for the
//! request handling loop.

#![cfg(windows)]

mod dll_log;
mod engine;
mod fname_repr;
mod local_reader;
mod log_ring;
mod lotdk;
mod lua_host;
mod names;
mod objects;
mod ops;
mod panic_guard;
mod pe;
mod probe;
mod seh;
mod walker;
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
/// Set to true by DllMain on DLL_PROCESS_DETACH; the worker loop checks this
/// on each iteration and exits cleanly.
pub(crate) static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// Duplicated thread HANDLE so DllMain can `CancelSynchronousIo` on the worker
/// to unblock it from `ConnectNamedPipe` during shutdown. Stored as `isize`
/// because `HANDLE` is `!Send + !Sync`.
pub(crate) static WORKER_THREAD_HANDLE: AtomicIsize = AtomicIsize::new(0);

/// Standard cdylib entry point. Spawns the worker thread on
/// `DLL_PROCESS_ATTACH` and returns immediately so we don't hold the loader
/// lock (the lock is held across DllMain; anything heavy here would deadlock
/// the host process).
///
/// # Safety
///
/// Called by the Windows loader with `hinst` = the just-loaded module handle.
/// `reserved` is non-null for static loads and null for `LoadLibrary` loads.
/// We do nothing with either pointer beyond passing `hinst` to
/// `DisableThreadLibraryCalls`.
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
                    .name("openforge-ue5-worker".into())
                    .spawn(move || worker::worker_entry(log))
                {
                    Ok(jh) => {
                        // Duplicate the thread handle so we can cancel its
                        // synchronous IO from DLL_PROCESS_DETACH later.
                        let raw = jh.as_raw_handle() as isize;
                        WORKER_THREAD_HANDLE.store(raw, Ordering::SeqCst);
                        // Intentionally leak the JoinHandle so the OS handle
                        // outlives DllMain. We re-wrap it on detach to wait
                        // / close.
                        std::mem::forget(jh);
                        crate::flog!("INFO", "worker thread spawned (handle=0x{raw:X})");
                    }
                    Err(e) => crate::flog!("ERROR", "worker thread spawn failed: {e}"),
                }
            });
        }
        x if x == DLL_PROCESS_DETACH => {
            // Signal the worker to exit, cancel its pending IO so it unblocks
            // from `ConnectNamedPipe`, then wait up to 1s for clean exit.
            // After that, close the handle — the loader unmaps our image
            // immediately after we return, so we must ensure the worker
            // thread no longer executes any of our code.
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
                // Wait up to 1000ms for the worker to exit cleanly.
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
