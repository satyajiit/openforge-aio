//! In-DLL `.text` code patcher (protocol v6) — ported from the UE5 stack's
//! `batman-lod-dll/src/ops/code_patch.rs`, plus a per-process auto-restore
//! registry so a client disconnect reverts every applied patch (matching the
//! god-mode freeze's detach-revert).
//!
//! The overwrite dance: snapshot bytes → refuse if they don't equal the
//! caller's `original` (a mismatch means the game updated; never force-write
//! unknown instructions) → `VirtualProtect(PAGE_EXECUTE_READWRITE)` → write via
//! the local reader → restore protection → `FlushInstructionCache` so the CPU
//! stops executing the cached pre-patch bytes.

use std::collections::HashMap;
use std::ffi::c_void;

use parking_lot::Mutex;
use windows::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows::Win32::System::Memory::{
    PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect,
};
use windows::Win32::System::Threading::GetCurrentProcess;

use openforge_dll_common::local_reader::{LocalReader, ReadError};
use openforge_glacier_protocol::Response;

/// Applied patches: `addr → original bytes`. Restored on client disconnect.
static PATCHES: Mutex<Option<HashMap<usize, Vec<u8>>>> = Mutex::new(None);

fn with_registry<R>(f: impl FnOnce(&mut HashMap<usize, Vec<u8>>) -> R) -> R {
    let mut guard = PATCHES.lock();
    f(guard.get_or_insert_with(HashMap::new))
}

/// Handle [`Request::CodePatch`](openforge_glacier_protocol::Request::CodePatch).
pub fn code_patch(addr: u64, original: &[u8], patched: &[u8]) -> Response {
    match apply(addr as usize, original, patched) {
        Ok(_) => {
            with_registry(|m| m.insert(addr as usize, original.to_vec()));
            crate::flog!(
                "INFO",
                "code_patch applied @ 0x{:X} ({} bytes)",
                addr,
                patched.len()
            );
            Response::WriteOk
        }
        Err(e) => Response::Error(format!("code_patch @ 0x{addr:X}: {e}")),
    }
}

/// Handle [`Request::RestorePatch`](openforge_glacier_protocol::Request::RestorePatch).
pub fn restore_patch(addr: u64, original: &[u8]) -> Response {
    match write_bytes_at(addr as usize, original) {
        Ok(()) => {
            with_registry(|m| m.remove(&(addr as usize)));
            crate::flog!("INFO", "code_patch restored @ 0x{:X}", addr);
            Response::WriteOk
        }
        Err(e) => Response::Error(format!("restore_patch @ 0x{addr:X}: {e:?}")),
    }
}

/// Restore every still-applied patch and clear the registry. Called when a
/// client disconnects so a detach leaves the game's `.text` pristine.
pub fn restore_all() {
    let patches: Vec<(usize, Vec<u8>)> = with_registry(|m| m.drain().collect());
    if patches.is_empty() {
        return;
    }
    crate::flog!(
        "INFO",
        "restoring {} code patch(es) on disconnect",
        patches.len()
    );
    for (addr, original) in patches {
        let _ = write_bytes_at(addr, &original);
    }
}

#[derive(Debug)]
enum PatchError {
    EmptyReplacement,
    LengthMismatch { original: usize, replacement: usize },
    ReadFailed(ReadError),
    OriginalMismatch { expected: Vec<u8>, actual: Vec<u8> },
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
                "original and replacement must be the same length ({original} vs {replacement})"
            ),
            PatchError::ReadFailed(e) => write!(f, "verify-read failed: {e:?}"),
            PatchError::OriginalMismatch { expected, actual } => write!(
                f,
                "bytes don't match `original`; expected={expected:02X?} actual={actual:02X?} (game updated?)"
            ),
            PatchError::WriteFailed(e) => write!(f, "write failed: {e:?}"),
        }
    }
}

/// Overwrite `addr` with `replacement` after verifying it equals `original`.
fn apply(addr: usize, original: &[u8], replacement: &[u8]) -> Result<(), PatchError> {
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
    let mut actual = vec![0u8; len];
    LocalReader::new()
        .read_bytes(addr, &mut actual)
        .map_err(PatchError::ReadFailed)?;
    if actual != original {
        return Err(PatchError::OriginalMismatch {
            expected: original.to_vec(),
            actual,
        });
    }
    write_bytes_at(addr, replacement).map_err(PatchError::WriteFailed)
}

/// Write `bytes` to `addr` through the VirtualProtect dance + flush. Idempotent
/// and safe for both the patch-write and the restore paths (each fully owns the
/// page-protection toggle around its own write).
fn write_bytes_at(addr: usize, bytes: &[u8]) -> Result<(), ReadError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let len = bytes.len();
    let mut old = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: VirtualProtect reports bad input via FALSE rather than raising;
    // len > 0; `old` is a stack slot.
    let vp_ok =
        unsafe { VirtualProtect(addr as *const c_void, len, PAGE_EXECUTE_READWRITE, &mut old) };
    if vp_ok.is_err() {
        return Err(ReadError::Failed);
    }
    let write_result = LocalReader::new().write_bytes(addr, bytes);
    // SAFETY: restore the protection we just changed, even on write failure.
    let _ = unsafe {
        VirtualProtect(
            addr as *const c_void,
            len,
            old,
            &mut PAGE_PROTECTION_FLAGS(0),
        )
    };
    write_result?;
    // SAFETY: current-process pseudo-handle + the range we just wrote.
    unsafe {
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(addr as *const c_void), len);
    }
    Ok(())
}
