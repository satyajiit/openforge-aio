//! In-DLL code patcher. Implements the `.text`-overwrite dance:
//!
//! 1. Snapshot the bytes currently at `addr` (length = `replacement.len()`).
//! 2. Refuse the patch if the snapshot doesn't equal the caller-supplied
//!    `original` — that means the game shipped an update and the trainer
//!    needs new signatures, NOT a forced overwrite of unknown instructions.
//! 3. `VirtualProtect(... PAGE_EXECUTE_READWRITE, &old)`.
//! 4. Write `replacement` via the local reader (RPM/WPM with pseudo-handle).
//! 5. Restore the old page protection.
//! 6. `FlushInstructionCache` so the CPU doesn't keep executing the cached
//!    pre-patch bytes.
//!
//! All five ops report errors via return value — no SEH frame needed.
//! `VirtualProtect` returns `FALSE` rather than raising on uncommitted /
//! freed pages; WPM via `GetCurrentProcess()` is documented fault-isolated
//! (returns `FALSE` instead of AV on bad addresses).

use std::ffi::c_void;

use windows::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows::Win32::System::Memory::{
    PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect,
};
use windows::Win32::System::Threading::GetCurrentProcess;

use openforge_dll_common::local_reader::{LocalReader, ReadError};

/// Errors a `code_patch` op can surface. Translated to `Response::Error`
/// strings by the worker.
#[derive(Debug)]
pub enum PatchError {
    EmptyReplacement,
    LengthMismatch {
        original: usize,
        replacement: usize,
    },
    ReadFailed(ReadError),
    /// The bytes at `addr` don't equal `original`. Either the game patched
    /// the executable or the caller passed wrong constants. We refuse to
    /// patch — overwriting unknown instructions is how trainers crash
    /// games.
    OriginalMismatch {
        addr: usize,
        expected: Vec<u8>,
        actual: Vec<u8>,
    },
    VirtualProtectFailed,
    WriteFailed(ReadError),
}

impl core::fmt::Display for PatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PatchError::EmptyReplacement => write!(f, "replacement bytes are empty"),
            PatchError::LengthMismatch {
                original,
                replacement,
            } => write!(
                f,
                "original and replacement must be the same length (got {original} vs {replacement})"
            ),
            PatchError::ReadFailed(e) => write!(f, "verify-read failed: {e:?}"),
            PatchError::OriginalMismatch {
                addr,
                expected,
                actual,
            } => write!(
                f,
                "bytes at 0x{addr:X} don't match `original`; expected={expected:02X?} actual={actual:02X?}"
            ),
            PatchError::VirtualProtectFailed => write!(f, "VirtualProtect failed"),
            PatchError::WriteFailed(e) => write!(f, "write failed: {e:?}"),
        }
    }
}

/// Apply `replacement` over the bytes at `addr`. Returns the bytes that were
/// overwritten (always equal to `original` on success, surfaced for caller
/// round-trip verification).
pub fn apply(addr: usize, original: &[u8], replacement: &[u8]) -> Result<Vec<u8>, PatchError> {
    if replacement.is_empty() {
        return Err(PatchError::EmptyReplacement);
    }
    if original.len() != replacement.len() {
        return Err(PatchError::LengthMismatch {
            original: original.len(),
            replacement: replacement.len(),
        });
    }
    let len = replacement.len();
    // Step 1: snapshot.
    let mut actual = vec![0u8; len];
    LocalReader::new()
        .read_bytes(addr, &mut actual)
        .map_err(PatchError::ReadFailed)?;
    // Step 2: refuse on mismatch.
    if actual != original {
        return Err(PatchError::OriginalMismatch {
            addr,
            expected: original.to_vec(),
            actual,
        });
    }
    // Step 3: make page writable.
    let mut old_protect = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: addr is the page we just successfully read from (committed,
    // executable); len > 0; old_protect is a stack u32. VirtualProtect
    // returns FALSE on bad input rather than raising.
    let vp_ok = unsafe {
        VirtualProtect(
            addr as *const c_void,
            len,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if vp_ok.is_err() {
        return Err(PatchError::VirtualProtectFailed);
    }
    // Step 4: write replacement.
    let write_result = write_bytes_at(addr, replacement);
    // Step 5: restore protection regardless of write outcome.
    // SAFETY: same invariants as the first VirtualProtect; old_protect is
    // the value we just received.
    let _ = unsafe {
        VirtualProtect(
            addr as *const c_void,
            len,
            old_protect,
            &mut PAGE_PROTECTION_FLAGS(0),
        )
    };
    if let Err(e) = write_result {
        return Err(PatchError::WriteFailed(e));
    }
    // Step 6: flush instruction cache so the CPU stops executing stale bytes.
    // SAFETY: handle is the current-process pseudo-handle; range is the bytes
    // we just patched.
    unsafe {
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(addr as *const c_void), len);
    }
    Ok(actual)
}

/// Write `bytes` to `addr` via the local reader's WriteProcessMemory wrapper.
/// Used both by the patch-write step above and by the auto-restore on
/// connection drop (which doesn't go through the `VirtualProtect` dance
/// because the destination page was already made writable when the patch
/// was originally applied — but defensively we still re-protect in `apply`,
/// not here, since `apply` is the only path that fully owns the page state).
///
/// In practice the restore-on-drop path writes back through the SAME page
/// that's now read-only-executable again; the win32 docs say WPM with the
/// pseudo-handle still honours protection. So this restore call WILL fail
/// silently for the auto-revert case unless we re-VirtualProtect.
///
/// Solution: this helper itself runs the VirtualProtect dance. That makes
/// it idempotent and safe for both the "apply" inner write and the
/// auto-restore path.
pub fn write_bytes_at(addr: usize, bytes: &[u8]) -> Result<(), ReadError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let len = bytes.len();
    let mut old_protect = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: VirtualProtect rejects bad input via FALSE return rather than
    // raising. `len > 0` and `addr` is provided by a previously-successful
    // apply (the auto-restore caller) or the apply path's own dance.
    let vp_ok = unsafe {
        VirtualProtect(
            addr as *const c_void,
            len,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if vp_ok.is_err() {
        return Err(ReadError::Failed);
    }
    let write_result = LocalReader::new().write_bytes(addr, bytes);
    // Restore even on failure.
    // SAFETY: as above.
    let _ = unsafe {
        VirtualProtect(
            addr as *const c_void,
            len,
            old_protect,
            &mut PAGE_PROTECTION_FLAGS(0),
        )
    };
    write_result?;
    // SAFETY: pseudo-handle + valid range we just wrote.
    unsafe {
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(addr as *const c_void), len);
    }
    Ok(())
}

/// Restore `original` to `addr`. Used as the explicit-restore path (the
/// `Request::RestorePatch` handler); the auto-restore path on connection
/// drop calls `write_bytes_at` directly via [`crate::ops::state::ConnState`].
pub fn restore(addr: usize, original: &[u8]) -> Result<(), PatchError> {
    if original.is_empty() {
        return Err(PatchError::EmptyReplacement);
    }
    write_bytes_at(addr, original).map_err(PatchError::WriteFailed)
}
