//! In-DLL, self-healing `GUObjectArray` resolver.
//!
//! `lotdk::ACTIVE.guobject_array_rva` is only a fast-path HINT. A game update —
//! or a different store binary (Epic/GOG/MS) — relinks the EXE and shifts that
//! RVA; the post-2026 LotDK patch moved it +0x4000. With no validation the
//! engine would read a zeroed header at the stale address and walk 0 objects,
//! so every reflection feature reports "no live instances" even mid-level.
//!
//! So we validate the hardcoded address STRUCTURALLY and, on failure,
//! re-discover `GUObjectArray` by its `FChunkedFixedUObjectArray` fingerprint
//! in the module image. The fingerprint depends on neither code patterns nor
//! log strings (both of which are codegen/Shipping-build specific), so it
//! survives recompiles and store variants. This mirrors the external
//! `discover ue5-locate` scan that confirmed the +0x4000 drift live.
//!
//! All reads go through `LocalReader` (`ReadProcessMemory(GetCurrentProcess())`)
//! so a wild candidate pointer returns an error instead of crashing the game —
//! mandatory given the injected DLL wraps handlers in `catch_unwind` and the
//! workspace builds with `panic = "unwind"`.

use openforge_dll_common::local_reader::LocalReader;
use openforge_dll_common::pe::enumerate_modules;
use openforge_ue5_protocol::PatternWire;

/// Elements per chunk in UE5's `FChunkedFixedUObjectArray` (NumElementsPerChunk).
const CHUNK_CAP: u64 = 65536;

/// Resolve the live `FChunkedFixedUObjectArray` (`GUObjectArray.ObjObjects`).
///
/// `hardcoded_va` is the BuildConfig fast-path hint. Returns `None` only when
/// BOTH the hint and the structural scan fail (e.g. injected before the game
/// populated its object array); callers should fall back to the hint and let
/// the per-feature retry path recover once a level loads.
pub fn resolve_guobject_array(reader: LocalReader, hardcoded_va: usize) -> Option<usize> {
    let modules = enumerate_modules();
    let main = modules.first()?;
    let base = main.base;
    let mod_end = base.checked_add(main.size)?;

    // Fast path: trust the hardcoded VA only if it validates structurally.
    if validate_candidate(reader, hardcoded_va, base, mod_end) {
        return Some(hardcoded_va);
    }
    crate::flog!(
        "WARN",
        "GUObjectArray hint 0x{hardcoded_va:X} failed structural validation; \
         running in-DLL fingerprint scan (game likely patched / different store build)"
    );

    scan_module_for_guobject(reader, base, main.size, mod_end)
}

/// Read the 32-byte header at `va` and confirm it is a live
/// `FChunkedFixedUObjectArray` (field invariants + deref to a real UObject).
fn validate_candidate(reader: LocalReader, va: usize, base: usize, mod_end: usize) -> bool {
    let mut header = [0u8; 32];
    if reader.read_bytes(va, &mut header).is_err() {
        return false;
    }
    match parse_header(&header) {
        Some((objects_ptr, _, _)) => confirm_via_deref(reader, objects_ptr, base, mod_end),
        None => false,
    }
}

/// Check the header field invariants. The chunk-math identity
/// `NumChunks == ceil(NumElements / 65536)` is the strong discriminator that
/// makes a false positive in `.data` essentially impossible.
///
/// `FChunkedFixedUObjectArray`:
///   +0x00 Objects**   (heap pointer to the chunk-pointer array)
///   +0x10 MaxElements (u32)
///   +0x14 NumElements (u32)
///   +0x18 MaxChunks   (u32)
///   +0x1C NumChunks   (u32)
fn parse_header(header: &[u8; 32]) -> Option<(usize, u32, u32)> {
    let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap());
    let max_elements = u32::from_le_bytes(header[0x10..0x14].try_into().unwrap());
    let num_elements = u32::from_le_bytes(header[0x14..0x18].try_into().unwrap());
    let max_chunks = u32::from_le_bytes(header[0x18..0x1C].try_into().unwrap());
    let num_chunks = u32::from_le_bytes(header[0x1C..0x20].try_into().unwrap());
    let expected_chunks = (num_elements as u64).div_ceil(CHUNK_CAP);
    let ok = (1000..=50_000_000).contains(&num_elements)
        && max_elements >= num_elements
        && max_elements <= 60_000_000
        && (num_chunks as u64) >= expected_chunks
        && (num_chunks as u64) <= expected_chunks + 2
        && max_chunks >= num_chunks
        && max_chunks <= 4000
        && objects_ptr != 0
        && objects_ptr.is_multiple_of(8)
        && objects_ptr > 0x10000;
    ok.then_some((objects_ptr as usize, num_elements, num_chunks))
}

/// Confirm a candidate by dereferencing `Objects[0][0].Object` and checking its
/// vtable lands inside the main module image — i.e. a real UObject lives there.
fn confirm_via_deref(reader: LocalReader, objects_ptr: usize, base: usize, mod_end: usize) -> bool {
    let Ok(chunk0) = reader.read_ptr(objects_ptr) else {
        return false;
    };
    if chunk0 == 0 || !chunk0.is_multiple_of(8) || chunk0 <= 0x10000 {
        return false;
    }
    // First FUObjectItem.Object sits at offset 0 within the chunk.
    let Ok(first_obj) = reader.read_ptr(chunk0) else {
        return false;
    };
    if first_obj == 0 || !first_obj.is_multiple_of(8) || first_obj <= 0x10000 {
        return false;
    }
    // The UObject vtable (offset 0) points into the module image.
    let Ok(vtable) = reader.read_ptr(first_obj) else {
        return false;
    };
    (base..mod_end).contains(&vtable)
}

/// Scan the main module image for the `FChunkedFixedUObjectArray` fingerprint.
fn scan_module_for_guobject(
    reader: LocalReader,
    base: usize,
    size: usize,
    mod_end: usize,
) -> Option<usize> {
    // Page-granular snapshot with zero-filled gaps: unreadable pages stay 0,
    // which the NumElements gate rejects. Single transient allocation, freed on
    // return; the global is in initialized `.data`, so the page holding it
    // always reads back.
    const PAGE: usize = 0x1000;
    let mut buf = vec![0u8; size];
    let mut p = 0usize;
    while p < size {
        let n = PAGE.min(size - p);
        // Ignore per-page failures — gaps stay zero-filled.
        let _ = reader.read_bytes(base + p, &mut buf[p..p + n]);
        p += n;
    }

    let mut off = 0usize;
    while off + 32 <= size {
        // Cheap gate on NumElements skips ~all offsets before the costlier checks.
        let num_elements = u32::from_le_bytes(buf[off + 0x14..off + 0x18].try_into().unwrap());
        if (1000..=50_000_000).contains(&num_elements) {
            let header: &[u8; 32] = buf[off..off + 32].try_into().unwrap();
            if let Some((objects_ptr, num, chunks)) = parse_header(header)
                && confirm_via_deref(reader, objects_ptr, base, mod_end)
            {
                let resolved = base + off;
                crate::flog!(
                    "INFO",
                    "GUObjectArray re-discovered via fingerprint @ 0x{resolved:X} \
                     (RVA 0x{:X}, NumElements={num}, NumChunks={chunks})",
                    resolved - base
                );
                return Some(resolved);
            }
        }
        off += 8;
    }
    crate::flog!("ERROR", "GUObjectArray fingerprint scan found no candidate");
    None
}

// ---------------------------------------------------------------------------
// No-hardcode function resolvers (FName::ToString, ProcessEvent)
//
// Both engine functions used to be baked absolute RVAs. A game patch relinks
// the binary and shifts every RVA (the 2026 LotDK patch moved ProcessEvent
// +0x50 and GUObjectArray +0x4000), silently breaking every reflection /
// UFunction feature. These resolvers find the functions structurally instead,
// so there's nothing to go stale.
// ---------------------------------------------------------------------------

/// Parse a human AOB string (`"48 8B ?? .."`, `??` = wildcard) into a
/// [`PatternWire`] (`mask` byte `0xFF` = literal, `0x00` = wildcard — the
/// encoding `crate::ops::aob_scan` expects).
fn parse_aob(sig: &str) -> Option<PatternWire> {
    let mut bytes = Vec::new();
    let mut mask = Vec::new();
    for tok in sig.split_whitespace() {
        if tok == "??" || tok == "?" {
            bytes.push(0);
            mask.push(0x00);
        } else {
            bytes.push(u8::from_str_radix(tok, 16).ok()?);
            mask.push(0xFF);
        }
    }
    if bytes.is_empty() {
        return None;
    }
    Some(PatternWire { bytes, mask })
}

/// Resolve `FName::ToString` with NO baked address.
///
/// Scans the main module's `.text` for each prologue signature, then hands the
/// hits (as RVAs) to the existing behavioral probe — which calls each under
/// SEH with `FName{0,0}` and accepts the first that decodes to `"None"`. The
/// signature survives relinks (identical bytes, new location); the behavioral
/// check is the decisive, build-independent validator.
pub fn resolve_fname_to_string(module_base: usize, sigs: &[&str]) -> Option<usize> {
    let mut candidate_rvas: Vec<usize> = Vec::new();
    for sig in sigs {
        let Some(pat) = parse_aob(sig) else {
            crate::flog!("WARN", "fname locate: un-parseable signature, skipping");
            continue;
        };
        for hit in crate::ops::aob_scan::scan_all("", &pat) {
            let rva = (hit as usize).saturating_sub(module_base);
            if !candidate_rvas.contains(&rva) {
                candidate_rvas.push(rva);
            }
        }
    }
    crate::flog!(
        "INFO",
        "fname locate: {} signature hit(s) to behaviorally validate",
        candidate_rvas.len()
    );
    crate::probe::find_fname_to_string(module_base, &candidate_rvas)
}

/// Number of UObject vtable slots to inspect when hunting ProcessEvent.
/// ProcessEvent sits around index ~76 on this build; 192 is a generous ceiling.
const PE_VTABLE_SLOTS: usize = 192;
/// `FUObjectItem` stride — matches `UeEngine::fuobject_item_stride` (UE5 stock).
const FUOBJECT_ITEM_STRIDE: usize = 24;

/// Resolve `UObject::ProcessEvent` with NO baked address and NO baked vtable
/// index.
///
/// ProcessEvent is a UObject base virtual: its pointer occupies the same vtable
/// slot across (almost) every class. We sample a handful of *distinct* live
/// vtables, find the slot whose target is shared by a majority AND whose
/// prologue matches the ProcessEvent signature, and return that pointer. The
/// bare prologue is generic (~30 `.text` hits), but only ProcessEvent occupies
/// a universal UObject-vtable slot with it — so the vtable cross-check pins it
/// uniquely without scanning `.text` or assuming an index.
pub fn resolve_process_event(
    reader: LocalReader,
    guobject_array: usize,
    pe_sig: &str,
) -> Option<usize> {
    let pat = parse_aob(pe_sig)?;
    let modules = enumerate_modules();
    let main = modules.first()?;
    let base = main.base;
    let mod_end = base.checked_add(main.size)?;

    // Header → live object array geometry.
    let mut header = [0u8; 32];
    reader.read_bytes(guobject_array, &mut header).ok()?;
    let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
    let num_elements = u32::from_le_bytes(header[0x14..0x18].try_into().unwrap()) as usize;
    let num_chunks = u32::from_le_bytes(header[0x1C..0x20].try_into().unwrap()) as usize;
    if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
        return None;
    }

    // Collect up to 16 DISTINCT in-module vtables from objects spread across
    // the array (deduping on the vtable pointer — many classes share one).
    let chunk_cap = CHUNK_CAP as usize;
    let step = (num_elements / 40_000).max(1);
    let mut vtables: Vec<usize> = Vec::new();
    let mut idx = 0usize;
    while idx < num_elements && vtables.len() < 16 {
        let ci = idx / chunk_cap;
        let wi = idx % chunk_cap;
        idx += step;
        if ci >= num_chunks {
            break;
        }
        let Ok(chunk) = reader.read_ptr(objects_ptr + ci * 8) else {
            continue;
        };
        if chunk == 0 {
            continue;
        }
        let Ok(obj) = reader.read_ptr(chunk + wi * FUOBJECT_ITEM_STRIDE) else {
            continue;
        };
        if obj == 0 || !obj.is_multiple_of(8) || obj <= 0x10000 {
            continue;
        }
        let Ok(vt) = reader.read_ptr(obj) else {
            continue;
        };
        if (base..mod_end).contains(&vt) && !vtables.contains(&vt) {
            vtables.push(vt);
        }
    }
    if vtables.len() < 3 {
        crate::flog!(
            "WARN",
            "ProcessEvent locate: only {} distinct vtable(s) sampled; too few to disambiguate",
            vtables.len()
        );
        return None;
    }

    // For each slot, take the most common in-module target. If a majority of
    // vtables agree on it AND its prologue matches, it's ProcessEvent.
    let need = (vtables.len() * 3).div_ceil(5); // >= 60%
    let mut best: Option<(usize, usize, usize)> = None; // (agree_count, slot, addr)
    for slot in 0..PE_VTABLE_SLOTS {
        let mut counts: Vec<(usize, usize)> = Vec::new(); // (addr, count)
        for &vt in &vtables {
            if let Ok(t) = reader.read_ptr(vt + slot * 8)
                && (base..mod_end).contains(&t)
            {
                match counts.iter_mut().find(|(a, _)| *a == t) {
                    Some(e) => e.1 += 1,
                    None => counts.push((t, 1)),
                }
            }
        }
        let Some(&(addr, count)) = counts.iter().max_by_key(|(_, c)| *c) else {
            continue;
        };
        if count >= need && prologue_matches(reader, addr, &pat) {
            crate::flog!(
                "DEBUG",
                "ProcessEvent locate: slot {slot} -> 0x{addr:X} ({count}/{} agree, prologue match)",
                vtables.len()
            );
            if best.as_ref().is_none_or(|(bc, _, _)| count > *bc) {
                best = Some((count, slot, addr));
            }
        }
    }

    match best {
        Some((count, slot, addr)) => {
            crate::flog!(
                "INFO",
                "ProcessEvent resolved @ 0x{addr:X} (RVA 0x{:X}) via UObject vtable slot {slot} \
                 ({count}/{} distinct vtables agree + prologue match) — no baked address",
                addr - base,
                vtables.len()
            );
            Some(addr)
        }
        None => {
            crate::flog!(
                "ERROR",
                "ProcessEvent locate: no universal vtable slot matched the prologue \
                 across {} vtables; UFunction features unavailable",
                vtables.len()
            );
            None
        }
    }
}

/// Read `pat.len()` bytes at `addr` (fault-isolated) and test the masked match.
fn prologue_matches(reader: LocalReader, addr: usize, pat: &PatternWire) -> bool {
    let mut buf = vec![0u8; pat.bytes.len()];
    if reader.read_bytes(addr, &mut buf).is_err() {
        return false;
    }
    pat.bytes
        .iter()
        .zip(&pat.mask)
        .enumerate()
        .all(|(k, (b, m))| *m != 0xFF || buf[k] == *b)
}
