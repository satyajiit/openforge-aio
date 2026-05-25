//! Thin wrappers around `CreateFileW`/`ReadFile`/`WriteFile`/
//! `SetNamedPipeHandleState`/`WaitNamedPipeW`.
//!
//! Centralised here so [`crate::client`] doesn't directly mention any Win32
//! types except `HANDLE`. Each call returns [`HostError::Win32`] with the
//! call name baked in.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::core::PCWSTR;

use crate::error::{HostError, Result};

/// Owned pipe `HANDLE` that closes itself on drop.
pub(crate) struct PipeHandle(HANDLE);

impl PipeHandle {
    /// `CreateFileW(name, GENERIC_READ | GENERIC_WRITE, 0, NULL, OPEN_EXISTING,
    /// 0, NULL)` with a retry loop that handles `ERROR_PIPE_BUSY` via
    /// `WaitNamedPipeW(250 ms)` and `ERROR_FILE_NOT_FOUND` via a 250 ms sleep.
    /// Total wall-clock budget is bounded by `overall_timeout`.
    pub fn open(name: &str, overall_timeout: Duration) -> Result<Self> {
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        let start = Instant::now();
        let step = Duration::from_millis(250);

        loop {
            // SAFETY: `wide` is null-terminated; the value we pass to
            // CreateFileW is a stable pointer for the duration of the call.
            let result = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                )
            };

            match result {
                Ok(handle) if !handle.is_invalid() => {
                    debug!(pipe = name, "pipe opened");
                    // DLL serves in BYTE / BYTE-stream mode; no client-side
                    // mode switch needed. Length-prefixed framing means we
                    // never rely on the kernel for message boundaries.
                    return Ok(PipeHandle(handle));
                }
                Ok(_) | Err(_) => {
                    // Inspect the OS error to decide whether to retry.
                    // SAFETY: GetLastError is always callable.
                    let code = unsafe { GetLastError() };
                    let elapsed = start.elapsed();
                    let remaining = overall_timeout.saturating_sub(elapsed);
                    if remaining.is_zero() {
                        return Err(HostError::Win32 {
                            call: "CreateFileW(pipe)",
                            code: code.0,
                        });
                    }
                    if code == ERROR_PIPE_BUSY {
                        // Wait up to 250 ms for the server side to free up.
                        // SAFETY: `wide` is null-terminated and lives across
                        // the call.
                        let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), 250) };
                    } else if code == ERROR_FILE_NOT_FOUND {
                        // DLL hasn't created the pipe yet. Back off and retry.
                        std::thread::sleep(step.min(remaining));
                    } else {
                        warn!(?code, "CreateFileW(pipe) failed, retrying");
                        std::thread::sleep(step.min(remaining));
                    }
                }
            }
        }
    }

    /// Write `buf` in full. Returns `Disconnected` if the server hangs up
    /// mid-write.
    pub fn write_all(&self, buf: &[u8]) -> Result<()> {
        let mut remaining = buf;
        while !remaining.is_empty() {
            let mut written: u32 = 0;
            // SAFETY: handle valid; lpBuffer is a slice; `lpNumberOfBytesWritten`
            // is a valid u32 ptr.
            let res = unsafe { WriteFile(self.0, Some(remaining), Some(&mut written), None) };
            match res {
                Ok(()) => {
                    if written == 0 {
                        return Err(HostError::Disconnected);
                    }
                    remaining = &remaining[written as usize..];
                }
                Err(_) => {
                    // SAFETY: GetLastError is always callable.
                    let code = unsafe { GetLastError() };
                    if code == ERROR_BROKEN_PIPE || code == ERROR_NO_DATA {
                        return Err(HostError::Disconnected);
                    }
                    return Err(HostError::Win32 {
                        call: "WriteFile(pipe)",
                        code: code.0,
                    });
                }
            }
        }
        Ok(())
    }

    /// Read exactly `buf.len()` bytes. Returns `Disconnected` if the pipe
    /// is closed before the read completes.
    pub fn read_exact(&self, buf: &mut [u8]) -> Result<()> {
        let mut filled: usize = 0;
        while filled < buf.len() {
            let mut got: u32 = 0;
            // SAFETY: handle valid; tail slice is non-empty; `&mut got` is a
            // valid u32 ptr.
            let res = unsafe { ReadFile(self.0, Some(&mut buf[filled..]), Some(&mut got), None) };
            match res {
                Ok(()) => {
                    if got == 0 {
                        return Err(HostError::Disconnected);
                    }
                    filled += got as usize;
                }
                Err(_) => {
                    // SAFETY: GetLastError is always callable.
                    let code = unsafe { GetLastError() };
                    if code == ERROR_BROKEN_PIPE {
                        return Err(HostError::Disconnected);
                    }
                    return Err(HostError::Win32 {
                        call: "ReadFile(pipe)",
                        code: code.0,
                    });
                }
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own this handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: a Win32 pipe HANDLE is a kernel integer that can move across
// threads. The `Ue5Client` puts its `PipeHandle` behind a `&mut self` API
// (caller is single-threaded per client), so no aliasing exists.
unsafe impl Send for PipeHandle {}

/// Sized buffer pointer type for parity with the `*const c_void` shape some
/// callers might prefer. Currently unused publicly.
#[allow(dead_code)]
pub(crate) fn ptr_as_c_void<T>(p: *const T) -> *const c_void {
    p as *const c_void
}
