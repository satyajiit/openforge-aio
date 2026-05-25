//! In-process heap scanner. Walks every committed, non-executable,
//! non-guard RW region in our address space (which IS the game's address
//! space) and returns the addresses where an aligned `u64` equals `needle`.
//!
//! Faster than the host-side scanner because in-process reads cost a single
//! `memcpy`-equivalent rather than the `ReadProcessMemory` round-trip the
//! out-of-process scanner pays per chunk. For a typical UE5 game with ~6 GB
//! of committed RW memory, the in-process scan finishes in ~150 ms on a
//! modern CPU.

use std::ffi::c_void;

use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY,
    VirtualQuery,
};

use crate::local_reader::LocalReader;

/// Scan every RW page for aligned u64 values equal to `needle`. `alignment`
/// is clamped to `>= 1`; values that aren't aligned are skipped. `label` is
/// echoed into the DLL log for attribution (typically the feature id).
pub fn scan(needle: u64, alignment: u32, label: &str) -> Vec<u64> {
    let align = alignment.max(1) as usize;
    let regions = enumerate_rw_regions();
    crate::flog!(
        "INFO",
        "heap_scan[{label}]: needle=0x{needle:X} align={align} regions={}",
        regions.len()
    );

    let reader = LocalReader::new();
    let needle_bytes = needle.to_le_bytes();
    let mut hits: Vec<u64> = Vec::new();
    // 4 MB scratch buffer — same sweet spot the host-side scanner uses.
    let mut buf = vec![0u8; 4 * 1024 * 1024];

    for region in &regions {
        let mut offset = 0usize;
        while offset < region.size {
            let chunk = (region.size - offset).min(buf.len());
            let slice = &mut buf[..chunk];
            if reader.read_bytes(region.base + offset, slice).is_err() {
                break;
            }
            if align == 8 {
                let mut i = 0;
                while i + 8 <= chunk {
                    if slice[i..i + 8] == needle_bytes {
                        hits.push((region.base + offset + i) as u64);
                    }
                    i += 8;
                }
            } else {
                let mut i = 0;
                while i + 8 <= chunk {
                    let abs = region.base + offset + i;
                    if abs.is_multiple_of(align) && slice[i..i + 8] == needle_bytes {
                        hits.push(abs as u64);
                    }
                    i += align;
                }
            }
            // Slide with a 7-byte overlap so a needle straddling the chunk
            // boundary is still found.
            let advance = if chunk < region.size - offset {
                chunk - 7
            } else {
                chunk
            };
            offset += advance;
        }
    }
    hits.sort_unstable();
    hits.dedup();
    crate::flog!("INFO", "heap_scan[{label}]: hits={}", hits.len());
    hits
}

/// One committed, non-executable, non-guard RW region. Discovered via
/// `VirtualQuery` against the current process.
#[derive(Clone, Copy)]
struct RwRegion {
    base: usize,
    size: usize,
}

fn enumerate_rw_regions() -> Vec<RwRegion> {
    let mut out = Vec::new();
    let mut addr: usize = 0;
    let mbi_size = core::mem::size_of::<MEMORY_BASIC_INFORMATION>();
    // x64 user-mode VA ceiling.
    const USER_MAX: usize = 0x0000_8000_0000_0000;
    while addr < USER_MAX {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery on the current process is always safe; the
        // output buffer is sized correctly.
        let n = unsafe { VirtualQuery(Some(addr as *const c_void), &mut mbi, mbi_size) };
        if n == 0 {
            break;
        }
        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        if size == 0 {
            break;
        }
        let next = base.saturating_add(size);
        if next <= addr {
            break;
        }
        addr = next;

        if mbi.State != MEM_COMMIT {
            continue;
        }
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
        if !is_rw || is_exec || is_guard || is_no_access {
            continue;
        }
        // Skip absurdly large regions (>1 GB) — they're almost never single
        // arenas and walking them word-by-word burns minutes for no value.
        if size > 1 << 30 {
            continue;
        }
        out.push(RwRegion { base, size });
    }
    out
}
