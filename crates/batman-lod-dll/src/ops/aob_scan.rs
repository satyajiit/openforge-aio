//! In-DLL AOB scanner. Walks a target module's `.text` section once, then
//! runs a masked-byte search over the snapshot. Reads use `LocalReader` so
//! page-protection accidents return errors instead of crashing the game.

use openforge_dll_common::local_reader::LocalReader;
use openforge_dll_common::pe::{LoadedModule, enumerate_modules};
use openforge_ue5_protocol::PatternWire;

/// Resolve a module by case-insensitive name (`""` = the main module —
/// matched by being the first enumerated entry, which Toolhelp32 always
/// reports as the EXE itself).
fn resolve_module<'a>(modules: &'a [LoadedModule], name: &str) -> Option<&'a LoadedModule> {
    if name.is_empty() {
        return modules.first();
    }
    modules.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

/// Read a module's `.text` section into an owned buffer. Returns `None` if
/// the module isn't present, has no parseable `.text`, or the read fails.
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

/// Scan `module_name`'s `.text` for `pattern`. Returns up to one absolute
/// address (the first match). Empty `module_name` selects the main module.
pub fn scan_first(module_name: &str, pattern: &PatternWire) -> Vec<u64> {
    scan_inner(module_name, pattern, true)
}

/// Scan `module_name`'s `.text` for every match of `pattern`. Used by
/// signature-uniqueness probes.
pub fn scan_all(module_name: &str, pattern: &PatternWire) -> Vec<u64> {
    scan_inner(module_name, pattern, false)
}

fn scan_inner(module_name: &str, pattern: &PatternWire, first_only: bool) -> Vec<u64> {
    let modules = enumerate_modules();
    let Some(m) = resolve_module(&modules, module_name) else {
        crate::flog!("WARN", "aob_scan: module {module_name:?} not found");
        return Vec::new();
    };
    let Some(bytes) = read_text(m) else {
        crate::flog!(
            "WARN",
            "aob_scan: {} has no parseable .text (text_size=0 or read failed)",
            m.name
        );
        return Vec::new();
    };
    if pattern.bytes.len() != pattern.mask.len() || pattern.bytes.is_empty() {
        crate::flog!(
            "WARN",
            "aob_scan: invalid PatternWire (bytes={}, mask={})",
            pattern.bytes.len(),
            pattern.mask.len()
        );
        return Vec::new();
    }
    let text_base = m.text_base() as u64;
    let mut out = Vec::new();
    let plen = pattern.bytes.len();
    if bytes.len() < plen {
        return out;
    }
    // Choose an anchor: the first byte with a literal mask. Cuts the inner
    // loop by ~94% for a typical signature (most bytes are wildcards in
    // RIP-relative patterns).
    let anchor = match pattern.mask.iter().position(|&m| m == 0xFF) {
        Some(i) => i,
        None => {
            crate::flog!("WARN", "aob_scan: pattern has no literal bytes");
            return out;
        }
    };
    let anchor_byte = pattern.bytes[anchor];
    let end = bytes.len() - plen + 1;
    for i in 0..end {
        if bytes[i + anchor] != anchor_byte {
            continue;
        }
        if matches_at(&bytes, i, &pattern.bytes, &pattern.mask) {
            out.push(text_base + i as u64);
            if first_only {
                return out;
            }
        }
    }
    out
}

#[inline]
fn matches_at(hay: &[u8], offset: usize, pat: &[u8], mask: &[u8]) -> bool {
    for k in 0..pat.len() {
        if mask[k] == 0xFF && hay[offset + k] != pat[k] {
            return false;
        }
    }
    true
}
