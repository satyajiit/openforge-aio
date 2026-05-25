//! Read/write a target process's address space (Windows).
//!
//! **Status**: the trainer (`openforge-app`) NO LONGER calls these
//! functions. Every read / write / scan / patch the trainer does crosses the
//! pipe to a per-game DLL (`openforge-batman-lod-dll` for LotDK). The
//! functions here are kept for the **dev tooling** in `openforge-discover`
//! that scans / narrows / inspects arbitrary game processes from the host
//! side — that workflow has no DLL injected and can't go through a pipe.
//!
//! The two memory pipelines coexist deliberately: in-game (DLL, no RPM)
//! for the shipped trainer; out-of-game (RPM, this module) for the
//! discover CLI's interactive scans.

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY,
    VirtualQueryEx,
};

use crate::error::{Error, Result};
use crate::process::ProcessHandle;

/// One contiguous committed RW region in the target's address space.
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub base: usize,
    pub size: usize,
}

pub fn read_bytes(proc: &ProcessHandle, addr: usize, buf: &mut [u8]) -> Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let mut nread: usize = 0;
    unsafe {
        ReadProcessMemory(
            proc.raw(),
            addr as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
            Some(&mut nread),
        )
        .map_err(|_| Error::ReadFailed {
            addr,
            len: buf.len(),
            source: io::Error::last_os_error(),
        })?;
    }
    if nread != buf.len() {
        return Err(Error::ReadFailed {
            addr,
            len: buf.len(),
            source: io::Error::other(format!("short read: {nread}/{}", buf.len())),
        });
    }
    Ok(())
}

pub fn write_bytes(proc: &ProcessHandle, addr: usize, buf: &[u8]) -> Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let mut nwritten: usize = 0;
    unsafe {
        WriteProcessMemory(
            proc.raw(),
            addr as *const c_void,
            buf.as_ptr() as *const c_void,
            buf.len(),
            Some(&mut nwritten),
        )
        .map_err(|_| Error::WriteFailed {
            addr,
            len: buf.len(),
            source: io::Error::last_os_error(),
        })?;
    }
    if nwritten != buf.len() {
        return Err(Error::WriteFailed {
            addr,
            len: buf.len(),
            source: io::Error::other(format!("short write: {nwritten}/{}", buf.len())),
        });
    }
    Ok(())
}

/// Enumerate all committed, non-executable, non-guard RW regions in the target.
/// Used by the runtime's heap-value-scan locator.
pub fn enumerate_rw_regions(proc: &ProcessHandle) -> Vec<MemoryRegion> {
    let mut out = Vec::new();
    let mut addr: usize = 0;
    let mbi_size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
    loop {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let written =
            unsafe { VirtualQueryEx(proc.raw(), Some(addr as *const c_void), &mut mbi, mbi_size) };
        if written == 0 {
            break;
        }
        let base = mbi.BaseAddress as usize;
        let region_size = mbi.RegionSize;
        if region_size == 0 {
            break;
        }

        let is_committed = mbi.State == MEM_COMMIT;
        let prot = mbi.Protect.0;
        let is_rw = prot == PAGE_READWRITE.0 || prot == PAGE_WRITECOPY.0;
        let is_exec = prot
            & (PAGE_EXECUTE.0
                | PAGE_EXECUTE_READ.0
                | PAGE_EXECUTE_READWRITE.0
                | PAGE_EXECUTE_WRITECOPY.0)
            != 0;
        let is_guard = prot & PAGE_GUARD.0 != 0;
        let is_no_access = prot == PAGE_NOACCESS.0;

        if is_committed && is_rw && !is_exec && !is_guard && !is_no_access {
            out.push(MemoryRegion {
                base,
                size: region_size,
            });
        }

        let next = base.saturating_add(region_size);
        if next <= addr {
            break;
        }
        addr = next;
    }
    out
}

/// Scan all RW pages for aligned 8-byte values exactly equal to `needle`.
/// Used by `heap_value_scan` locators in the runtime.
///
/// Regions are processed in parallel via rayon — each worker uses its own
/// scratch buffer and never shares state with the others. ReadProcessMemory
/// is documented as safe to call concurrently against the same process handle
/// on Windows; the kernel serialises the actual page reads internally.
///
/// `on_progress(scanned_bytes, total_bytes)` is called periodically (cheap,
/// lock-free) so callers can drive a UI progress bar. The callback is allowed
/// to be slow — workers debounce internally so a single chatty callback won't
/// destroy throughput.
pub fn scan_rw_for_u64(
    proc: &ProcessHandle,
    needle: u64,
    alignment: usize,
    on_progress: impl Fn(u64, u64) + Sync,
) -> Vec<usize> {
    let align = alignment.max(1);
    let regions = enumerate_rw_regions(proc);
    let total_bytes: u64 = regions.iter().map(|r| r.size as u64).sum();
    tracing::debug!(
        regions = regions.len(),
        total_bytes,
        needle = format!("0x{needle:X}"),
        threads = rayon::current_num_threads(),
        "heap-scan begin"
    );

    // Lock-free running counter of bytes consumed across all worker threads.
    let scanned = AtomicU64::new(0);
    // Progress every ~16 MB of cumulative work. Workers compare-and-swap the
    // "next emit threshold" so only one thread fires the callback per slot.
    const PROGRESS_GRANULARITY: u64 = 16 * 1024 * 1024;
    let next_emit = AtomicU64::new(PROGRESS_GRANULARITY);

    // Initial tick so the UI knows the total upfront.
    on_progress(0, total_bytes);

    let mut hits: Vec<usize> = regions
        .par_iter()
        .flat_map_iter(|region| {
            // Per-worker scratch buffer — no shared allocation.
            // 4 MB chunks: empirically the sweet spot for ReadProcessMemory
            // throughput against UE5 processes on modern Windows. Halves
            // syscall overhead vs the legacy 256 KB chunk size used by the
            // discover CLI.
            let mut buf = vec![0u8; 4 * 1024 * 1024];
            let mut local: Vec<usize> = Vec::new();
            let mut offset = 0usize;
            let needle_bytes = needle.to_le_bytes();
            while offset < region.size {
                let chunk = (region.size - offset).min(buf.len());
                let slice = &mut buf[..chunk];
                if read_bytes(proc, region.base + offset, slice).is_err() {
                    break;
                }
                // Fast path: 8-byte stride scan when alignment == 8 (the common
                // case for u64 needles). Branch-free, vectorizable.
                if align == 8 {
                    let mut i = 0;
                    while i + 8 <= chunk {
                        if slice[i..i + 8] == needle_bytes {
                            local.push(region.base + offset + i);
                        }
                        i += 8;
                    }
                } else {
                    let mut i = 0;
                    while i + 8 <= chunk {
                        let abs_addr = region.base + offset + i;
                        if abs_addr.is_multiple_of(align) {
                            let v = u64::from_le_bytes(slice[i..i + 8].try_into().unwrap());
                            if v == needle {
                                local.push(abs_addr);
                            }
                        }
                        i += align;
                    }
                }
                // Account before adjusting offset for overlap — overlap reads
                // are real work, but for progress we only want to count
                // forward progress through the region.
                let advance = if chunk < region.size - offset {
                    chunk - 7
                } else {
                    chunk
                };
                // Bump the global counter and fire a progress event if we've
                // crossed the next threshold. We claim the slot atomically so
                // only one worker emits per granularity step.
                let new_total =
                    scanned.fetch_add(advance as u64, Ordering::Relaxed) + advance as u64;
                let threshold = next_emit.load(Ordering::Relaxed);
                if new_total >= threshold {
                    // Claim this slot. If we lose the CAS race, another
                    // thread will emit — that's fine, we skip.
                    let next = ((new_total / PROGRESS_GRANULARITY) + 1) * PROGRESS_GRANULARITY;
                    if next_emit
                        .compare_exchange(threshold, next, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                    {
                        on_progress(new_total.min(total_bytes), total_bytes);
                    }
                }
                offset += advance;
            }
            local.into_iter()
        })
        .collect();

    // Final tick at 100%.
    on_progress(total_bytes, total_bytes);

    // Deterministic ordering for downstream validators / tests.
    hits.sort_unstable();
    tracing::debug!(hits = hits.len(), "heap-scan done");
    hits
}
