//! In-process memory reader that uses `ReadProcessMemory(GetCurrentProcess())`
//! so wild pointers return an error instead of crashing the game.
//!
//! `ReadProcessMemory` on the pseudo-handle `GetCurrentProcess()` is safe to
//! call on any address — invalid regions return `FALSE` with
//! `ERROR_PARTIAL_COPY` / `ERROR_NOACCESS`, no SEH frame required. This is the
//! cheapest way to get fault-isolated reads inside the same process.

use std::ffi::c_void;

use windows::Win32::Foundation::GetLastError;
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Read errors from a [`LocalReader`].
#[derive(Debug, Clone, Copy)]
pub enum ReadError {
    /// `ReadProcessMemory` returned FALSE.
    Failed,
    /// Pointer alignment / sentinel reject (e.g. NULL, < 0x10000, kernel half).
    BadPointer,
}

#[derive(Clone, Copy)]
pub struct LocalReader;

impl LocalReader {
    pub const fn new() -> Self {
        Self
    }

    /// Safe read into `out`. Returns `Err` on partial copy / invalid page.
    #[inline]
    pub fn read_bytes(self, addr: usize, out: &mut [u8]) -> Result<(), ReadError> {
        if out.is_empty() {
            return Ok(());
        }
        if !Self::looks_readable(addr) {
            return Err(ReadError::BadPointer);
        }
        let mut got: usize = 0;
        // SAFETY: ReadProcessMemory with the pseudo-handle from
        // GetCurrentProcess() is the canonical "fault-isolated read" pattern.
        // It will not raise an AV on an unreadable page; it returns FALSE and
        // sets last-error.
        let ok = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                addr as *const c_void,
                out.as_mut_ptr() as *mut c_void,
                out.len(),
                Some(&mut got as *mut usize),
            )
        };
        if ok.is_err() || got != out.len() {
            // Drain the last-error to keep the OS thread-local clean.
            let _ = unsafe { GetLastError() };
            return Err(ReadError::Failed);
        }
        Ok(())
    }

    /// Convenience: read a `u32` LE.
    #[inline]
    pub fn read_u32(self, addr: usize) -> Result<u32, ReadError> {
        let mut b = [0u8; 4];
        self.read_bytes(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Convenience: read a `u64` LE.
    #[inline]
    pub fn read_u64(self, addr: usize) -> Result<u64, ReadError> {
        let mut b = [0u8; 8];
        self.read_bytes(addr, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    /// Convenience: read a pointer-sized value as `usize` (LE u64 on x64).
    #[inline]
    pub fn read_ptr(self, addr: usize) -> Result<usize, ReadError> {
        self.read_u64(addr).map(|v| v as usize)
    }

    /// Write `src` bytes to `addr`. Uses `WriteProcessMemory(GetCurrentProcess())`
    /// which, like RPM, does not raise SEH on bad pages and reports failure via
    /// the return code. Does NOT call `VirtualProtect` — the caller must ensure
    /// the page is writable (UObject data pages typically are).
    #[inline]
    pub fn write_bytes(self, addr: usize, src: &[u8]) -> Result<(), ReadError> {
        if src.is_empty() {
            return Ok(());
        }
        if !Self::looks_readable(addr) {
            return Err(ReadError::BadPointer);
        }
        let mut put: usize = 0;
        // SAFETY: see read_bytes.
        let ok = unsafe {
            WriteProcessMemory(
                GetCurrentProcess(),
                addr as *const c_void,
                src.as_ptr() as *const c_void,
                src.len(),
                Some(&mut put as *mut usize),
            )
        };
        if ok.is_err() || put != src.len() {
            let _ = unsafe { GetLastError() };
            return Err(ReadError::Failed);
        }
        Ok(())
    }

    /// Cheap pointer sanity check that mirrors the legacy locator's reject
    /// filters: reject NULL, the first page, and the kernel half of x64 VA.
    #[inline]
    fn looks_readable(addr: usize) -> bool {
        (0x10000..0x0000_8000_0000_0000).contains(&addr)
    }
}

impl Default for LocalReader {
    fn default() -> Self {
        Self::new()
    }
}
