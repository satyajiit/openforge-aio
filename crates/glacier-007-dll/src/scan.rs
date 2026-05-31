//! In-process scanners: an RW-heap `u64` scan (for the reflection bootstrap's
//! "find pointers to a type-name string" step) and a masked AOB scan over a
//! module's `.text`. Both read through [`LocalReader`] so a page-protection
//! accident returns an error instead of crashing the game.

use std::ffi::c_void;

use openforge_core::Pattern;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY,
    VirtualQuery,
};

use crate::local_reader::LocalReader;
use crate::pe::{LoadedModule, enumerate_modules};

// ---------------------------------------------------------------------------
// Heap u64 scan
// ---------------------------------------------------------------------------

/// Scan every committed, non-executable, non-guard RW region for aligned `u64`
/// values equal to `needle`. `alignment` is clamped to `>= 1`. `label` is
/// echoed into the DLL log for attribution.
///
/// In-process reads cost a `memcpy`-equivalent rather than the per-chunk
/// `ReadProcessMemory` round-trip an out-of-process scanner pays, so a full
/// sweep of a multi-GB address space finishes in a few hundred milliseconds.
pub fn heap_for_u64(needle: u64, alignment: u32, label: &str) -> Vec<u64> {
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
    // 4 MiB scratch buffer.
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

/// Visit every 8-aligned address in committed RW memory whose first qword
/// (the object's vtable slot) points into `[mod_lo, mod_hi)` — i.e. candidate
/// C++ object bases. `visit` returns `false` to stop early (e.g. a result cap).
///
/// The vtable-in-module filter is a pure in-buffer comparison (no per-slot
/// `ReadProcessMemory`), so a full multi-GB sweep is a few hundred ms; only the
/// in-range hits incur the caller's deeper validation reads.
pub fn for_each_candidate_object(mod_lo: u64, mod_hi: u64, mut visit: impl FnMut(u64) -> bool) {
    let reader = LocalReader::new();
    // 4 MiB scratch buffer; a multiple of 8 so qword iteration never straddles
    // a chunk boundary and we can advance by the full chunk.
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let mut examined: u64 = 0;
    let mut vtable_hits: u64 = 0;
    let mut next_log: u64 = 1 << 27; // progress line every ~128M qwords
    for region in enumerate_rw_regions() {
        let mut offset = 0usize;
        while offset < region.size {
            let chunk = (region.size - offset).min(buf.len());
            let usable = chunk & !7; // whole qwords only
            let slice = &mut buf[..chunk];
            if reader.read_bytes(region.base + offset, slice).is_err() {
                break;
            }
            let mut i = 0;
            while i + 8 <= usable {
                examined += 1;
                let vptr = u64::from_le_bytes(slice[i..i + 8].try_into().unwrap());
                if vptr >= mod_lo && vptr < mod_hi {
                    vtable_hits += 1;
                    let candidate = (region.base + offset + i) as u64;
                    if !visit(candidate) {
                        crate::flog!(
                            "INFO",
                            "candidate scan: stopped early ({examined} qwords, {vtable_hits} vtable hits)"
                        );
                        return;
                    }
                }
                i += 8;
            }
            if examined >= next_log {
                crate::flog!(
                    "INFO",
                    "candidate scan progress: {examined} qwords, {vtable_hits} vtable hits"
                );
                next_log = next_log.wrapping_add(1 << 27);
            }
            offset += chunk;
        }
    }
    crate::flog!(
        "INFO",
        "candidate scan done: {examined} qwords, {vtable_hits} vtable hits"
    );
}

#[derive(Clone, Copy)]
struct RwRegion {
    base: usize,
    size: usize,
}

fn enumerate_rw_regions() -> Vec<RwRegion> {
    let mut out = Vec::new();
    let mut addr: usize = 0;
    let mbi_size = core::mem::size_of::<MEMORY_BASIC_INFORMATION>();
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
        // Skip absurdly large regions (>1 GiB) — almost never single arenas;
        // walking them word-by-word burns time for no value.
        if size > 1 << 30 {
            continue;
        }
        out.push(RwRegion { base, size });
    }
    out
}

// ---------------------------------------------------------------------------
// AOB scan over a module's .text
// ---------------------------------------------------------------------------

/// Resolve a module by case-insensitive name (`""` = the main module, which
/// Toolhelp32 always reports first).
fn resolve_module<'a>(modules: &'a [LoadedModule], name: &str) -> Option<&'a LoadedModule> {
    if name.is_empty() {
        return modules.first();
    }
    modules.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

/// Read a module's `.text` section into an owned buffer.
fn read_text(m: &LoadedModule) -> Option<Vec<u8>> {
    if m.text_size == 0 {
        return None;
    }
    let mut buf = vec![0u8; m.text_size];
    LocalReader::new()
        .read_bytes(m.text_base(), &mut buf)
        .ok()?;
    Some(buf)
}

/// Scan `module_name`'s `.text` for `pattern`, returning absolute VAs. When
/// `first_only` is set, stops at (and returns) the first match.
pub fn aob(module_name: &str, pattern: &Pattern, first_only: bool) -> Vec<u64> {
    let modules = enumerate_modules();
    let Some(m) = resolve_module(&modules, module_name) else {
        crate::flog!("WARN", "aob: module {module_name:?} not found");
        return Vec::new();
    };
    let Some(bytes) = read_text(m) else {
        crate::flog!(
            "WARN",
            "aob: {} has no parseable .text (text_size=0 or read failed)",
            m.name
        );
        return Vec::new();
    };
    let text_base = m.text_base() as u64;
    if first_only {
        match pattern.scan(&bytes) {
            Some(off) => vec![text_base + off as u64],
            None => Vec::new(),
        }
    } else {
        pattern
            .scan_all(&bytes)
            .into_iter()
            .map(|off| text_base + off as u64)
            .collect()
    }
}
