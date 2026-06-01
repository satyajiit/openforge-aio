//! In-process scanners: an RW-heap `u64` scan (for the reflection bootstrap's
//! "find pointers to a type-name string" step) and a masked AOB scan over a
//! module's `.text`. Both read through [`LocalReader`] so a page-protection
//! accident returns an error instead of crashing the game.

use std::ffi::c_void;

use openforge_core::Pattern;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use openforge_dll_common::local_reader::LocalReader;
use openforge_dll_common::pe::{LoadedModule, enumerate_modules};

// ---------------------------------------------------------------------------
// Heap u64 scan
// ---------------------------------------------------------------------------

/// Scan committed, writable, non-guard memory for aligned `u64` values equal to
/// `needle`. `alignment` is clamped to `>= 1`. `label` is echoed into the DLL
/// log for attribution.
///
/// Region enumeration is via in-process `VirtualQuery`, and each region is read
/// with **page-level fallback**: a bulk window read is all-or-nothing (a single
/// unmapped/guard page inside the window makes `ReadProcessMemory` fail for the
/// whole window), so on failure we retry the window page-by-page and lose only
/// the genuinely unmapped pages. A blind fixed-grid sweep without this fallback
/// dropped every window straddling a committed/unmapped boundary and missed the
/// fragmented high-memory arena entirely (player health/ammo value-boxes live at
/// 5-7 GiB across thousands of ~4 MiB regions).
///
/// Defence in depth against an anti-tamper layer that might blank the region map
/// for *in-process* queries: any region `VirtualQuery` reports as *free* is
/// probed with a single page read, and swept in full if `ReadProcessMemory` can
/// in fact read it (RPM on the pseudo-handle reaches pages a lying map denies).
/// The log line reports `regions`/`hidden`/`walked` so a coverage shortfall is
/// attributable. (Measured on 007 First Light: `hidden=0` — VirtualQuery is
/// honest here and page-fallback alone restores full coverage; the free-region
/// probe is cheap insurance, not a load-bearing workaround.)
pub fn heap_for_u64(needle: u64, alignment: u32, label: &str) -> Vec<u64> {
    let align = alignment.max(1) as usize;
    let reader = LocalReader::new();
    let needle_bytes = needle.to_le_bytes();
    let mut hits: Vec<u64> = Vec::new();
    // 4 MiB bulk window + a 4 KiB page-fallback buffer, reused across regions.
    let mut win = vec![0u8; 4 * 1024 * 1024];
    let mut page = vec![0u8; 4096];

    let mut walked: u64 = 0;
    let mut regions_read: u64 = 0;
    let mut hidden_regions: u64 = 0;

    const USER_MAX: usize = 0x0000_8000_0000_0000;
    const STEP: usize = 0x1_0000; // 64 KiB allocation granularity
    const MAX_REGION: usize = 16usize << 30; // skip pathological (>16 GiB) ranges
    let mbi_size = core::mem::size_of::<MEMORY_BASIC_INFORMATION>();
    let mut addr: usize = STEP; // start at the looks_readable floor (0x10000)
    let mut iters: u64 = 0;

    while addr < USER_MAX {
        iters += 1;
        if iters > 50_000_000 {
            crate::flog!(
                "WARN",
                "heap_scan[{label}]: iteration cap hit at 0x{addr:X}"
            );
            break;
        }
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery on the current process with a correctly-sized MBI.
        let n = unsafe { VirtualQuery(Some(addr as *const c_void), &mut mbi, mbi_size) };
        if n == 0 {
            // VirtualQuery couldn't classify this address. Don't assume unmapped:
            // probe a page, and on success sweep the granularity window.
            if reader.read_bytes(addr, &mut page).is_ok() {
                hidden_regions += 1;
                walked += scan_range(
                    &reader,
                    addr,
                    STEP,
                    &needle_bytes,
                    align,
                    &mut win,
                    &mut page,
                    &mut hits,
                );
            }
            addr = addr.saturating_add(STEP);
            continue;
        }
        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let next = base.saturating_add(size);
        if size == 0 || next <= addr {
            addr = addr.saturating_add(STEP);
            continue;
        }
        addr = next;

        let prot = mbi.Protect.0;
        let is_guard = prot & PAGE_GUARD.0 != 0;
        let is_writable = prot == PAGE_READWRITE.0
            || prot == PAGE_WRITECOPY.0
            || prot == PAGE_EXECUTE_READWRITE.0
            || prot == PAGE_EXECUTE_WRITECOPY.0;
        let committed = mbi.State == MEM_COMMIT;

        if committed && is_writable && !is_guard && size <= MAX_REGION {
            regions_read += 1;
            walked += scan_range(
                &reader,
                base,
                size,
                &needle_bytes,
                align,
                &mut win,
                &mut page,
                &mut hits,
            );
        } else if !committed && size <= MAX_REGION {
            // Reported free/reserved. If a tamper layer blanked a live region,
            // the bytes are still RPM-readable — probe the first page, and on a
            // hit sweep the whole claimed range. (Committed-but-read-only and
            // guard regions are legitimately skipped: not freeze targets.)
            if reader.read_bytes(base, &mut page).is_ok() {
                hidden_regions += 1;
                walked += scan_range(
                    &reader,
                    base,
                    size,
                    &needle_bytes,
                    align,
                    &mut win,
                    &mut page,
                    &mut hits,
                );
            }
        }
    }

    hits.sort_unstable();
    hits.dedup();
    let high = hits.iter().filter(|&&v| v >= 0x1_0000_0000).count();
    crate::flog!(
        "INFO",
        "heap_scan[{label}]: needle=0x{needle:X} align={align} walked={} MiB regions={} hidden={} hits={} (high={})",
        walked / 1048576,
        regions_read,
        hidden_regions,
        hits.len(),
        high
    );
    hits
}

/// Read `[base, base+len)` via [`LocalReader`] in 4 MiB windows, falling back to
/// page-granular reads when a window read fails, recording every `align`-aligned
/// match of `needle`. Returns the number of bytes actually read. `win` (4 MiB)
/// and `page` (4 KiB) are caller-owned scratch buffers reused across calls.
#[allow(clippy::too_many_arguments)]
fn scan_range(
    reader: &LocalReader,
    base: usize,
    len: usize,
    needle: &[u8; 8],
    align: usize,
    win: &mut [u8],
    page: &mut [u8],
    hits: &mut Vec<u64>,
) -> u64 {
    let mut walked = 0u64;
    let mut off = 0usize;
    while off < len {
        let want = (len - off).min(win.len());
        let a = base + off;
        if reader.read_bytes(a, &mut win[..want]).is_ok() {
            walked += want as u64;
            search_span(a, &win[..want], needle, align, hits);
        } else {
            // Window straddles an unmapped/guard page — recover the mapped pages.
            let mut p = 0usize;
            while p < want {
                let pn = (want - p).min(page.len());
                let pa = a + p;
                if reader.read_bytes(pa, &mut page[..pn]).is_ok() {
                    walked += pn as u64;
                    search_span(pa, &page[..pn], needle, align, hits);
                }
                p += pn;
            }
        }
        off += want;
    }
    walked
}

/// Record every `align`-aligned offset in `bytes` whose 8 bytes equal `needle`,
/// as an absolute VA. `abs` (the span base) is always page-aligned and `align`
/// divides the page size, so stepping by `align` keeps every candidate aligned
/// without a per-iteration modulo.
#[inline]
fn search_span(abs: usize, bytes: &[u8], needle: &[u8; 8], align: usize, hits: &mut Vec<u64>) {
    let mut i = 0usize;
    while i + 8 <= bytes.len() {
        if bytes[i..i + 8] == needle[..] {
            hits.push((abs + i) as u64);
        }
        i += align;
    }
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
                // Skip an unreadable chunk (guard/reserved page) and keep going;
                // do NOT abandon the region — pawns/value-boxes live past it.
                offset += chunk;
                continue;
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
    // 64 KiB allocation granularity — the step we use to recover from a
    // VirtualQuery that fails on a reserved/special range.
    const STEP: usize = 0x1_0000;
    let mut iters: u64 = 0;
    while addr < USER_MAX {
        iters += 1;
        if iters > 50_000_000 {
            // Backstop against a pathological walk; a real process has a few
            // thousand regions, so this never fires in practice.
            crate::flog!(
                "WARN",
                "enumerate_rw_regions: iteration cap hit at 0x{addr:X}"
            );
            break;
        }
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery on the current process is always safe; the
        // output buffer is sized correctly.
        let n = unsafe { VirtualQuery(Some(addr as *const c_void), &mut mbi, mbi_size) };
        if n == 0 {
            // VirtualQuery can fail on a reserved/special range. Do NOT abandon
            // the rest of the address space — that early `break` hid the
            // high-memory gameplay arena (every dynamic value-box lives there),
            // so the scanner saw only the low ~1 GiB. Step past and keep going.
            addr = addr.saturating_add(STEP);
            continue;
        }
        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let next = base.saturating_add(size);
        if size == 0 || next <= addr {
            addr = addr.saturating_add(STEP);
            continue;
        }
        addr = next;

        if mbi.State != MEM_COMMIT {
            continue;
        }
        let prot = mbi.Protect.0;
        // Any WRITABLE protection is a valid scan/freeze target — including
        // executable-writable (XRW/XWC) pages, which a JIT / anti-tamper engine
        // can place live gameplay state in. Exclude only guard pages.
        let is_writable = prot == PAGE_READWRITE.0
            || prot == PAGE_WRITECOPY.0
            || prot == PAGE_EXECUTE_READWRITE.0
            || prot == PAGE_EXECUTE_WRITECOPY.0;
        let is_guard = prot & PAGE_GUARD.0 != 0;
        if !is_writable || is_guard {
            continue;
        }
        // Skip only pathologically large regions (>16 GiB). The gameplay /
        // value-box arena IS a multi-GiB single region, so the old >1 GiB skip
        // made the player pawn and every dynamic value invisible — an
        // in-process memcpy+compare sweep of a few GiB is well under a second.
        if size > 16usize << 30 {
            continue;
        }
        out.push(RwRegion { base, size });
    }
    crate::flog!(
        "INFO",
        "enumerate_rw_regions: {} writable regions, highest base 0x{:X}",
        out.len(),
        out.last().map(|r| r.base).unwrap_or(0)
    );
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

/// Match `pattern` against an owned `bytes` buffer based at `base_va`,
/// returning absolute VAs (`base_va + offset`).
fn scan_buffer(bytes: &[u8], base_va: u64, pattern: &Pattern, first_only: bool) -> Vec<u64> {
    if first_only {
        match pattern.scan(bytes) {
            Some(off) => vec![base_va + off as u64],
            None => Vec::new(),
        }
    } else {
        pattern
            .scan_all(bytes)
            .into_iter()
            .map(|off| base_va + off as u64)
            .collect()
    }
}

/// Read `[start, start+len)` into an owned buffer with page-level fallback:
/// a bulk read is all-or-nothing, so on failure we retry 4 KiB at a time and
/// leave genuinely-unreadable pages zero-filled (they can't host a match).
fn read_span_pagefallback(start: usize, len: usize) -> Vec<u8> {
    let reader = LocalReader::new();
    let mut buf = vec![0u8; len];
    if reader.read_bytes(start, &mut buf).is_ok() {
        return buf;
    }
    let mut off = 0;
    while off < len {
        let n = core::cmp::min(4096, len - off);
        // Best-effort per-page; failures stay zeroed.
        let _ = LocalReader::new().read_bytes(start + off, &mut buf[off..off + n]);
        off += n;
    }
    buf
}

/// Scan `module_name`'s executable code for `pattern`, returning absolute VAs.
/// When `first_only` is set, stops at (and returns) the first match.
///
/// Fast path: scan the PE `.text` section. Fallback: if `.text` yields nothing,
/// scan the whole module image — 007 First Light's PE is header-mislabeled so a
/// large chunk of executable code sits in the `.udata`-named region *before*
/// `.text` (the ammo/recoil code-patch sites live there). The patterns we ship
/// are AOB-unique across the image, so the broader scan introduces no false
/// matches; it only runs when `.text` misses, so `.text`-resident lookups
/// (engine-fn prologues) keep their original fast, exact behaviour.
pub fn aob(module_name: &str, pattern: &Pattern, first_only: bool) -> Vec<u64> {
    let modules = enumerate_modules();
    let Some(m) = resolve_module(&modules, module_name) else {
        crate::flog!("WARN", "aob: module {module_name:?} not found");
        return Vec::new();
    };

    // Fast path: the declared .text section.
    if let Some(bytes) = read_text(m) {
        let hits = scan_buffer(&bytes, m.text_base() as u64, pattern, first_only);
        if !hits.is_empty() {
            return hits;
        }
    }

    // Fallback: scan the entire mapped image (covers the pre-.text executable
    // region this binary mislabels as .udata).
    if m.size == 0 {
        crate::flog!(
            "WARN",
            "aob: {} reports size=0; cannot full-image scan",
            m.name
        );
        return Vec::new();
    }
    crate::flog!(
        "INFO",
        "aob: .text miss in {}, full-image scan ({} MiB)",
        m.name,
        m.size / (1024 * 1024)
    );
    let image = read_span_pagefallback(m.base, m.size);
    scan_buffer(&image, m.base as u64, pattern, first_only)
}
