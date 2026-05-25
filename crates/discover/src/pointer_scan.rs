//! Shared scanning primitive for the discovery side of pointer-chain analysis.
//!
//! Two operations live here:
//!
//! 1. [`scan_for_pointers_to`] — walk every read-writable page of a remote
//!    process and record every aligned `u64` whose value falls into
//!    `[target - max_offset, target]`. This is the "find pointers to X" step.
//! 2. [`walk_chain`] — given a static module address and a sequence of
//!    `(deref, offset)` hops, verify the chain by replaying it via remote
//!    reads. Used by `trace-chain` to drop bogus candidates.
//!
//! `pages::enumerate_rw` is the single source of truth for region enumeration —
//! we never re-implement `VirtualQueryEx` here. Reads are chunked to keep peak
//! memory usage bounded; a per-page read failure (page unmapped between
//! `VirtualQueryEx` and the read) is silently skipped.

#![cfg(windows)]

use std::ffi::c_void;

use anyhow::Result;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;

use crate::pages::{self, MemoryRegion};

/// One source location whose 8-byte value points into the target window.
#[derive(Debug, Clone)]
pub struct PointerCandidate {
    pub source_addr: usize,
    pub value: usize,
    pub trailing_offset: usize,
}

/// Scan every RW page of `handle` for aligned `u64` values `v` such that
/// `target - max_offset <= v <= target`. `alignment` must be ≥ 1; values of 1
/// imply byte-granular scans (slow). Pass 8 for ordinary pointer scans.
///
/// The pages are read in `chunk_bytes`-sized slices (default 256 KiB if 0) to
/// keep peak memory bounded; the per-chunk overlap is `alignment - 1` so we
/// don't miss values straddling a chunk boundary.
pub fn scan_for_pointers_to(
    handle: HANDLE,
    target: usize,
    max_offset: usize,
    alignment: usize,
    chunk_bytes: usize,
) -> Result<Vec<PointerCandidate>> {
    let alignment = alignment.max(1);
    let chunk_bytes = if chunk_bytes == 0 {
        256 * 1024
    } else {
        chunk_bytes
    };
    let regions = pages::enumerate_rw(handle);
    let lower = target.saturating_sub(max_offset);
    let upper = target;

    let mut out = Vec::new();
    let mut buf = vec![0u8; chunk_bytes];
    for region in regions {
        let mut offset_in_region: usize = 0;
        while offset_in_region < region.size {
            let want = chunk_bytes.min(region.size - offset_in_region);
            let chunk_base = region.base + offset_in_region;
            let read_n = match read_chunk(handle, chunk_base, &mut buf[..want]) {
                Some(n) => n,
                None => {
                    // Page got unmapped (or never readable). Skip the rest of
                    // this region — partial reads inside a single RW region
                    // are unusual enough that bailing is cheaper than probing.
                    break;
                }
            };
            if read_n < 8 {
                break;
            }

            scan_chunk_for_pointers(
                &buf[..read_n],
                chunk_base,
                alignment,
                lower,
                upper,
                &mut out,
            );

            // Step forward by read_n - 7 so a u64 straddling the boundary is
            // still visible in the next chunk. (We re-scan up to 7 bytes of
            // overlap; cheap, and prevents pointer-near-boundary misses.)
            let step = read_n.saturating_sub(7).max(alignment);
            offset_in_region += step;
        }
    }
    Ok(out)
}

fn scan_chunk_for_pointers(
    chunk: &[u8],
    chunk_base: usize,
    alignment: usize,
    lower: usize,
    upper: usize,
    out: &mut Vec<PointerCandidate>,
) {
    if chunk.len() < 8 {
        return;
    }
    // Start at the first aligned offset within this chunk's *absolute* address.
    let first_aligned = chunk_base.next_multiple_of(alignment) - chunk_base;
    let mut i = first_aligned;
    while i + 8 <= chunk.len() {
        let v = u64::from_le_bytes(chunk[i..i + 8].try_into().unwrap()) as usize;
        if v >= lower && v <= upper {
            out.push(PointerCandidate {
                source_addr: chunk_base + i,
                value: v,
                trailing_offset: upper - v,
            });
        }
        i += alignment;
    }
}

fn read_chunk(handle: HANDLE, addr: usize, buf: &mut [u8]) -> Option<usize> {
    let mut nread: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            addr as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
            Some(&mut nread),
        )
        .is_ok()
    };
    if !ok || nread == 0 { None } else { Some(nread) }
}

/// Read a single `usize` (8-byte LE) from the target. Returns `None` if the
/// address isn't readable.
pub fn read_pointer(handle: HANDLE, addr: usize) -> Option<usize> {
    let mut buf = [0u8; 8];
    let n = read_chunk(handle, addr, &mut buf)?;
    if n < 8 {
        return None;
    }
    Some(u64::from_le_bytes(buf) as usize)
}

/// Walk `(deref, offset)` hops starting at `anchor`. Each `deref=true` hop
/// reads a `usize` from `current` and replaces `current` with that value;
/// every hop adds `offset` to `current` afterwards. Returns `None` on the
/// first read failure (chain dead).
pub fn walk_chain(handle: HANDLE, anchor: usize, hops: &[(bool, i64)]) -> Option<usize> {
    let mut current = anchor;
    for &(deref, offset) in hops {
        if deref {
            current = read_pointer(handle, current)?;
        }
        current = (current as i64).wrapping_add(offset) as usize;
    }
    Some(current)
}

/// Bracket a flat list of candidates per region (used by callers that want to
/// associate each hit with the region it came from). Kept here so callers
/// don't need to re-enumerate pages.
pub fn enumerate_rw_regions(handle: HANDLE) -> Vec<MemoryRegion> {
    pages::enumerate_rw(handle)
}
